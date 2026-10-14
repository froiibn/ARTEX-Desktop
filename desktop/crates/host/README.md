# Native OS boundary

`HostBridge::execute_direct` connects the native kernel to OS operations in the
same application process. The kernel supplies the workspace and actor context;
the UI cannot override ownership. Directory capabilities reject traversal and
link escapes. The former engine sidecar client and WebView transport are removed.

`ExecutionSupervisor` owns external tool processes: Windows children start
suspended, join a kill-on-close Job, and then resume; Unix uses process groups.
Output is retained in a bounded 64 KiB ring with byte cursors. Execution IDs are
deduplicated and status/stop/list calls check run and epoch. `stop_run` interrupts
processes and sleep, and forced termination confirms cleanup where possible.
PTY shells and MCP stdio servers remain supervised external tool processes.

`HostRuntime` implements file and editor operations, Bash, task output/stop/list,
sleep, PTY, Glob/Grep, network access, scoped skills, MCP transports, traffic
capture, notifications, and native logs. Native management requests retain actor
validation, scoped paths, and effect receipts. Large native downloads return a
staged file capability for the UI to save through its own file picker.

`SecretVault` uses Windows Credential Manager, macOS Keychain, or Linux Secret
Service. It does not write plaintext secret files. `ProfileInstanceLock` holds
an OS advisory lock for the application lifetime and supports second-launch
handoff. Suspend/resume monitoring and migration integrations remain available.

From the `desktop` directory, run `cargo test --locked -p artex-host`. The former
sidecar build is unnecessary. Network integration tests use local fixtures and
isolated temporary directories; they do not require PostgreSQL or an external
application server. `--no-default-features` remains accepted for existing native
verification scripts.
