//! Explicit local target capture. The native spool is durable until the engine
//! single writer acknowledges a committed exchange. No SQL or WebView API.
use crate::{CapabilityRegistry, SecretVault, tools::atomic_write};
use artex_core::{Error, Result, new_id};
use futures_util::TryStreamExt;
use http_body_util::BodyExt;
use hudsucker::{
    Body, HttpContext, HttpHandler, Proxy, RequestOrResponse,
    certificate_authority::CertificateAuthority,
    hyper::{
        Request, Response,
        body::{Body as HttpBody, Bytes, Frame, SizeHint},
    },
    rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
        KeyUsagePurpose,
    },
    rustls::crypto::aws_lc_rs,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Write},
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    task::{Context, Poll},
    thread,
    time::Duration,
};
// rcgen's CertificateParams::new handles IP SANs as IP addresses, unlike the
// upstream authority's DNS-only implementation. Never relax client validation.
struct NativeAuthority {
    issuer: Issuer<'static, KeyPair>,
    cache: Mutex<HashMap<String, Arc<hudsucker::rustls::ServerConfig>>>,
    fallback: Arc<hudsucker::rustls::ServerConfig>,
}
impl NativeAuthority {
    fn new(issuer: Issuer<'static, KeyPair>) -> Result<Self> {
        let fallback = Self::certificate(&issuer, "localhost")?;
        Ok(Self {
            issuer,
            cache: Mutex::new(HashMap::new()),
            fallback,
        })
    }
    fn certificate(
        issuer: &Issuer<'static, KeyPair>,
        host: &str,
    ) -> Result<Arc<hudsucker::rustls::ServerConfig>> {
        use hudsucker::rustls::{
            ServerConfig,
            pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let mut params = CertificateParams::new(vec![host.into()])
            .map_err(|_| Error::new("INVALID_PARAMS", "Invalid certificate hostname"))?;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![hudsucker::rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let leaf = KeyPair::generate()
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "Capture leaf key generation failed"))?;
        let cert = params
            .signed_by(&leaf, issuer)
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "Capture leaf signing failed"))?;
        let mut config =
            ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|_| Error::new("EXTERNAL_FAILURE", "TLS versions unavailable"))?
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.der().clone()],
                    PrivateKeyDer::from(PrivatePkcs8KeyDer::from(leaf.serialize_der())),
                )
                .map_err(|_| Error::new("EXTERNAL_FAILURE", "TLS certificate unavailable"))?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}
