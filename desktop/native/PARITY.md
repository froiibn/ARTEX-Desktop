# Native host parity boundaries

The 2026-10-14 native lifecycle additions preserve the original 81-tool registry
and 54-table/571-field SQL-derived shape. Optional conversation trash fields
preserve absent, explicit null and present values through generated typed models
without changing legacy DTOs. Original lifecycle deletion entry points now use
the same scope/version and real cleanup coordinator as the typed native UI;
legacy return shapes remain unchanged. Files retired from active execution
directories remain in a recovery quarantine. No secure erase or removal of
historical journals/backups is claimed. Unknown earlier execution cleanup can
block destructive removal; metadata trash and restore remain available.

The active native runtime invokes Rust libraries in the application process.
The former React/Tauri application, framed engine client and their dedicated
tests/build scripts have been removed from the current source tree. Native
HostBridge execution and the current OS/process/tool boundaries are retained.

## Files and exports

Native file editors pin SHA-256 revisions. A changed file requires an explicit
reload and draft reconciliation. Native save commands use durable receipts;
retrying a completed command does not overwrite a later external modification.
This is optimistic conflict detection: unrelated Bash and third-party programs
cannot be forced to participate in the native editor lock.

Native workspace downloads return immutable file capabilities. The GUI owns
destination selection. Read-only export receipts and native log spools are kept
under the configured app profile outside the source workspace. Export into the
read-only source workspace is refused. Staged files have a 512 MiB aggregate
budget and a 32-token limit; retiring their gateway clears its temporary files.

Task and conversation attachments use their corresponding scoped working
directory. Admitted shared legacy/staging attachments are copied into that scope
only if the scoped file is absent.

Unlike the original worker's per-intent run directory, the accepted desktop
implementation specification uses a shared working directory per task (and a
separate directory per conversation). Intent/run objects remain distinct in the
object graph. Agents in the same task intentionally share files and persisted
information; run-specific output filenames distinguish their artifacts. The
code-owned prompt states this layout and does not claim intent-exclusive folders.
See `docs/desktop-implementation-spec.md` for the accepted directory contract.

## Diagnostics

Native host connection, tool completion and operation error codes enter a bounded
durable native log spool. The existing kernel `logs.poll` / object-graph commit /
`logs.ack` flow persists these diagnostics before acknowledgment. The native UI
also displays command and kernel errors directly.

There is no separate engine child's stderr stream in this architecture. Arbitrary
Rust library `eprintln!` output is **not** redirected into the log spool. Additional
provider/kernel library diagnostics need an explicit native logging callback;
process-wide stderr redirection has not been implemented. Tool process output
continues to belong to the existing bounded supervisor output/artifact APIs and
is not represented as an automatically captured engine stderr stream.

## Qualification

Windows power notification registration and injected suspend/checkpoint behavior
have native fixture coverage. macOS IOKit and Linux login1 adapters are present;
their physical-platform qualification is still required. Clock-gap recovery is a
conservative fallback and does not claim to complete a checkpoint before sleep.
Concrete passing runs and package evidence are recorded by the final verifier.
Native tray support uses Win32/AppKit on the GUI thread and Linux KSNI on its
worker. Linux requires an actual registered StatusNotifier host; watcher failure
restores visibility and retains normal window-close behavior. Single-instance
launch intents use a bounded profile mailbox with session and read-only-mode
fences. Neither mechanism creates another engine process. Windows, macOS and
Arch physical-platform release qualification remains incomplete; successful
compile or synthetic-event tests alone do not certify it.
