# 원본 기능 차이 수정 및 검증 기록

기준 소스는 `a585764e9a88f5b3e4f39c5a787e4080b6b4c610`이다. 2026-10-16 읽기 전용 비교에서 발견한 F01~F15를 수정하는 작업이며, 기존 합의에 따른 Rust 단일 프로세스·객체/파일 저장·영문 프롬프트·네이티브 UI·승인/bypass·중지/정리 정책은 유지한다. 원본 Go/Web 코드로 되돌리는 작업이 아니다.

작업 시작 시 `docs/contracts/source-manifest.v1.json`의 원본 287개 파일 해시를 확인했다. 원본은 수정하지 않았다. 기존 `AGENTS.md`와 한국어·영어·중국어 README의 규칙 링크 변경도 보존한다. 이 작업은 로컬 소스 수정·격리 검증 범위이며 커밋·푸시·태그·릴리스 교체·안정판 전환을 포함하지 않는다.

## 수정 대응표

| ID | 원본에서 유지할 의미와 수정 | 구현 근거 | 격리 회귀의 핵심 조건 |
| --- | --- | --- | --- |
| F01 | 재검증 결론은 임시 저장하고 소유 실행의 정상 종료에서 확정한다. 취소·실패는 `fixed`를 확정하지 않는다. | `domain/src/findings.rs::finish_retests`, `conversation.rs::finish_run`, `rpc/src/controller.rs::finish_runtime` | 정상/실패/취소/결론 없음, 같은 트랜잭션의 실행·재검증·취약점 상태, 중복/다른 결론, Blob 증거 재열기와 원본 byte 한도 |
| F02 | 같은 실행의 MCP 연결·서버 상태를 호출 사이에 유지하고 종료 시 회수한다. | `host/src/mcp.rs::SessionPool`, `tools.rs::stop_run/stop_epoch` | stdio/Streamable HTTP/legacy SSE의 연속 호출, 동시 호출 직렬화, run/epoch/설정 분리, in-flight 취소, UI 임시 연결 종료 |
| F03 | deadline 마무리에서도 이미 확인한 사실·자산·취약점·목표 판단을 저장할 수 있다. 소진된 의도는 `exhausted`로 남긴다. | `controller.rs::begin_timeout_wrapup`, `tools/src/lib.rs::wrapup_tool_allowed`, `conversation.rs::finish_run` | 소유 intent·승인 모드 보존, 사실 저장, 새 의도/외부 실행 차단, plan 권한 유지, 체크포인트 후 purpose/parent 보존 |
| F04 | MCP `isError:true`는 성공이 아니다. | `controller.rs::complete_host` | camelCase 오류 플래그를 실행 원장·저장 결과·모델 transcript에 전달; 정상 응답 대조 |
| F05 | 일반 실행의 `max_turns=0`은 무제한이다. | `controller.rs::start_model` | 101번째 이후 실제 로컬 모델 요청, 유한 한도에서 마무리 지침 적용 |
| F06 | Write는 없는 상위 디렉터리를 생성한다. | `host/src/tools.rs` | 중첩 파일 생성, 경로 이탈·Unix symlink/Windows junction 차단, 생성 뒤 capability 재검사 |
| F07 | WebFetch는 HTTP 오류 페이지의 본문·응답 코드·서버 지문·일반 가시 텍스트·링크를 유지한다. | `host/src/network.rs` | 로컬 403/200 대조, div/span/독립 링크, script 제외, 깊은 HTML·UTF-8 byte 한도, 비 HTTP 거부 |
| F08 | 백그라운드 Bash/Monitor 완료를 실행 소유자에게 한 번 알린다. | `host/src/execution.rs::drain_events`, `bridge.rs` | 성공/실패 알림, run/epoch 격리, 중복 drain·foreground 제외, 출력 본문은 scoped artifact에 유지 |
| F09 | Anthropic thinking 비활성화 시 과거 thinking을 제거하고 빈 메시지를 보내지 않는다. | `runtime/src/provider.rs::encode` | 서명 유무·비활성/활성 대조, 연속 role 병합, 원본 transcript 불변 |
| F10 | 공급자별 reasoning 필드의 의미를 JSON/SSE 양쪽에서 보존한다. | `provider.rs`, `provider_stream.rs` | `reasoning_content`/`reasoning`/`reasoning_text` 우선순위, Responses content reasoning, 도구 replay 유지 |
| F11 | 독립 질문은 오래된 대화와 큰 부모 문맥을 실제 모델로 요약한다. | `rpc/src/side_context.rs`, `side_runtime.rs`, `domain/src/side_questions.rs` | 요약→답변 HTTP 왕복, ordinal memory·재열기·provenance 캐시, tool-pair 보존, overflow 한 번 축소, 실패·취소 사용량, clear 뒤 늦은 답변 차단 |
| F12 | 새 작업에서 원본의 생성 옵션과 템플릿을 실제로 지정한다. | `native/src/task_creation.rs`, `panes.rs` | 생성 버튼의 전체 옵션 명령, 템플릿 적용/초안 유지 클릭, 잘못된 값·삭제 참조·읽기 전용 차단 |
| F13 | 취약점 검색·그룹·트리·필터 export는 Blob으로 저장된 report/evidence도 검색한다. | `domain/src/findings_ui.rs`, `asset_graph_ui.rs`, `report_export.rs`, `rpc/src/facade.rs` | 큰 UTF-8 본문의 뒤쪽 검색, inline 대조, 관련 없는 Blob 읽기 생략, 필요한 읽기 실패는 명시적 오류 |
| F14 | 작업을 보관해도 전체 취약점·토큰·도구·스킬 통계는 유지된다. | `domain/src/archive_statistics.rs`, `configuration_ui.rs`, `skills_ui.rs`, `rpc/src/archives.rs`, `importer/src/archive.rs` | 실제 usage 기록→보관→hot 삭제→조회→이전 preview 메타데이터 읽기 전용 복구→재열기/복원; opaque 원장 키·탐색 소유권 보존 |
| F15 | commandStats의 query는 저장된 명령 본문까지 검색한다. | `domain/src/auxiliary_ui.rs` | inline/Blob 명령·출력·오류 본문 검색, 필터/합계/페이지 결과, 필요한 Blob 실패 전파 |

