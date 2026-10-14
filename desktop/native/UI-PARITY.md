# Native UI source comparison

The 2026-10-14 folder separation moves the original Go/Web checkout to
`D:/Artex/original` and the independent Rust repository to `D:/Artex/rebuilt`.
Production Rust source, compiled release bytes and contracts are unchanged.
Fresh native verification at the new location is
`test-results/7fe0d9ab6b75451a85b0d88c4d108bc4`: 24 service, 56 host, 96 UI and
339 shared tests, build and integrated/window/close/second-launch checks pass.
The final run uses `RUST_TEST_THREADS=1`; two preceding parallel OS test failures
are retained and described in STATUS.md. No test assertions were weakened.
Old absolute paths in prior JSON reports are preserved as historical evidence.

The conversation/task lifecycle and owned execution management revision is
verified in `test-results/beb90167125c43c0b3b9ba062c1fc52c`: 96 UI tests,
actual integrated provider/tool/lifecycle persistence checks, Windows 23-section
window/close/second-launch checks, and the exact unchanged 168 original management
signatures and 23 section identities. Shared input hashes (168 files) match the
fresh full regression in `f6216fce1d56491a894e630ea3e71d7a` (24 service, 56 host,
339 shared tests; overlapping groups are not summed). New typed monitor, stop,
force-stop, conversation clear/trash/restore/purge and task removal operations
supplement that original registry. Scoped history tokens reject reads issued
before a successful clear/removal; another conversation's cache/draft and run
stop history remain intact. Preview labels count scoped directory candidates,
not filesystem files. Physical backup/WAL erasure, live stdout and true worker
heartbeat are not claimed. Actual Mac/Arch builds remain unverified.

Twenty-four real Windows GPU captures were reviewed and accepted by root in
`visual-review.json`, including normal/minimum execution cards, bounded deletion
previews, trash/archive and the preserved chat drawers. The earlier
`c6f48f879dc64af5bc5da1c53a226380` rendering was rejected for monitor row/button
layout and a clipped task deletion footer, repaired, then recaptured. No rejected
visual build was packaged. Captured-frame query evidence is retained separately
from orderly-close retirement of observation caches.

The integrated window-frame revision is verified in
`test-results/ccdd6e9861a242cbbf01e6a49c93878c`: 82 UI tests, fresh
native service/host/shared regression runs, actual integrated service and
Windows window/second-launch checks, and the unchanged 168 management action
signatures and 23 sections. The 40px production titlebar replaces the menu
row; application close retains the existing lifecycle. Six real egui chrome
tests cover controls, dragging, double click, disjoint menu hit regions and
eight initial resize gestures. The actual minimum-window composer fixture
renders this same titlebar. Mac Command+Enter preserves draft/CAS/attachment
ownership. Unsupported macOS BeginResize is avoided by retaining a transparent
native resizable window configuration. Non-UTF8 file admission is rejected
before JSON serialization; its Unix regression is not run on Windows.
Three target dependency graphs resolve, but Mac/Arch compilation and native
hardware qualification remain unverified. See `docs/desktop-platform-audit.md`
for remaining Unix process containment, suspend and distribution limitations.
Fifteen actual Windows GPU captures were reviewed and accepted by root in
`visual-review.json`, including normal/minimum chat with each drawer tab and
minimum model/settings/task views. The requested native decorations flag and
deferred application close configuration are recorded in each window report.

The full-width bottom drawer revision is verified in
`test-results/25fbc16be5a2465db4e1ceda752e3649`: 75 UI tests,
actual integrated service and Windows window checks, the exact unchanged 168
management signatures and 23 section identities, and fifteen GPU captures
accepted by root in `visual-review.json`. Activity and chat Management share
one initially closed drawer. Chat no longer reserves a second management panel;
other sections retain their original inline management. Expanded management
uses the same draft/token/CAS/secret/async paths. Production toolbar/drawer
renderers are shared by minimum-window geometry tests with five attachments
and pending feedback. Width never shrinks to short log content, and actual
capture body allocation is checked at approximately 60 logical pixels minimum.
The 74-test intermediate build was rejected after visual review and repaired;
it was not selected for distribution. Provider, storage and 81 tool contracts
are unchanged; physical macOS/Arch Linux qualification remains outstanding.

