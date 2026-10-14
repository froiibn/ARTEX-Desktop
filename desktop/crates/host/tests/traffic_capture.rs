use artex_core::new_id;
use artex_host::{ExecutionRequest, HostRuntime, SecretVault};
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};
#[test]
fn interrupted_stream_recovery_seals_actual_bytes_before_ack() {
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let vault = SecretVault::default();
    let cap = host.capabilities.grant_directory(root.path()).unwrap();
    let epoch = new_id("epoch");
    let config = json!({"enabled":true,"global_proxy":"","directory_capability":cap});
    host.traffic
        .configure(&host.capabilities, &vault, &config, &epoch)
        .unwrap();
    host.traffic.stop();
    let id = new_id("traffic");
    let dir = root.path().join(".artex-host/traffic/spool").join(&id);
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("request.bin"), b"partial request").unwrap();
    fs::write(dir.join("response.bin"), b"partial response").unwrap();
    fs::write(dir.join("metadata.json"),serde_json::to_vec(&json!({"id":id,"state":"capturing","request_file":"outside-native-root","response_file":"outside-native-root","request_length":"0","response_length":"0","request_captured_bytes":"0","response_captured_bytes":"0"})).unwrap()).unwrap();
    host.traffic
        .configure(&host.capabilities, &vault, &config, &epoch)
        .unwrap();
    let poll = host.traffic.poll(20).unwrap();
    let e = &poll["exchanges"][0];
    assert_eq!(e["state"], "failed");
    assert_eq!(e["request_captured_bytes"], "15");
    assert_eq!(e["response_length"], "16");
    assert_eq!(
        e["response_sha256"],
        hex::encode(Sha256::digest(b"partial response"))
    );
    assert!(
        std::path::Path::new(e["request_file"].as_str().unwrap())
            .starts_with(dir.canonicalize().unwrap())
    );
    host.traffic.stop();
    host.traffic
        .configure(&host.capabilities, &vault, &config, &epoch)
        .unwrap();
    host.traffic.acknowledge(&[id]).unwrap();
    assert_eq!(host.traffic.poll(20).unwrap()["exchanges"], json!([]));
    host.traffic.stop();
    let descriptor: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join(".artex-host/traffic/ca.json")).unwrap())
            .unwrap();
    vault
        .delete(descriptor["key"]["secret_ref"].as_str().unwrap())
        .unwrap();
}
#[test]
fn actual_http_and_https_capture_is_durable_until_ack() {
    let d = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let vault = SecretVault::default();
    let cap = host.capabilities.grant_directory(d.path()).unwrap();
    let epoch = new_id("epoch");
    let mut config = host
        .traffic
        .configure(
            &host.capabilities,
            &vault,
            &json!({"enabled":true,"global_proxy":"","directory_capability":cap}),
            &epoch,
        )
        .unwrap();
    for tls in [false, true] {
        let key = hudsucker::rcgen::KeyPair::generate().unwrap();
        let params =
            hudsucker::rcgen::CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()])
                .unwrap();
        let cert = params.self_signed(&key).unwrap();
        fs::write(d.path().join("target.pem"), cert.pem()).unwrap();
        fs::write(d.path().join("target-key.pem"), key.serialize_pem()).unwrap();
        let execution = new_id("exec");
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/capture_http.py");
        let mut args = vec![
            "-u".into(),"-c".into(),
            // Remove the wrapper's fixture-path argument before runpy, so the
            // real script keeps its certificate/key CLI for the HTTPS case.
            "import runpy,socket,sys; socket.getfqdn=lambda *args: (_ for _ in ()).throw(RuntimeError('Loopback fixture must not resolve a host name')); fixture=sys.argv.pop(1); runpy.run_path(fixture,run_name='__main__')".into(),
            fixture.to_string_lossy().into_owned(),
        ];
        if tls {
            args.extend([
                d.path().join("target.pem").to_string_lossy().into_owned(),
                d.path()
                    .join("target-key.pem")
                    .to_string_lossy()
                    .into_owned(),
            ]);
        }
        host.executions
            .execute(ExecutionRequest {
                execution_id: execution.clone(),
                run_id: "fixture".into(),
                engine_epoch: epoch.clone(),
                executable: if cfg!(windows) {
                    "python.exe".into()
                } else {
                    "python3".into()
                },
                args,
                cwd: d.path().into(),
                // Fixture lifetime covers independent startup, HTTP, capture
                // and upgrade phases. Their individual deadlines remain below.
                timeout_ms: 60000,
            })
            .unwrap();
        // Python/import/TLS readiness is not an HTTP/capture latency budget.
        let startup_deadline = Instant::now() + Duration::from_secs(15);
        let port = loop {
            let status = host
                .executions
                .status(&execution, "fixture", &epoch)
                .unwrap();
            if let Some(port) = status
                .output
                .lines()
                .find_map(|s| s.trim().parse::<u16>().ok().filter(|p| *p != 0))
            {
                break port;
            }
            assert_eq!(
                status.state, "running",
                "Capture fixture exited before readiness (tls={tls}): {status:?}"
            );
            assert!(
                Instant::now() < startup_deadline,
                "Capture fixture did not become ready (tls={tls}): {status:?}"
            );
            thread::sleep(Duration::from_millis(10));
        };
        let ca =
            reqwest::Certificate::from_pem(config["ca_pem"].as_str().unwrap().as_bytes()).unwrap();
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .proxy(reqwest::Proxy::all(config["proxy_url"].as_str().unwrap()).unwrap())
            .add_root_certificate(ca)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let response = client
            .post(format!(
                "{}://127.0.0.1:{port}/capture",
                if tls { "https" } else { "http" }
            ))
            .body("actual request")
            .send()
            .unwrap();
        assert_eq!(response.status().as_u16(), 201);
        assert_eq!(response.text().unwrap(), "actual request");
        let deadline = Instant::now() + Duration::from_secs(5);
        let exchange = loop {
            let p = host.traffic.poll(20).unwrap();
            if let Some(e) = p["exchanges"].as_array().unwrap().iter().find(|e| {
                e["url"].as_str().unwrap().contains(&format!(":{port}")) && e["method"] == "POST"
            }) {
                break e.clone();
            }
            assert!(Instant::now() < deadline, "Capture not finalized: {p}");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(exchange["state"], "completed", "{exchange}");
        assert_eq!(exchange["request_length"], "14");
        assert_eq!(exchange["response_captured_bytes"], "14");
        let file = exchange["response_file"].as_str().unwrap();
        assert_eq!(fs::read(file).unwrap(), b"actual request");
        assert!(
            host.traffic.poll(20).unwrap()["exchanges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == exchange["id"])
        );
        if !tls {
            let upstream_dir = tempfile::tempdir().unwrap();
            let upstream = artex_host::traffic::TrafficManager::default();
            let upstream_cap = host
                .capabilities
                .grant_directory(upstream_dir.path())
                .unwrap();
            let upstream_config = upstream
                .configure(
                    &host.capabilities,
                    &vault,
                    &json!({"enabled":true,"global_proxy":"","directory_capability":upstream_cap}),
                    &epoch,
                )
                .unwrap();
            for upstream_url in ["", upstream_config["proxy_url"].as_str().unwrap()] {
                config=host.traffic.configure(&host.capabilities,&vault,&json!({"enabled":true,"global_proxy":upstream_url,"directory_capability":cap}),&epoch).unwrap();
                let proxy = reqwest::Url::parse(config["proxy_url"].as_str().unwrap()).unwrap();
                let mut stream =
                    TcpStream::connect((proxy.host_str().unwrap(), proxy.port().unwrap())).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                write!(stream,"GET http://127.0.0.1:{port}/upgrade HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: Upgrade\r\nUpgrade: fixture\r\n\r\n").unwrap();
                let mut head = vec![];
                while !head.ends_with(b"\r\n\r\n") {
                    let mut b = [0];
                    stream.read_exact(&mut b).unwrap();
                    head.extend_from_slice(&b);
                }
                assert!(String::from_utf8_lossy(&head).contains("101"));
                stream.write_all(b"hello").unwrap();
                let mut echoed = [0; 5];
                stream.read_exact(&mut echoed).unwrap();
                assert_eq!(&echoed, b"hello");
                drop(stream);
            }
            upstream.stop();
            let descriptor: serde_json::Value = serde_json::from_slice(
                &fs::read(upstream_dir.path().join(".artex-host/traffic/ca.json")).unwrap(),
            )
            .unwrap();
            vault
                .delete(descriptor["key"]["secret_ref"].as_str().unwrap())
                .unwrap();
            config = host
                .traffic
                .configure(
                    &host.capabilities,
                    &vault,
                    &json!({"enabled":true,"global_proxy":"","directory_capability":cap}),
                    &epoch,
                )
                .unwrap();
        }
        host.traffic
            .acknowledge(&[exchange["id"].as_str().unwrap().to_string()])
            .unwrap();
        assert!(!std::path::Path::new(file).exists());
        host.executions.stop(&execution, "fixture", &epoch).unwrap();
    }
    host.traffic
        .configure(
            &host.capabilities,
            &vault,
            &json!({"enabled":false,"global_proxy":""}),
            &epoch,
        )
        .unwrap();
    let remaining = host.traffic.poll(20).unwrap();
    let ids = remaining["exchanges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    host.traffic.acknowledge(&ids).unwrap();
    assert_eq!(host.traffic.poll(20).unwrap()["exchanges"], json!([]));
    // Only the public certificate and its opaque key reference live on disk.
    let descriptor = fs::read_to_string(d.path().join(".artex-host/traffic/ca.json")).unwrap();
    let secret: serde_json::Value = serde_json::from_str(&descriptor).unwrap();
    assert!(secret["key"]["secret_ref"].is_string());
    assert!(!descriptor.contains("PRIVATE KEY"));
    vault
        .delete(secret["key"]["secret_ref"].as_str().unwrap())
        .unwrap();
}