경로는 위 표에서 `desktop/crates/` 기준이며 `native/`만 `desktop/` 기준이다. 원본 근거는 재검증 `db/finding_retests.go`, MCP `server/assembly.go`·`norma/mcp`, 마무리 `agent/wrapup.go`·`worker.go`, 도구 `norma`, LLM `norma/llm`, 독립 질문 `sidequestion`, 작업 생성 `web`, 통계 `db`의 관련 구현과 대조했다. 계약의 이름·도구 개수만으로 통과 판정을 내리지 않는다.

## 저장·권한·호환 경계

- 새로운 DB·서비스·프로세스 계층이나 의존성은 추가하지 않는다. manifest·lock 및 원본 SQL/도구/RPC 계약의 동결 형태는 유지한다.
- 마무리 실행의 선택 필드 `purpose`와 `parent_run_id`는 이미 존재하는 스키마 필드다. required 기본값만 병합하는 생성 경로에서 사라지던 값을 완전한 run 레코드로 저장한다. 이전 레코드를 일괄 수정하거나 사용자 작업을 자동 재개하지 않는다.
- deadline 이후 허용하는 그래프 기록도 정상 role·binding·승인 검사를 통과해야 한다. 사용자의 plan 모드를 승격하지 않는다. 새 작업·의도·네트워크·외부 프로세스는 마무리 허용 목록에서 제외한다.
- MCP 세션은 run/epoch/서버별이며 승인된 설정·작업 디렉터리·네트워크 설정이 달라지면 이전 연결을 폐기한다. 불확실한 외부 호출을 자동 재실행하지 않는다. 연결 종료와 실제 외부 프로세스/출력 정리 증명은 구분한다.
- 새 보관 파일에는 task 소유 원장에서 계산한 통계를 보관한다. 이전 preview의 table-count 메타데이터는 기존 불변 패키지를 같은 importer 경계로 검증한 뒤 조회 뷰에서만 복원한다. 읽기 전용 저장소에 repair write를 하지 않으며, 누락·손상된 패키지를 0건 성공으로 표시하지 않는다.
- 독립 질문의 요약은 추가 LLM 호출과 실제 사용량을 발생시킨다. 부모의 고정 checkpoint·transcript·그래프는 변경하지 않는다. 취소 후 도착한 worker 결과는 같은 세대에서 사용량만 정산할 수 있으며 답변·memory·상태를 되살리지 않는다.

## 검증 범위