The 2026-10-13 usability fixes are verified in `test-results/36ee0641756d44ad9b42ce395b1dd5e3`: 69 UI tests, 23 native sections, and the exact unchanged set of 168 management action signatures. Eleven GPU captures cover normal and minimum-window chat, model, settings and task views plus approvals, sessions and files. Compact list preferences, local timestamps with offsets, readable approval/session names, grouped forms and state-dependent controls preserve the existing object identities, drafts, CAS vectors and provider/tool contracts. Read-only tests use production flag injection and retain scoped body/history queries. This does not certify physical macOS/Arch Linux execution or all user interactions with a live model.

This is an implementation inventory, not a claim that 23 navigation entries
replace every interaction in the original web application. The UI uses Rust
egui widgets and the in-process typed service. The original Go and web sources
are reference material and are not modified by this migration.

The composer placement correction is verified in
`test-results/468b24e115c94fcf9e5169cc1f224888/verification-result.json`
(54 UI tests and actual service/Windows checks), its `ui-parity.json` (exact
unchanged action/section identities), and `ui-review` GPU captures. The editor
renders once, reuses its owner's actual measured height across width changes,
and anchors to the pane bottom. The transcript occupies the remaining height.
Two real-frame geometry regressions cover empty/populated history, attachment
and resize changes, including the surrounding ScrollArea and scrollbar width
transition that left a gap in the initial minimum-window capture. Drafts,
attachments, command ownership and the original tests remain preserved.

The approved A+C+B visual revision uses the soft Linux palette and rounded
controls, a wider conversation with section-local collapsed details by default,
and existing task tabs plus agent-run overview/approval and activity-history
tabs. Message metadata and expandable tool/audit values remain available. The
composer groups draft text, scoped attachments, reference insertion and sending
inside one surface. Compact layout reduces input rows and attachment viewport
height while keeping their complete contents scrollable. Selection and input
focus use a tinted background with a distinct accent outline; primary actions
keep their separate saturated background. Source action catalog entries and
backend contracts are unchanged by this visual revision.

Final visual revision evidence is
`test-results/7610baa914f14252b6a828a7e00e3bb4/verification-result.json`
(52 UI tests and actual service/Windows checks), `ui-parity.json` (exact 168
action signatures and 23 section names), and
`ui-review/capture-review-result.json` (eight GPU captures including minimum
900×620 conversation). The packaged release separately passed service/tool
execution, 23-screen rendering and orderly shutdown. The initial four UI test
failures were repaired in implementation, preserving existing assertions.

The October 13 workspace approval/UI revision retains the 23 native sections,
81 tool definitions/executor mappings and 168 existing management action
identities. The left navigation remains in place. Main panels use aligned
label/input rows, wrapping action bars and full-width scrolling; narrow windows
adapt their layout, and long approval input paths wrap without truncating values.
Draft revisions, secret references and request ownership remain pinned.

The workspace toolbar adds persisted **Bypass 켜기/끄기** through the existing
`ui.settings` / `ui.setSettings` contract with an optional boolean. This skips
internal Ask/model review but preserves explicit deny, plan-write and scope/path
restrictions, ordinary questions and plan approval. Exact committed manual
approval is reused for the matching permission step; background queries no
longer precede admitted interaction commands. Pending feedback disables duplicate
actions and completion is attached to its original request.

Evidence: `test-results/576264b2d5e0459bb02d1bba21dd2c3e/verification-result.json`
(47 native UI tests, 165 unchanged shared source hashes against the full passing
regression), `ui-parity.json` (unchanged action identities), and
`ui-review/capture-review-result.json` (five actual Windows GPU captures).
The packaged executable separately passed real manual/bypass Read execution,
persisted reopening and 23-screen orderly-shutdown checks; these do not certify
every original web interaction or physical macOS/Arch deployment.

## Flows connected in native widgets

