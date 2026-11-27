//! Test-only loopback transport and memory-secret boundary for actual App tests.
//! Intended child module of app_workflow_e2e_tests, named `transport`.
//! All provider listeners are loopback; only explicit fixture SecretRefs resolve.
use artex_core::{Context, Error, Result};
use artex_host::SecretVault;
use artex_native_core::{HostFactory, gateway::NativeGateway};
use artex_runtime::HostGateway;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::{Read, Write}, net::{TcpListener, TcpStream},
    path::Path, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    thread::{self, JoinHandle}, time::{Duration, Instant}};

#[derive(Clone, Default)]
pub(super) struct Probe {
    // Count before dispatch: a failing host attempt must not look like no call.
    pub(super) attempts: Arc<Mutex<Vec<Value>>>,
    pub(super) results: Arc<Mutex<Vec<Value>>>,
}
pub(super) struct MemoryGateway {
    native: NativeGateway,
    secrets: BTreeMap<String, String>,
    probe: Probe,
}
impl HostGateway for MemoryGateway {
    fn request(&self, workspace: &str, context: &Context, operation: &str,
               input: Value, timeout: Duration) -> Result<Value> {
        // Even an unknown secret operation must never fall through to OS vault.
        if operation.starts_with("secret.") {
            // User model probes use a separate operation from scheduled runs.
            // Admit only that actor and our explicit fixture references; never
            // fall back to the OS vault even for an unknown reference.
            return if operation == "secret.resolve"
                || (operation == "secret.resolve_user" && context.actor.kind == "user") {
                input["secret_ref"].as_str().and_then(|id| self.secrets.get(id))
                    .map(|value| json!({"value":value}))
                    .ok_or_else(|| Error::new("PERMISSION_DENIED", "Fixture secret is not admitted"))
            } else { Err(Error::new("PERMISSION_DENIED", "Fixture secret operation is not admitted")) };
        }
        let observed = json!({"workspace":workspace,"operation":operation,"input":input});
        if operation == "tool.execute" {
            self.probe.attempts.lock().unwrap().push(observed.clone());
        }
        let result = self.native.request(workspace, context, operation, input, timeout);
        if operation == "tool.execute" {
            // Preserve errors without using `?` before recording the outcome.
            let outcome = match &result {
                Ok(value) => json!({"request":observed,"ok":value}),
                Err(error) => json!({"request":observed,"error":format!("{error:?}")}),
            };
            self.probe.results.lock().unwrap().push(outcome);
        }
        result
    }
    fn retire(&self, timeout:Duration)->Result<()> { self.native.retire(timeout) }
    fn retirement_report(&self,workspace:&str,contexts:&[Context])->Result<Option<Value>> {
        self.native.retirement_report(workspace,contexts)
    }
}
pub(super) fn factory(profile:&Path,secrets:BTreeMap<String,String>,probe:Probe)->HostFactory {
    let profile=profile.to_owned();
    // Construction only: this wrapper intercepts/rejects every secret operation.
    let vault=Arc::new(SecretVault::default());
    Arc::new(move |workspace| Ok(Arc::new(MemoryGateway {
        native:NativeGateway::open_with_profile(workspace,vault.clone(),&profile)?,
        secrets:secrets.clone(),probe:probe.clone(),
    })))
}
#[derive(Clone)]
pub(super) enum Step { Reply(u16,&'static str,String), Hold }
pub(super) struct LocalProvider {
    pub(super) url:String,
    seen:Arc<Mutex<Vec<Value>>>,
    stopped:Arc<AtomicBool>,
    worker:Option<JoinHandle<()>>,
}
impl LocalProvider {
    pub(super) fn new(steps:Vec<Step>)->Self {
        Self::from_handler(move |index,_| steps.get(index).expect("Unexpected extra provider request").clone())
    }
    // Dynamic replies may parse an actual host-created ShellId from request 2.
    // This closure only controls fixture HTTP responses, never product results.
    pub(super) fn from_handler(mut response:impl FnMut(usize,&Value)->Step+Send+'static)->Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url=format!("http://{}/v1",listener.local_addr().unwrap());
        let seen=Arc::new(Mutex::new(Vec::new()));let captured=seen.clone();
        let stopped=Arc::new(AtomicBool::new(false));let stop=stopped.clone();
        let worker=thread::spawn(move || {
            let end=Instant::now()+Duration::from_secs(90);let mut held=Vec::new();
            while !stop.load(Ordering::Acquire) {
                assert!(Instant::now()<end,"Local fixture exceeded serving bound");
                match listener.accept() {
                    Ok((mut socket,_))=> {
                        socket.set_nonblocking(false).unwrap();
                        socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                        socket.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
                        let request=read_request(&mut socket);
                        let index={let mut items=captured.lock().unwrap();let index=items.len();items.push(request.clone());index};
                        match response(index,&request) {
                            Step::Hold=>held.push(socket),
                            Step::Reply(status,media,body)=> {
                                write!(socket,"HTTP/1.1 {status} Fixture\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                            }
                        }
                    }
                    Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>thread::sleep(Duration::from_millis(5)),
                    Err(e)=>panic!("{e}"),
                }
            }
            drop(held);
        });
        Self{url,seen,stopped,worker:Some(worker)}
    }
    pub(super) fn requests(&self)->Vec<Value> { self.seen.lock().unwrap().clone() }
    pub(super) fn finish(mut self)->Vec<Value> {
        self.stopped.store(true,Ordering::Release);
        self.worker.take().unwrap().join().unwrap();self.requests()
    }
}
impl Drop for LocalProvider {
    fn drop(&mut self) {
        self.stopped.store(true,Ordering::Release);
        if let Some(worker)=self.worker.take() { let _=worker.join(); }
    }
}
fn read_request(socket:&mut TcpStream)->Value {
    let mut header=vec![];let mut byte=[0];
    while !header.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).unwrap();header.push(byte[0]);assert!(header.len()<16384);
    }
    let header=String::from_utf8(header).unwrap();
    let length=header.lines().find_map(|line|line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|s|s.trim().parse::<usize>().ok())).unwrap();
    assert!(length<2*1024*1024);let mut body=vec![0;length];
    socket.read_exact(&mut body).unwrap();serde_json::from_slice(&body).unwrap()
}
pub(super) fn anthropic(text:&str,capped:bool)->String {
    json!({"content":[{"type":"text","text":text}],"stop_reason":if capped{"max_tokens"}else{"end_turn"},"usage":{"input_tokens":3,"cache_read_input_tokens":17,"cache_creation_input_tokens":5,"output_tokens":2}}).to_string()
}
pub(super) fn responses(output:Option<Value>,first:bool)->String {
    let mut terminal=json!({"status":"completed","usage":{"input_tokens":9,"output_tokens":4}});
    if let Some(output)=output { terminal["output"]=output; }
    let mut events=vec![("response.output_text.delta",json!({"output_index":0,"delta":if first{"RESPONSES_OBSERVED_SENTINEL"}else{"RESPONSES_FINAL_SENTINEL"}}))];
    if first {
        events.push(("response.output_item.added",json!({"output_index":1,"item":{"type":"function_call","call_id":"responses-shell-list","name":"shell_list","arguments":""}})));
        events.push(("response.function_call_arguments.delta",json!({"output_index":1,"delta":""})));
    }
    events.push(("response.completed",json!({"response":terminal})));
    events.into_iter().map(|(name,value)|format!("event: {name}\ndata: {value}\n\n")).collect()
}