최종 전체 실행은 `desktop/native/test-results/4a25dc8d6d204311a11cae623dbe5a47/verification-result.json`이다. `RUST_TEST_THREADS=1`로 `desktop/native/scripts/verify.ps1 -FullRegression`을 실행했다. 시험 내부의 명시적 동시 호출·취소/세대 충돌 조건은 유지했다.

| 검사 | 최종 결과 |
| --- | --- |
| native-core 서비스 | 24개 통과 |
| host, no-default-features | 67개 통과; 신규 호스트 통합 9개 및 SSE/깊은 HTML 회귀 포함 |
| 네이티브 UI | 117개 통과; 새 작업 실제 생성/템플릿 버튼 회귀 4개 포함 |
| Cargo 공통 workspace | 368개 통과; 실패·무시 0개. RPC 121개, runtime 19개 포함 |
| 실제 실행 파일 | 빌드, 통합 서비스/로컬 모델/파일 도구, PE·창 아이콘, 23개 화면, 정상 종료 통과 |
| 두 번째 실행 | 같은 프로필의 단일 소유 창 유지 및 정확한 request ID/path/read_only의 GUI 수신 확인. 모드 전환 대화상자의 사용자 수락은 자동화 범위 밖 |
| 원본 계약 검사 | 54 tables / 571 fields, 261 routes, 258 UI operations, 81 tools, 1,221 schemas 및 정상/거부 예제·저널 byte 통과 |

검사 범위가 겹치므로 위 시험 수는 합산하지 않는다. 각 단계의 stderr에서 Cargo warning은 0개다. 검증 입력 208개 파일 해시가 종료 후 현재 소스와 모두 일치했다. 원본 287개 해시 및 이전 AGENTS/README 3개 변경의 보존도 별도로 재확인했다. manifest·lock·동결 계약 변경은 없다.

검증한 개발 실행 파일은 `desktop/target/debug/artex-native.exe`이며 SHA-256은 `36632c0be96dc63c3a3b5b8edd40b931bb400dc93503c909dd8ba99e1374de5a`다. 위 검증 JSON의 SHA-256은 `b7ee4d177ffbebb98387c5dda2ea0cd369d45be2a8f200fc6055741a1fdaac71`다. 실제 PNG를 직접 검토했다. 두 번째 실행의 읽기 전용 전환 확인창이 열린 상태이며 저장 후 전환/현재 공간 유지 버튼과 본문이 표시된다. 관찰된 자식 프로세스는 Windows `conhost.exe`였고 별도 ARTEX engine/WebView는 관찰되지 않았다. 이 개발 빌드를 릴리스 배포물로 교체하지 않았다.

중간 실패도 최종 통과 근거와 구분한다. HTTP MCP 종료 시 Tokio runtime 밖에서 future를 만들던 문제, 이미 inline 값이 일치해도 관련 없는 큰 Blob을 먼저 읽던 문제, 마무리 run의 선택 필드가 생성 병합에서 사라지던 문제는 제품 경로를 수정했다. 추가 검토에서는 지속 SSE 세션의 누적 바이트를 한 응답으로 계산하던 문제도 수정했다. 보관 시험의 handshake/fixture ID 및 재열기 시험의 저장 디렉터리 오기는 시험 설정을 바로잡았다. 실패한 실행이나 설정 오류가 있던 실행을 통과 숫자에 더하지 않는다.

첫 전체 실행 `ffdc30cd572b4501900b69be490d9000`은 서비스·호스트·UI 단계 후 공통 도메인 회귀에서 중단됐다. 새 정상 종료 시험이 Store에 직접 커밋할 때 필수 command ID를 설정하지 않아 거부된 것으로, 다른 기존 시험과 같은 준비 절차를 추가했다. 실제 저장 검증이나 원자성 assertion은 약화하지 않았다. 이 실행에는 전체 통과 판정을 부여하지 않는다.

실제 사용자 작업 공간·키·실제 사이트·실제 LLM 공급자는 시험에 사용하지 않는다. 로컬 HTTP/SSE/stdio fixture와 격리된 저장소·앱 프로필로 재현한다. 이 검사는 Windows에서의 기능 회귀 근거이며 macOS/Arch의 실기기 UI·보안 저장소·프로세스 정리나 모든 외부 MCP 서버·모델의 동작을 인증하지 않는다. 원본 Go 전체 시험을 실행했다는 주장도 하지 않는다.
