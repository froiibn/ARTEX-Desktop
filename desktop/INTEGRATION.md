# Implementation integration contract

The original Go/web production code is frozen input. Implementation lives under D:/Artex/rebuilt/desktop. The separate original checkout is D:/Artex/original; it retains its existing localized edits and Git history. Current native UI/service entry points are desktop/native and desktop/native-core. Former app/web prototypes and the engine sidecar have been removed from the current source tree; recover historical sources from the pre-cleanup Git commit if needed. Canonical docs/contracts and desktop/contracts are updated together for reviewed desktop additions. Do not change source manifests to hide original-source drift. Common APIs below are the integration boundary.

artex-core public API:
- Error { code:String, message:String, retryable:bool, recovery_required:bool, details:serde_json::Value }, Error::new(code,&str), Error::io(std::io::Error); type Result<T>.
- Header { id,collection,version,commit_seq,created_at,updated_at:String, legacy_key:Value }; Record { header:Header,data:Value }.
- Expected { collection,id,version:String }; Change enum Put {collection:String,id:String,data:Value}, Delete {collection:String,id:String}.
- CommandBatch { command_id,method:String, normalized_params:Value, expected:Vec<Expected>, changes:Vec<Change>, events:Vec<Record>, deliveries:Vec<Record>, result:Value }.
- Receipt { command_id,commit_seq:String,result:Value }.
- Snapshot { workspace_id:String,commit_seq:u64,collections:BTreeMap<String,BTreeMap<String,Record>> }; Snapshot::get(collection,id)->Option<&Record>; Snapshot::records(collection)->Vec<&Record>.
- Actor { kind:String,agent_id:Option<String>,run_id:Option<String>,engine_epoch:String }; Context { actor:Actor,task_id:Option<String>,intent_id:Option<String>,conversation_id:Option<String> }.
- new_id(prefix:&str)->String; now()->String; body(text:&str)->Value; parse_u64(&str)->Result<u64>; contracts::read(name:&str)->Value; validate_record(&Record)->Result<()>; default_data(collection:&str)->Result<Value> supplies complete nullable/default fields (no arbitrary omitted required fields).

artex-store public API:
- Store::create(path:&Path,name:&str)->Result<Store>; Store::open(path:&Path)->Result<Store>; snapshot()->Snapshot; commit(&mut self,CommandBatch)->Result<Receipt>; checkpoint(&mut self)->Result<Value>; info()->Value; read_only()->bool; blob_put(&mut self,bytes:&[u8],media_type:&str)->Result<Value>; blob_read(&self,reference:&Value,offset:u64,length:usize)->Result<Vec<u8>>. OS lock remains held for Store lifetime.
- read_generation()->Arc<ReadGeneration> pins an immutable generation and its blob dependencies; read_view gives workers read-only access without writer mutation. Large files use blob_put_file_cancellable/copy_blob_to with bounded chunks. Background checkpoint and GC cannot delete pinned dependencies.
- Source-preserving migration is in crates/importer; the separate crates/migrate CLI performs staged offline imports and stores secrets through the native OS vault.

artex-domain:
- plan(method:&str,params:&Value,state:&Snapshot,context:&Context)->Result<CommandBatch> for mutation; query(method:&str,params:&Value,state:&Snapshot,context:&Context)->Result<Value>; tools executable dispatch has explicit capability/coverage reporting. Prepared batches receive RPC command_id+expected from caller (plan can use temporary blank command id).

Root coordinates Cargo.toml, crates/agent, crates/rpc, documentation and integration verification. Storage work covers crates/core and crates/store. Domain work covers crates/domain and crates/tools. Host work covers crates/host; native UI work covers native and native-core. Coordinate shared edits before changing another area. Each crate package inherits workspace metadata. Own unsafe forbidden in core/store/domain; OS boundary isolated.

Do not call undefined functions successful, fake external output or pass a registration count off as implemented coverage. Feature not executable must fail explicit FEATURE_DISABLED and be listed pending. Add substantive tests of invariants, source behavior and fault recovery. Report APIs promptly if incompatibility necessary. User requested independent developer agents, coordination, rejection/fix cycle, and final parity audit.

Runtime integration:
- Provider generate_streaming emits bounded text/thinking/usage progress. Tool arguments become executable only after the complete Turn validates. The writer stores partial activities, then atomically replaces them with the verified provider message; ActivityReplaced events preserve deleted cursor continuity across restart.
- Display-only partials never enter replay history. Failed/cancelled progress remains non-replayable and cannot trigger intent retries after committed provider output.
- Native services use the in-process bridge and capability-checked direct tool execution. No WebView or engine-sidecar IPC is exposed. Agent messaging and file transfer contracts remain part of the shared Rust model.
- workspace.close/system.shutdown pause tasks, account the final active-clock fraction, mark unknown effects recovery_required, checkpoint, and release the writer. Shutdown errors propagate; the host uses bounded natural child exit with a kill fallback.
- Source trace/object references use public activity projections. Thinking/signatures/usage and provider raw-message blobs cannot leak through grouped transcript bodies.