| Native area | Source contract / native service | Current behavior |
|---|---|---|
| Models | `ui.saveLLMProfile`, `ui.activateLLMProfile`, `ui.deleteLLMProfile` | Scalar fields, format/thinking choices, retries, profile activation, OS-vault references. Existing credentials remain references. |
| Agents | `ui.createAgent`, `ui.updateAgent`, `ui.saveAgentConfig`, prompt/wrapup/reset APIs, `ui.setAgentVisibility`, `ui.saveTool`, trigger CRUD | Prompt/history including verified Blob reads, metadata, bindings, MCP/skill visibility, trigger condition lists. Archived prompts can be copied into a new editable version. |
| Tools / MCP | Tool CRUD/reset and MCP CRUD/refresh APIs | Structured nested parameter-schema editor, editable execution/header/environment lists, typed agent choices, immutable revision vector, separate secret sealing action. |
| Skills | Source create/delete/upload/file-list APIs; typed `SkillMetaRead/Write` and Resources `FileRead/Write` | Metadata/body edits pin the actual file SHA. Existing metadata preserves unknown YAML/body in the host. The Skills ZIP picker stages an opaque file capability and defaults to no overwrite; explicit overwrite applies to matching names in the ZIP. File listings can be refreshed without rebasing metadata/file drafts. |
| Constraints / categories / templates | Corresponding Source CRUD | Scalar/typed-reference/list forms. Existing draft values and read-time revisions are retained across snapshot refresh. |
| Interception / notifications / global settings | Corresponding Source rule/channel/settings APIs | Form widgets, provider-specific notification connection fields, headers and vault references, explicit delete/test actions. |
| Tasks | `ui.createTask`, `ui.controlTask`, `ui.archiveTask`, scope CRUD, `ui.recoverTaskBudget` | Create/start, pause/resume, archive, scopes, graph/activity links; uncertain elapsed budget needs explicit user recovery. |
| Company scopes | `ui.createCompany`, `ui.addCompanyScope` | Company kind defaults to `domain` and is independent of the task's `subdomain` selector. Supported company kinds are domain/IP/CIDR/ICP/keyword. |
| Conversations / sessions | Conversation create/send/history/stop; task chat; `ui.controlIntent`, `ui.activityHistory`, `ui.activityDetail`, `user.answer` | Worker pause/resume/cancel, parent task instructions, paged session history, pending question/plan/tool approval answers with pinned generation and revisions, scoped attachments. Message input is kept until successful admission. |
| Traffic | `ui.traffic`, `ui.trafficExchange`, `ui.trafficHosts`, deletion APIs | Host/method/status/body/path/size/order filters, 0-based pagination, request/response Body reads, explicit capture deletion. Capture error/available-byte state is shown separately from nominal byte counts. |
| Finding evidence / export | `ui.findingTraffic`, body/download APIs, `ui.exportFindings` | 8 KiB previews and paging, binary indication, full evidence export, JSON/CSV/Markdown/ZIP export. Base64 Source responses use typed decoded staging rather than saving encoded text as the file. |
| Workspace files | Workspace list/upload/download and typed Work `FileRead/Write` | Uses actual `entries` list shape, SHA-pinned edits, explicit reload after conflict, local selected-file staging, export preparation. |
| ScopeSentry | `ui.ssStatus`, `ui.ssDatasource`, `ui.ssProjects`, `ui.ssTasks`, `ui.ssSync` | Explicit network status/list requests, target paging/search/selection, asset-type selection, explicit sync click, visible counts/warnings/errors. API key setup uses the MCP vault-reference editor. |
| Diagnostics / LLM records | `legacy.getLogsHistory`, `ui.llmRecords`, `ui.llmRecordDetail`, `ui.llmRecordsDeleteTask` | Persistent log cursor paging, model/session/task filters, correct 0-based record pages, request/response/raw Body details, confirmed per-task record deletion. |
| Archives | `ui.restoreTaskArchive` | Existing archive restore is available. Archive creation is available from task controls. |

## Consistency boundaries

- Settings editors do not replace a draft from background query results. A
  changed read-time version blocks save until the user explicitly reloads.
- A save of one existing settings panel preserves the other draft buffers and
  requires explicit reload before starting the next edit. It does not silently
  upgrade an unrelated draft's revisions.
- File versions use SHA-256 rather than database `Expected` rows. Metadata
  revisions come from `SkillMetaRead`, not the skill catalog listing.
- Pane requests carry local request tokens. Reversed query completions cannot
  replace a newer filter result; concurrent Body reads keep separate paths.
- Staged-file completions check their initiating section, selection and target
  path. Stale capabilities are released rather than uploaded into a new target.