impl CertificateAuthority for NativeAuthority {
    async fn gen_server_config(
        &self,
        authority: &hudsucker::hyper::http::uri::Authority,
    ) -> Arc<hudsucker::rustls::ServerConfig> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(config) = cache.get(authority.host()) {
            return config.clone();
        }
        let config = Self::certificate(&self.issuer, authority.host())
            .unwrap_or_else(|_| self.fallback.clone());
        if cache.len() >= 1000 {
            cache.clear();
        }
        cache.insert(authority.host().into(), config.clone());
        config
    }
}
const DISK: u64 = 512 * 1024 * 1024;
const BODY: u64 = 64 * 1024 * 1024;
struct Spool {
    directory: PathBuf,
    entries: Mutex<BTreeMap<String, Value>>,
    live: Mutex<HashMap<String, Arc<Mutex<Capture>>>>,
    bytes: AtomicU64,
}
struct Capture {
    id: String,
    data: Value,
    req_done: bool,
    resp_done: bool,
    queued: bool,
    bytes: u64,
    active_tees: usize,
    handler_done: bool,
}
struct Active {
    epoch: String,
    proxy_url: String,
    ca_pem: String,
    ca_path: PathBuf,
    upstream: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
#[derive(Default)]
pub struct TrafficManager {
    active: Mutex<Option<Active>>,
    retained: Mutex<Option<Arc<Spool>>>,
    direct_proxy: Mutex<String>,
    configuration: Mutex<()>,
}
impl TrafficManager {
    pub fn configure(
        &self,
        registry: &CapabilityRegistry,
        vault: &SecretVault,
        input: &Value,
        epoch: &str,
    ) -> Result<Value> {
        let _configuration = self.configuration.lock().unwrap();
        let enabled = input["enabled"]
            .as_bool()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Explicit capture toggle required"))?;
        let upstream = input["global_proxy"].as_str().unwrap_or("").to_string();
        if !upstream.is_empty() {
            let u = reqwest::Url::parse(&upstream)
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid global proxy"))?;
            if !matches!(u.scheme(), "http" | "https" | "socks5" | "socks5h") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Unsupported global proxy scheme",
                ));
            }
        }
        if let Some(active) = self.active.lock().unwrap().as_ref() {
            if active.epoch == epoch && active.upstream == upstream && enabled {
                return Ok(info(active));
            }
        }
        self.stop();
        *self.direct_proxy.lock().unwrap() = upstream.clone();
        if !enabled {
            return Ok(json!({"enabled":false,"proxy_url":upstream,"ca_pem":null,"ca_path":null}));
        }
        let cap = input["directory_capability"].as_str().ok_or_else(|| {
            Error::new("PERMISSION_DENIED", "Capture directory capability required")
        })?;
        let directory = registry.resolve(cap, ".artex-host/traffic", true)?;
        fs::create_dir_all(&directory).map_err(Error::io)?;
        let certfile = registry.resolve(cap, ".artex-host/traffic/ca.pem", true)?;
        let descriptor = registry.resolve(cap, ".artex-host/traffic/ca.json", true)?;
        let (pem, key) = if descriptor.exists() {
            if fs::metadata(&descriptor).map_err(Error::io)?.len() > 32768 {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Oversized capture CA descriptor",
                ));
            }
            let d: Value = serde_json::from_slice(&fs::read(&descriptor).map_err(Error::io)?)?;
            let pem = d["certificate"]
                .as_str()
                .ok_or_else(|| Error::new("CORRUPT_STORE", "Capture CA missing"))?
                .to_owned();
            let key = vault.resolve(d["key"]["secret_ref"].as_str().ok_or_else(|| {
                Error::new("CORRUPT_STORE", "Capture CA key reference missing")
            })?)?;
            (pem, key)
        } else {
            let key = KeyPair::generate()
                .map_err(|_| Error::new("EXTERNAL_FAILURE", "Capture CA key generation failed"))?;
            let mut params = CertificateParams::new(vec![])
                .map_err(|_| Error::new("EXTERNAL_FAILURE", "Capture CA generation failed"))?;
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "ARTEX Workspace Capture CA");
            params.distinguished_name = dn;
            let certificate = params
                .self_signed(&key)
                .map_err(|_| Error::new("EXTERNAL_FAILURE", "Capture CA signing failed"))?;
            let pem = certificate.pem();
            let raw = key.serialize_pem();
            let reference = vault.set(
                &format!(
                    "capture-ca:{}",
                    hex::encode(Sha256::digest(directory.to_string_lossy().as_bytes()))
                ),
                "private-key",
                &raw,
            )?;
            atomic_write(
                &descriptor,
                &serde_json::to_vec(&json!({"certificate":pem,"key":reference}))?,
            )?;
            (pem, raw)
        };
        atomic_write(&certfile, pem.as_bytes())?;
        let key = KeyPair::from_pem(&key)
            .map_err(|_| Error::new("CORRUPT_STORE", "Invalid capture CA key"))?;
        let issuer = Issuer::from_ca_cert_pem(&pem, key)
            .map_err(|_| Error::new("CORRUPT_STORE", "Invalid capture CA certificate"))?;
        let ca = NativeAuthority::new(issuer)?;
        let spooldir = registry.resolve(cap, ".artex-host/traffic/spool", true)?;
        fs::create_dir_all(&spooldir).map_err(Error::io)?;
        let mut entries = BTreeMap::new();
        let mut total = 0u64;
        for entry in fs::read_dir(&spooldir).map_err(Error::io)? {
            let entry = entry.map_err(Error::io)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type().map_err(Error::io)?.is_dir() || !valid_id(&name) {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Unexpected capture spool entry",
                ));
            }
            let metadata = registry.resolve(
                cap,
                &format!(".artex-host/traffic/spool/{name}/metadata.json"),
                false,
            )?;
            if fs::metadata(&metadata).map_err(Error::io)?.len() > 131072 {
                return Err(Error::new("CORRUPT_STORE", "Oversized capture metadata"));
            }
            let mut data: Value = serde_json::from_slice(&fs::read(&metadata).map_err(Error::io)?)?;
            if data["id"] != name
                || !matches!(
                    data["state"].as_str(),
                    Some("capturing" | "completed" | "failed")
                )
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Capture identity or state mismatch",
                ));
            }
            let interrupted = data["state"] == "capturing";
            if interrupted {
                data["state"] = json!("failed");
                data["capture_error"] = json!("Capture was interrupted by native host restart");
            }
            for which in ["request", "response"] {
                let p = registry.resolve(
                    cap,
                    &format!(".artex-host/traffic/spool/{name}/{which}.bin"),
                    false,
                )?;
                let (length, digest) = sealed_body(&p)?;
                if !interrupted
                    && (data[format!("{which}_sha256")].as_str() != Some(digest.as_str())
                        || data[format!("{which}_captured_bytes")].as_str()
                            != Some(length.to_string().as_str()))
                {
                    return Err(Error::new(
                        "CORRUPT_STORE",
                        "Committed capture body changed",
                    ));
                }
                // Only capability-resolved paths enter engine ingestion. After a crash the
                // files, rather than stale pre-stream metadata, define captured bytes/hash.
                data[format!("{which}_file")] = json!(p);
                data[format!("{which}_captured_bytes")] = json!(length.to_string());
                data[format!("{which}_sha256")] = json!(digest);
                if interrupted {
                    let observed = data[format!("{which}_length")]
                        .as_str()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0)
                        .max(length);
                    data[format!("{which}_length")] = json!(observed.to_string());
                    data[if which == "request" {
                        "req_len"
                    } else {
                        "resp_len"
                    }] = json!(observed.to_string());
                }
                total = total
                    .checked_add(length)
                    .ok_or_else(|| Error::new("SIZE_LIMIT", "Capture spool size overflow"))?;
            }
            atomic_write(&metadata, &serde_json::to_vec(&data)?)?;
            entries.insert(name, data);
        }
        if entries.len() > 256 || total > DISK {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Capture spool requires engine ingestion before new capture",
            ));
        }
        let spool = Arc::new(Spool {
            directory: spooldir,
            entries: Mutex::new(entries),
            live: Mutex::new(HashMap::new()),
            bytes: AtomicU64::new(total),
        });
        *self.retained.lock().unwrap() = Some(spool.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let capture = Recorder {
            spool: spool.clone(),
            current: None,
            upstream: upstream.clone(),
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(Error::io(e)));
                    return;
                }
            };
            runtime.block_on(async move{let listener=match tokio::net::TcpListener::bind("127.0.0.1:0").await{Ok(v)=>v,Err(e)=>{let _=tx.send(Err(Error::io(e)));return;}};let address=listener.local_addr().unwrap();let proxy=Proxy::builder().with_listener(listener).with_ca(ca).with_rustls_connector(aws_lc_rs::default_provider()).with_http_handler(capture).build();let proxy=match proxy{Ok(p)=>p,Err(_)=>{let _=tx.send(Err(Error::new("EXTERNAL_FAILURE","Capture proxy initialization failed")));return;}};if tx.send(Ok(format!("http://{address}"))).is_err(){return;}tokio::select!{_ =proxy.start()=>{},_=async{while !stopping.load(Ordering::SeqCst){tokio::time::sleep(Duration::from_millis(20)).await;}}=>{}}});
        });
        let proxy_url = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(url)) => url,
            other => {
                stop.store(true, Ordering::SeqCst);
                let _ = worker.join();
                return Err(match other {
                    Ok(Err(e)) => e,
                    _ => Error::new("TIMEOUT", "Capture proxy initialization timed out"),
                });
            }
        };
        let active = Active {
            epoch: epoch.into(),
            proxy_url,
            ca_pem: pem,
            ca_path: certfile,
            upstream,
            stop,
            worker: Some(worker),
        };
        let result = info(&active);
        *self.active.lock().unwrap() = Some(active);
        Ok(result)
    }
    fn spool(&self) -> Result<Arc<Spool>> {
        self.retained
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::new("FEATURE_DISABLED", "No capture spool is open"))
    }
    pub fn poll(&self, limit: usize) -> Result<Value> {
        let spool = self.spool()?;
        let entries = spool.entries.lock().unwrap();
        let mut items = vec![];
        let mut budget = 0;
        for entry in entries.values().take(limit.clamp(1, 20)) {
            let size = serde_json::to_vec(entry)?.len();
            if size > 768000 {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Capture metadata exceeds frame budget",
                ));
            }
            if budget + size > 768000 {
                break;
            }
            budget += size;
            items.push(entry.clone());
        }
        Ok(json!({"exchanges":items}))
    }
    pub fn acknowledge(&self, ids: &[String]) -> Result<Value> {
        if ids.len() > 20 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Capture acknowledgement batch exceeded",
            ));
        }
        let spool = self.spool()?;
        let mut entries = spool.entries.lock().unwrap();
        let mut acknowledged = 0;
        for id in ids {
            if !valid_id(id) {
                return Err(Error::new("INVALID_PARAMS", "Invalid capture exchange id"));
            }
            if entries.contains_key(id) {
                let dir = spool.directory.join(id);
                let mut size = 0;
                for file in ["request.bin", "response.bin"] {
                    size += fs::metadata(dir.join(file)).map_err(Error::io)?.len();
                }
                fs::remove_dir_all(dir).map_err(Error::io)?;
                entries.remove(id);
                spool.bytes.fetch_sub(size, Ordering::SeqCst);
            }
            acknowledged += 1;
        }
        Ok(json!({"acknowledged":acknowledged}))
    }
    pub fn apply(&self, input: &mut Value) {
        let active = self.active.lock().unwrap();
        if let Some(a) = active.as_ref() {
            if !input["network_config"].is_object() {
                input["network_config"] = json!({});
            }
            input["network_config"]["proxy"] = json!(a.proxy_url);
            input["network_config"]["ca_pem"] = json!(a.ca_pem);
            input["native_environment"] = json!({"HTTP_PROXY":a.proxy_url,"HTTPS_PROXY":a.proxy_url,"http_proxy":a.proxy_url,"https_proxy":a.proxy_url,"SSL_CERT_FILE":a.ca_path,"REQUESTS_CA_BUNDLE":a.ca_path,"NODE_EXTRA_CA_CERTS":a.ca_path});
        } else {
            let proxy = self.direct_proxy.lock().unwrap();
            if !proxy.is_empty() {
                if !input["network_config"].is_object() {
                    input["network_config"] = json!({});
                }
                input["network_config"]["proxy"] = json!(*proxy);
                input["native_environment"] = json!({"HTTP_PROXY":*proxy,"HTTPS_PROXY":*proxy,"ALL_PROXY":*proxy,"http_proxy":*proxy,"https_proxy":*proxy,"all_proxy":*proxy});
            }
        }
    }
    pub fn stop_epoch(&self, epoch: &str) {
        let same = self
            .active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|a| a.epoch == epoch);
        if same {
            self.stop();
        }
    }
    pub fn stop(&self) {
        if let Some(mut a) = self.active.lock().unwrap().take() {
            a.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = a.worker.take() {
                let _ = worker.join();
            }
        }
    }
}
impl Drop for TrafficManager {
    fn drop(&mut self) {
        self.stop();
    }
}
fn info(a: &Active) -> Value {
    json!({"enabled":true,"proxy_url":a.proxy_url,"ca_pem":a.ca_pem,"ca_path":a.ca_path})
}
fn sealed_body(path: &std::path::Path) -> Result<(u64, String)> {
    let mut file = fs::File::open(path).map_err(Error::io)?;
    if file.metadata().map_err(Error::io)?.len() > BODY {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Capture body exceeds recovery budget",
        ));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    let mut length = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(Error::io)?;
        if n == 0 {
            break;
        }
        length += n as u64;
        if length > BODY {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Capture body grew beyond recovery budget",
            ));
        }
        hash.update(&buffer[..n]);
    }
    Ok((length, hex::encode(hash.finalize())))
}
fn valid_id(id: &str) -> bool {
    id.strip_prefix("traffic_")
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
#[derive(Clone)]
struct Recorder {
    spool: Arc<Spool>,
    current: Option<Arc<Mutex<Capture>>>,
    upstream: String,
}
impl HttpHandler for Recorder {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        request: Request<Body>,
    ) -> RequestOrResponse {
        if request.method() == hudsucker::hyper::Method::CONNECT {
            return request.into();
        }
        if self.spool.entries.lock().unwrap().len() + self.spool.live.lock().unwrap().len() >= 256
            || self.spool.bytes.load(Ordering::SeqCst) >= DISK
        {
            return Response::builder()
                .status(507)
                .body(Body::from(
                    "Capture spool is awaiting engine acknowledgement",
                ))
                .unwrap()
                .into();
        }
        if request.uri().to_string().len() > 16384 || headers(request.headers()).len() > 32768 {
            return Response::builder()
                .status(431)
                .body(Body::empty())
                .unwrap()
                .into();
        }
        let id = new_id("traffic");
        let dir = self.spool.directory.join(&id);
        if fs::create_dir(&dir).is_err() {
            return Response::builder()
                .status(507)
                .body(Body::empty())
                .unwrap()
                .into();
        }
        let reqfile = match fs::File::create(dir.join("request.bin")) {
            Ok(f) => f,
            Err(_) => {
                return Response::builder()
                    .status(507)
                    .body(Body::empty())
                    .unwrap()
                    .into();
            }
        };
        let respfile = match fs::File::create(dir.join("response.bin")) {
            Ok(f) => f,
            Err(_) => {
                return Response::builder()
                    .status(507)
                    .body(Body::empty())
                    .unwrap()
                    .into();
            }
        };
        drop(respfile);
        let url = request.uri().to_string();
        let head = format!(
            "{} {} {:?}\r\n{}\r\n",
            request.method(),
            request.uri(),
            request.version(),
            headers(request.headers())
        );
        let capture = Arc::new(Mutex::new(Capture {
            id: id.clone(),
            data: json!({"id":id,"state":"capturing","ts":artex_core::now(),"host":request.uri().host().unwrap_or(""),"method":request.method().as_str(),"url":url,"status":0,"content_type":"","request_head":head,"response_head":"","request_file":dir.join("request.bin"),"response_file":dir.join("response.bin"),"request_length":"0","response_length":"0","req_len":"0","resp_len":"0","request_captured_bytes":"0","response_captured_bytes":"0","capture_error":null}),
            req_done: false,
            resp_done: false,
            queued: false,
            bytes: 0,
            active_tees: 0,
            handler_done: false,
        }));
        if atomic_write(
            &dir.join("metadata.json"),
            &serde_json::to_vec(&capture.lock().unwrap().data).unwrap(),
        )
        .is_err()
        {
            return Response::builder()
                .status(507)
                .body(Body::empty())
                .unwrap()
                .into();
        }
        self.spool.live.lock().unwrap().insert(id, capture.clone());
        self.current = Some(capture.clone());
        let (parts, body) = request.into_parts();
        let tee = Tee::new(
            body,
            reqfile,
            capture,
            self.spool.clone(),
            true,
            parts
                .headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok()),
        );
        let request = Request::from_parts(parts, Body::from(tee.boxed()));
        self.forward(request).await.into()
    }
    async fn handle_response(
        &mut self,
        _ctx: &HttpContext,
        response: Response<Body>,
    ) -> Response<Body> {
        self.capture_response(response)
    }
    async fn handle_error(
        &mut self,
        _ctx: &HttpContext,
        _error: hudsucker::hyper_util::client::legacy::Error,
    ) -> Response<Body> {
        self.capture_response(Response::builder().status(502).body(Body::empty()).unwrap())
    }
}
impl Recorder {
    fn capture_response(&self, response: Response<Body>) -> Response<Body> {
        let Some(capture) = self.current.as_ref() else {
            return response;
        };
        let mut c = capture.lock().unwrap();
        c.data["status"] = json!(response.status().as_u16());
        c.data["content_type"] = json!(
            response
                .headers()
                .get("Content-Type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
        );
        let mut head = format!(
            "{:?} {}\r\n{}\r\n",
            response.version(),
            response.status(),
            headers(response.headers())
        );
        if head.len() > 32768 {
            let mut end = 32768;
            while !head.is_char_boundary(end) {
                end -= 1;
            }
            head.truncate(end);
            c.data["capture_error"] = json!("Response headers exceeded capture budget");
        }
        c.data["response_head"] = json!(head);
        let file = match fs::OpenOptions::new()
            .write(true)
            .open(self.spool.directory.join(&c.id).join("response.bin"))
        {
            Ok(f) => f,
            Err(_) => {
                c.data["capture_error"] = json!("Capture response file unavailable");
                return response;
            }
        };
        drop(c);
        let (parts, body) = response.into_parts();
        let expected = parts
            .headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok());
        Response::from_parts(
            parts,
            Body::from(
                Tee::new(
                    body,
                    file,
                    capture.clone(),
                    self.spool.clone(),
                    false,
                    expected,
                )
                .boxed(),
            ),
        )
    }
    async fn forward(&self, mut request: Request<Body>) -> Response<Body> {
        let is_upgrade = request.headers().contains_key("upgrade");
        let upgraded = if is_upgrade {
            Some(hudsucker::hyper::upgrade::on(&mut request))
        } else {
            None
        };
        let (method, uri, request_headers, body) = (
            request.method().clone(),
            request.uri().to_string(),
            request.headers().clone(),
            request.into_body(),
        );
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none());
        if !self.upstream.is_empty() {
            let proxy = match reqwest::Proxy::all(&self.upstream) {
                Ok(p) => p,
                Err(_) => {
                    return self.capture_response(
                        Response::builder().status(502).body(Body::empty()).unwrap(),
                    );
                }
            };
            builder = builder.proxy(proxy);
        }
        let client = match builder.build() {
            Ok(c) => c,
            Err(_) => {
                return self.capture_response(
                    Response::builder().status(502).body(Body::empty()).unwrap(),
                );
            }
        };
        let empty = body.is_end_stream();
        let mut outgoing = client.request(method, uri).headers(request_headers);
        if !empty {
            outgoing = outgoing.body(reqwest::Body::wrap_stream(body.into_data_stream()));
        } else {
            drop(body);
        }
        let response = outgoing.send().await;
        match response {
            Ok(response) => {
                let mut out = Response::builder().status(response.status());
                for (name, value) in response.headers() {
                    out = out.header(name, value);
                }
                if response.status().as_u16() == 101 {
                    if let Some(upgraded) = upgraded {
                        tokio::spawn(async move {
                            if let (Ok(client), Ok(mut target)) =
                                (upgraded.await, response.upgrade().await)
                            {
                                let mut client = hudsucker::hyper_util::rt::TokioIo::new(client);
                                let _ =
                                    tokio::io::copy_bidirectional(&mut client, &mut target).await;
                            }
                        });
                        return self.capture_response(out.body(Body::empty()).unwrap());
                    }
                    return self.capture_response(
                        Response::builder().status(502).body(Body::empty()).unwrap(),
                    );
                }
                let body =
                    Body::from_stream(response.bytes_stream().map_err(std::io::Error::other));
                self.capture_response(out.body(body).unwrap())
            }
            Err(_) => {
                self.capture_response(Response::builder().status(502).body(Body::empty()).unwrap())
            }
        }
    }
}
fn headers(headers: &hudsucker::hyper::HeaderMap) -> String {
    headers
        .iter()
        .map(|(name, value)| {
            format!(
                "{}: {}\r\n",
                name,
                value.to_str().unwrap_or("<binary-header>")
            )
        })
        .collect::<String>()
}
struct Tee {
    body: Body,
    file: fs::File,
    capture: Arc<Mutex<Capture>>,
    spool: Arc<Spool>,
    request: bool,
    length: u64,
    seen: u64,
    expected: Option<u64>,
    hash: Sha256,
    finished: bool,
}
impl Tee {
    fn new(
        body: Body,
        file: fs::File,
        capture: Arc<Mutex<Capture>>,
        spool: Arc<Spool>,
        request: bool,
        expected: Option<u64>,
    ) -> Self {
        let empty = body.is_end_stream();
        capture.lock().unwrap().active_tees += 1;
        let mut tee = Self {
            body,
            file,
            capture,
            spool,
            request,
            length: 0,
            seen: 0,
            expected,
            hash: Sha256::new(),
            finished: false,
        };
        if empty {
            tee.finish(None);
        }
        tee
    }
    fn finish(&mut self, error: Option<&str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let mut c = self.capture.lock().unwrap();
        let (which, lenkey) = if self.request {
            c.req_done = true;
            ("request", "req_len")
        } else {
            c.resp_done = true;
            ("response", "resp_len")
        };
        c.data[format!("{which}_length")] = json!(self.seen.to_string());
        c.data[lenkey] = json!(self.seen.to_string());
        c.data[format!("{which}_captured_bytes")] = json!(self.length.to_string());
        c.data[format!("{which}_sha256")] = json!(hex::encode(self.hash.clone().finalize()));
        if error.is_some() || self.file.sync_all().is_err() {
            c.data["capture_error"] = json!(error.unwrap_or("Capture flush failed"));
        }
        c.bytes += self.length;
    }
}
impl HttpBody for Tee {
    type Data = Bytes;
    type Error = hudsucker::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        ctx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, Self::Error>>> {
        match Pin::new(&mut self.body).poll_frame(ctx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(bytes) = frame.data_ref() {
                    let amount = bytes.len() as u64;
                    self.seen += amount;
                    let reserved = self
                        .spool
                        .bytes
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                            n.checked_add(amount).filter(|n| *n <= DISK)
                        })
                        .is_ok();
                    if !reserved
                        || self.length + amount > BODY
                        || self.file.write_all(bytes).is_err()
                    {
                        if reserved {
                            self.spool.bytes.fetch_sub(amount, Ordering::SeqCst);
                        }
                        self.finish(Some("Capture byte budget exceeded or disk write failed"));
                        return Poll::Ready(Some(Err(std::io::Error::other(
                            "Capture storage unavailable",
                        )
                        .into())));
                    }
                    self.length += amount;
                    self.hash.update(bytes);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(None) => {
                self.finish(None);
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                self.finish(Some("Capture stream failed"));
                Poll::Ready(Some(Err(error)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
impl Drop for Tee {
    fn drop(&mut self) {
        if !self.finished {
            let complete = self.body.is_end_stream() || self.expected == Some(self.seen);
            self.finish(if complete {
                None
            } else {
                Some("Capture stream interrupted")
            });
        }
        let mut capture = self.capture.lock().unwrap();
        capture.active_tees -= 1;
        publish(&mut capture, &self.spool);
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(capture) = self.current.as_ref() {
            let mut capture = capture.lock().unwrap();
            capture.handler_done = true;
            publish(&mut capture, &self.spool);
        }
    }
}
fn publish(c: &mut Capture, spool: &Spool) {
    if c.queued || !c.handler_done || c.active_tees != 0 {
        return;
    }
    if !c.req_done || !c.resp_done {
        c.data["capture_error"] =
            json!("Capture exchange interrupted before both streams completed");
    }
    c.data["state"] = json!(if c.data["capture_error"].is_null() {
        "completed"
    } else {
        "failed"
    });
    if let Ok(bytes) = serde_json::to_vec(&c.data) {
        if atomic_write(&spool.directory.join(&c.id).join("metadata.json"), &bytes).is_ok() {
            c.queued = true;
            spool
                .entries
                .lock()
                .unwrap()
                .insert(c.id.clone(), c.data.clone());
            spool.live.lock().unwrap().remove(&c.id);
        }
    }
}