- Conversation message drafts are kept per task/conversation. A late success
  only clears its captured message and attachments; a newer draft and another
  conversation's input remain intact. Save/error acknowledgments target the
  original settings-editor token even after navigation.
- Suspend retires loading/pending flags while preserving unsaved parameters,
  private UI buffers, old database versions and file SHAs. Incomplete reads or
  uncertain saves require explicit reload; suspend itself does not approve
  discarding a draft.
- Source archive APIs contain archive/restore/delete workflows. The reviewed
  original `web/src/lib/api.ts`, Go server and frozen contract contain no
  `exportTask`, `previewImport` or `importTask` operation. No pretend archive
  import endpoint is presented.

## Source workflow and presentation differences

The Linux-style workbench keeps the original 23 Section identities in five
navigation groups and separates the object list from application navigation.
Native menus, toolbar, tabs and status bar use a shared light/dark Rust theme.
The inspector only reads the latest snapshot; it cannot mutate selections,
query owners, drafts or versions. The activity monitor can be collapsed.
Dashboard links and counts use actual stored tasks, runs, approvals and
questions. Task tabs retain goal/scope/exploration controls under Overview.
Conversation records and tool input/results are structured native widgets;
tool results can be expanded and the composer has a separate scroll boundary.
When bottom panels leave too little height, the whole conversation pane can
scroll. Existing management actions remain on their original scope/tokens.

The curated “추가 관리” menu now provides typed access to additional asset/company
scope batches, finding reports/evidence/retests, task goals/categories/templates
and task-specific interception, side-question history/checkpoints/cancellation,
judge policies, notification histories/retry, skill paths, archive batches,
model retry policies/probes/statistics, conversation management/message details,
command records and Python/search/tool tests. These operations use the same
scope, pinned generation and conflict checks as the primary native panes.

The original bespoke popup/table layouts have been replaced by Rust forms and
structured result viewers. Prompt preview/variable inspection is a separate
management action. The conversation reference picker searches typed snapshot
objects and inserts a canonical token; it does not reproduce the web editor's
live `@` autocomplete or caret-position insertion.

The primary graph is a workspace relation canvas with full name/ID search and
250-object pages. Source task/shared exploration graph data is also accessible
through the task management action. The canvas does not reproduce the web
graph's layout or persist node positions across application sessions.

The original UI contract does not expose independent report-body editing,
arbitrary intent CRUD, a dedicated per-MCP-tool visibility mutation, or archive
package import/export. Supported report reads/exports, evidence changes, goal
CRUD, intent control, agent/server visibility and archive restore/delete are
implemented. Those nonexistent endpoints are not presented as missing features.
The web-server updater remains intentionally excluded.

## Verification status

The separate `forms::operations::SourceOperations` module now provides a
curated Source-contract action menu for additional asset, finding, task,
side-question, interception, notification, archive, skill and model operations.
Its typed inputs and result viewer supplement the primary panes above. The
root integrated its callbacks and verified the real catalog, old-version CAS,
Body/Secret scope fences, helper-failure isolation and actual egui input rendering.
The resulting catalog contains 168 actions, including 11 Source side-question
routes. Catalog presence is not a claim that every possible argument/state has
been executed in an automated test.

Native form regressions cover optional-field preservation, read-time CAS,
secret callback fencing, opaque custom schema preservation, new typed identity,
large inline Body forwarding, and skill metadata SHA pinning. Pane regressions
cover reversed response fencing, stale picker release, opaque upload inputs,
legacy log cursor identity, company scope defaults, and a nonempty pending
question renderer. Additional regressions cover late A-to-B success/failure,
suspend draft retention, submitted Skill-file baselines, namespace-checked
reference tokens and label injection. The latest root verification passed 32
native UI tests, alongside native service/host checks and the actual 23-section
window, captured pixels, second-launch handoff and confirmed orderly shutdown.
Exact binary/source hashes and reused unchanged shared-regression evidence, when
applicable, are recorded in the verifier JSON. Agents did not run Cargo.

Headless widget/contract tests are not physical-platform qualification or proof
of every Source workflow. Refer to `PARITY.md` for host/storage/platform bounds
and to the final verifier's recorded native-window and service tests.
