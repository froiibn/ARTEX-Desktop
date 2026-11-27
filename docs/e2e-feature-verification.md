# 원본 기능별 종단 간 검증표

2026-10-17. 기준 `a585764e9a88f5b3e4f39c5a787e4080b6b4c610` + 작업 시작 당시 수정본. **V01–V14 제품 수정 전에 작성한 수락 기준**이다. 원본 고정 manifest 287개 파일, 현재 소스 325개 파일의 시작 해시는 저장소 밖 `../.organization/e2e-vfixes-20261017/start.json`에 보존한다.

원본 기능의 개별 목록은 [JSON](e2e-feature-matrix.v1.json) / [CSV](e2e-feature-matrix.v1.csv)에 있다. 원본 HTTP 경로 261개, 화면 API 258개, 도구 81개를 각각 빠짐없이 등록했다. 세 목록은 같은 기능을 서로 다른 경계에서 가리키므로 **600개 독립 기능으로 합산하지 않는다**. 목록·스키마의 존재만으로 실행 통과를 표시하지 않는다.

원본 HTTP 경로 행은 업무 출처를 확인하는 참조다. HTTP 서버 전송은 통합 앱 설계로 대체했으므로 같은 업무의 UI 명령/도구에 연결해 검증한다. 정확한 공개 어댑터 경로로 220개 행을 자동 대조한 뒤, 나머지 41개도 양쪽 handler·호출부·해시·줄을 수동 대조했다. 그중 31개는 같은 업무 코드, 8개는 native 설계 대체, 2개(`insertAssets`, `reattribute`)는 수동 관리 API의 대응 확인 필요로 남겼다. 원본 웹 호출부가 확인되지 않는다는 이유로 두 API를 통과시키지는 않았다. 이 대조는 정적 근거이며 실행 성공이 아니다.

목록 gate는 화면 명령 258개와 도구 81개(중복 의미는 유지)의 실행 근거를 검사한다. 이는 전체 동등성의 필요조건이며 충분조건이 아니다. 별도 `legacy.*` UI로 직접 노출된 12개 참조 행과 두 미대응 API의 수락 여부도 따로 해결해야 한다. 참조 경로 수를 실행 성공 수로 계산하지 않는다.

## 판정과 검증 경로

모든 유지 대상 경로의 수락 순서는 다음과 같다.

1. 실제 native 위젯/폼의 입력·초안·선택 ID 및 명령 이름을 확인한다.
2. UI가 사용하는 public RPC/typed service에 올바른 workspace/epoch/command ID/CAS로 요청한다. 내부 plan의 직접 commit만으로 이 단계를 대신하지 않는다.
3. 정상·경계·오류·취소의 실제 실행 결과와 외부 효과/소유권을 확인한다. 승인 접수와 완료를 구분한다.
4. 결과 계약을 검증하고 레코드·본문 Blob·사용량·도구 결과·이벤트를 확인한다. 실패 시 커밋되지 않아야 할 변경도 확인한다.
5. checkpoint/닫기/재열기 후 원래 조회 API로 상태와 참조를 다시 확인한다. 자동 재실행이나 불확실한 외부 효과 재생을 허용하지 않는다.
6. 조회 결과가 실제 native 표시/선택/다음 명령에 연결되는지 확인한다. 화면 렌더링만으로 이전 단계들을 통과시키지 않는다.

단계별 근거를 `정적 연결`, `계층 실행`, `연결 실행`, `미검증`, `환경 제한`으로 구분한다. 전체 경로가 실행되지 않은 개별 항목은 E2E 통과로 세지 않는다. 명시적 변경은 아래 합의한 새 기대를 검증하여 `변경점 통과`로 기록한다. Windows fixture로 macOS/Arch 실기기 검증을 대신하지 않는다.

## 기능군별 고정 수락표

| ID | 원본 기능·근거 | 현재 입력→처리→저장→표시 | 필수 시나리오·수락 조건 |
|---|---|---|---|
| E01 | 서버 시작·인증·설정 (`server/auth.go`, `server/server_mgmt.go`) | native 시작/작업 공간 선택→native-core 서비스→Store→설정/상태 | OS 세션 대체, 읽기 전용, 잘못된 경로, 중복 시작, 종료/재열기. 로그인/JWT 부재는 설계 대체로 별도 판정 |
| E02 | 회사·회사 범위 (`db/companies.go`, 회사 HTTP 등록) | 회사 폼→ui.createCompany/addCompanyScope/updateCompanyScope/deleteCompany→domain→Store→회사/자산 | domain/IP/CIDR/ICP/keyword, 중복·잘못된 범위, 삭제시 관계 해제, 재열기 |
| E03 | 작업 생성·수정·분류·템플릿 (`server/server.go`, `db/tasks.go`, `agent/tools.go`) | task_creation/폼→create/update/category/template RPC→tasks→Store→작업 상세 | goal/company/category/template/source/profile chain 전체, 원본 삭제 후 숨은 선택 **V12**, 초안 유지·명시 해제, 참조 재열기 |
| E04 | 작업 실행·큐·중단·재개 (`agent`, 작업 제어 handler) | 작업/실행 버튼→controlTask/Intent→controller→run/intent/task→상태·이력 | start/pause/resume/stop/cancel/강제종료, timeout, queue, 무제한0turn, 명시 재개·소유 프로세스 정리, 불확실 효과 회복 |
| E05 | 작업 보관·복원·삭제 (`server/task_archives.go`) | panes::archives→archive/restore RPC→archive worker→패키지/Store→목록·복원 작업 | 단일/배치, 큐 수락/완료 분리, 실제 단일 복원 계약 **V01**, 기존/새 패키지·해시 오류·의존성, 재열기후 task graph/통계 |
| E06 | 목표·제약·범위·작업 자산 (`server`/`db` 해당 등록) | 작업 폼→goal/constraint/scope/asset RPC→domain→Store→작업 탭 | CRUD, 직접 source읽기 범위, nullable 관계, CAS 충돌, 삭제 후/재열기 |
| E07 | 자산 수집·검색·회사 귀속 (`db/assets.go`, `agent/tools_insert.go`) | 자산 도구/화면→tool.invoke/assets RPC→domain→Store→검색/자산 수 | 6타입·자연키·중복 병합·태그·회사 자동/명시 귀속·관계 attach/detach·페이지·재열기 |
| E08 | 탐색 그래프·노드·앵커·digest (`agent/tools*.go`, `db/exploration*.go`) | 그래프/도구→domain exploration→Store→graph/frontier/intents | begin/goal/intent/fact/finding/hint/digest, nonempty 간선·계보 계약 **V17**, state/version/edge·링크 제약, 공유 그래프·마감 settlement·재열기 |
| E09 | 취약점·재검증·심화 (`server/findings*`, `agent` 재검증) | finding 폼/도구→finding/retest RPC→controller/domain→증거·결과→finding 화면 | 수정/등급/상태/삭제, 진행중 결과 stage·terminal 확정, 취소/실패/stale version·외부증거·재열기 |
| E10 | 증거 트래픽·내보내기 (`server/finding_traffic.go`) | evidence viewer/export→body/bind/export RPC→Blob/host capability→파일/표시 | immutable snapshot·순서·추가/편집/삭제, binary/다중청크·다운로드 내용, JSON/CSV/MD/ZIP, 재열기 |
| E11 | 일반/작업 대화·세션 (`server` chat/conversation) | composer/대화 목록→send/history/detail/stop→controller→transcript/usage→본문 | 정상 text/tool/thinking, 첨부·nonempty mention 검색 및 typed ID **V18**, page, stop/late reply, clear/trash/restore/purge, 재열기·draft ownership |
| E12 | Side question·문맥 요약 (`server/side_questions.go`) | side drawer→side RPC→worker→answer/summary/usage→drawer | 부모 snapshot 고정·client dedup·clear/cancel/stale epoch, 부분본문·summary 구분·재열기 |
| E13 | LLM 설정·프로필·체인·풀 (`server/server_mgmt.go`, `norma/llmpool`) | Models 폼→save/activate/delete/test→domain/runtime→profile/health→모델 화면 | secret ref 유지·설정 미저장 키 미사용, 실제 delete **V02**, failover/chain exhausted·자체deadline 제외 **V08**, 재열기 |
| E14 | 공급자 요청·JSON/SSE·이어쓰기 (`norma/llm`, `norma/harness/query.go`) | composer/task→start_model→provider→complete_model→message/usage/record→history | Anthropic/Chat/Responses, max_tokens continuation **V03**, terminal missing/[] **V04**, named SSE **V05**, empty args **V07**, transport/parse/cancel/no replay |
| E15 | 토큰·사용량·LLM 기록 (`db/llm_usage.go`, `server/commands.go`) | dashboard/기간/LLM record→stats RPC→hot+cold→Body→통계 표 | 캐시 포함 총입력 **V06**, rolling days/0/default **V13**, profile/task/date, 삭제/보관/복원·재열기, ledger 불변 |
| E16 | Agent 설정·prompt·버전·trigger (`agent`/server mgmt) | Agents 폼→agent/prompt/config/trigger RPC→Store→preview/history | prompt 英문화의 역할·도구·wrapup 계약 유지, reset/archive/current ownership, enabled/visibility·trigger 조건·재열기 |
| E17 | 고정 81 도구·동적 사용자 도구 (`agent/tools*.go`, `norma/tool`) | runtime tool offer→policy→next_tool→tool.invoke/HostBridge→ledger/Body→transcript | 각 도구 개별 행에 schema/handler/제공조건, 실제효과/오류·scope/CAS/중복, 성공·거부·불확실 효과를 구분 |
| E18 | 셸·PTY·백그라운드 실행 (`norma/tool/shell_session.go`) | Bash/shell_*→host supervisor/ConPTY→result Blob→model/session viewer | 반환 ShellId의 다음 공개 호출 **V15**, drain **V09**, non-drain/offset·omitted·UTF8·완료/kill/cleanup/restart, 반환안된 데이터를 소비하지 않음 |
| E19 | 파일·검색·첨부·리소스 (`norma/tool/files.go`) | Read/Write/Edit/Grep/Glob/GUI files→host capability→파일/Blob→본문 | 긴 앞행 offset **V11**, 선택행 truncation·binary·빈파일·nested path·SHA conflict·권한·재열기 |
| E20 | HTTP·WebFetch·WebSearch (`norma/tool/webfetch.go`) | tool→host network→HTTP capture/asset/result→Body→본문/증거 | raw/markdown/RECON, 압축 보안신호 **V10**, Content-Type case **V14**, 403/timeout/UTF8/50k/오류 증거 |
| E21 | MCP·사용자 도구·Python (`agent` MCP/custom tool) | MCP/Tools 폼→refresh/test→HostBridge session/process→cache/result→화면/model | stdio/streamable/legacy SSE, long-lived session/timeout/cancel·JSON-RPC 오류 정상유지·isError·secret env·cleanup |
| E22 | Skill·파일·가시성·사용량 (`server` skills / `norma/tool`) | skill picker/editor/tool→native staging/host→resources/usage→skills/files | ZIP overwrite 선택·unknown YAML 보존·SHA conflict·실행 resource/mcp view·누락 skill·재열기 |
| E23 | 승인·규칙·judge·질문·계획 (`agent` interception) | approval/user input→interceptDecide/user.answer→policy→invocation→activity | allow/block/ask precedence, exact request/version, workspace bypass 범위·deny/plan 유지·빠른 재평가·duplicate/stale/취소 |
| E24 | 알림·재전송 (`server` notifications) | 채널 폼/test→notifications worker→delivery state→history | 신규 채널 생성/기존 credential 변경 guard 분리 **V16**, vaultref·전송실패·재시도·중복fanout방지·background completion·재열기; 실제외부발송 없이 local fixture |
| E25 | 트래픽 캡처·검색·삭제 (`traffic`, server traffic) | 도구proxy/capture→Blob+exchange→traffic RPC→viewer | request/response 누락·binary·body限·host/filter/0page·삭제/증거보존·재열기 |
| E26 | ScopeSentry (`server` ss handlers) | 명시 조회/동기화→host remote→task asset link→화면 | key ref·project/task paging·선택 asset type·실패/중복·명시 sync·재열기; 실제서버 대신 HTTP fixture |
| E27 | 로그·명령·기록 조회 (`server` logs/commands) | native logs poll/ack→Store→history/detail RPC→drawer | bounded spool·ack durable·Blob 검색/청크·page/filter·delete·재열기; 임의 stderr 자동 수집은 미구현 경계 |
| E28 | 원본 데이터 이전·저장·회복 (`db/schema.sql`, archive formats) | importer/migrate→single-writer CAS/WAL/checkpoint/Blob→reopen/query→native UI | 54table571field, FK/NULL/cascade·legacy opaque key·v1–v3archive·손상/읽기전용·명령 dedup·epoch·비밀분리 |

## 명시적으로 추가·개선한 사항을 유지하는 목록

아래 사항은 사용자 지시와 현재 설계/코드에서 확인하는 유지 대상이다. **원본과 다르다는 이유만으로 제거·회귀 수정하지 않는다.** 실제 새 동작의 부수적인 명령 실패나 데이터 손실은 별도 결함이다.

| 변경 | 사용자 의도·새 기대 | 확인 위치 |
|---|---|---|
| Rust 통합형 native desktop | 언어만 변환한 FE/BE exe가 아니라 한 Rust 앱·프로세스; 일반 사용자 실행에 Docker/DB/Node/WebView 불필요 | desktop/native, native-core, docs/desktop-native-architecture.md |
| Windows/macOS/Arch 지원 목표 | 플랫폼 분기·프로세스·창·배포 준비; 실기기 미검증은 명시 | .github/workflows/native-rust.yml, docs/desktop-platform-audit.md |
| DB→객체 그래프·파일 | 기존 에이전트 정보공유를 지속 저장·ID참조·single writer/CAS로 유지 | crates/store, domain, docs/desktop-implementation-spec.md |
| Brevduva **구조만 참고** | 자체 artex-agent/1.0, 외부 Brevduva 서버/SDK 연동 요구 없음 | crates/agent, contracts/agent-message.v1.schema.json, docs/brevduva-structure-reference.md |
| 영문 LLM prompt | 모델 호환 목적의 언어 변경, 역할·context·tool 계약 유지 | crates/domain/resources/builtin-prompts.v1.json, desktop/tests/prompt-comparison-audit.mjs, rpc/prompt_tails.rs |
| 한국어/영어/중국어 소개 문서 | README/소개문서만 세 언어, 앱 전체 중국어 지원을 약속하지 않음 | README*.md, docs/project-purpose*.md |
| workspace bypass | 현재 작업 공간에 저장, 내부 실행심사 생략; 명시 deny/plan/scope·일반질문 유지 | settings approval_bypass, runtime/permission, native/workbench |
| 승인 응답성·상태 표시 | 사용자 변경 FIFO 우선·정확한 request 소유, 접수와 실제 완료 구별 | native-core, rpc/controller, native model |
| 작업/대화 삭제·trash·restore·purge | 명시 삭제와 scope 격리, 실제 external cleanup 확인; journal/backup secure erase 아님 | native lifecycle, rpc/native_lifecycle |
| 에이전트 현황·stop/force-stop | 실행 상태·자식 외부 자원·소유권/cleanup 결과 표시, pause와 stop 분리 | native execution panes, host supervisor |
| 작업 공간 열기시 명시 재개 | 열기만으로 paused 작업 재시작하지 않음, 현재 공간 저장·실행 정리 후 전환 | native-core workspace lifecycle, workbench |
| Linux desktop 느낌의 둥근 UI | soft palette·명확한 버튼·가로 정렬·폭/최소창 대응, sidebar 유지 | native/theme/forms/panes/workbench |
| 대화 여백·구분선·출력폭 개선 | composer 하단 불필요영역 제거, 얇은 구분선, 하단 Activity/Management full-width drawer | native/panes/workbench |
| 앱 일체형 프레임·가운데 제목 | native titlebar/drag/resize·OS별처리, 실제 제목 중앙 | native/window_chrome |
| 중앙 시작화면·workspace 버튼 | 새 영역/기존 영역 분리, 작업목록·열기·새공간이 실제버튼, 저장/정리후전환 | native/workspace_landing/workbench |
| dashboard 개선·상세 조건부 | dashboard에서는 상세 숨김, 실제선택 항목에서만 상세동작 | native/panes |
| 앱 아이콘·릴리스 패키지 | 원본아이콘 자산·exe resource/창/tray·배포 대응 | native/assets/branding, build.rs, packaging |
| 방어적·비전공자 지향 설명 | 자신이 소유한 서비스 분석/대응, 복잡한 설치 제거 목적 | docs/project-purpose*.md |

추가로 명세에서 승인된 구현 선택(typed ID, task별 공유 작업디렉터리, OS 비밀 저장소·파일 capability)은 해당 불변식으로 검증한다. 서버 로그인/원본 self-update의 OS/배포 대체는 설계상 대체 항목으로 표시하며, 사용자가 별도로 지시한 UI 개선과 혼동하지 않는다. Git 원본연결 분리·이력정리·라이선스/출처 고지는 저장소 관리 이력으로 보존하며 실행기능 누락의 면책 근거로 사용하지 않는다.

## V01–V23 수정·재검증 추적

2026-10-20 명시적 변경: LLM·일반 설정·에이전트·도구/MCP·스킬·전역 승인 정책·알림 채널 구성을 앱 프로필에서 공유한다. [별도 공통 설정 수락 기준](shared-application-settings.md)을 적용한다. 실제 Service의 저장·활성화·공간 전환·재시작, readonly 전체 입력 해시 불변, 삭제·FK 정리·대형 프롬프트/카탈로그·스킬 전체 파일 이전과 별도 실제 App의 A/B 대화 입력→로컬 HTTP 응답→저장/표시→readonly 재열기를 확인한다. Bypass와 사이트/작업별 규칙·실행 데이터는 공간에 유지한다. 설정 Form의 모든 입력을 실제 UI에서 조작한 시험과는 구분하며, 이 경로를 기존 설정 관련 행 전체의 E2E 수락으로 승격하지 않는다. 확장된 공개 서비스 경로 근거는 27개 관련 행에 추가했으나 전체 수락은 11/339를 유지한다.

2026-10-20 명시적 추가 기능: 사이트 분석의 종합 보고서와 native 전용 탭·전체 Markdown 내보내기는 [별도 수락 기준](site-assessment-report.md)을 따른다. 기존 `ui.report` 행 UI138에 실제 보고서 탭→Service 조회→저장 기록→표시→같은 공간 읽기 전용 재열기 근거를 추가했다. 이 연결 시험은 중지된 부분 보고서 사례이며, 완료 상태·실제 도구·대용량 전체 내보내기·UTF-8 페이지 경계는 각각 별도 RPC/서비스/위젯 시험으로 구분한다. 실제 OS 창·저장 위치 대화상자 조작이나 전체 원본 동등성을 인증하지 않는다. 현재 제한 수락 행은 11/339이며 나머지 328행의 전체 경로 수락은 남아 있다. JSON의 과거 `last_verification` 집계는 당시 날짜의 기록으로 보존한다.

| 결함 | 연결 기능 | 수정 후 필수 연결 검증 | 실행 근거·판정 범위 |
|---|---|---|---|
| V01 | E05/E15 | 실제 restore UI RPC admission→worker 완료→checkpoint/reopen→task/graph/stats; rich 통계 보존 | domain_ui_e2e_tests 통계/공개 계약; app_workflow_e2e_tests 실제 복구 버튼→동일 workspace 재열기·표시. 비어 있지 않은 통계와 실제 App 경로는 별도 시험 |
| V02 | E13/E03 | 실제 profile delete 계약→Store 삭제·참조해제→reopen→draft 선택 명시해제 | domain_ui_e2e_tests의 참조 해제·계약·CAS 대조; actual_app_*의 원래 Form CAS/Prepare/완료·재열기·표시 연결 |
| V03 | E14/E11 | 실제 model HTTP cap→부분답변 저장→resume nudge→다음 요청→완료→usage/history reopen; 무한 복구 금지 | e2e_parity_tests와 실제 app_provider_e2e_tests composer 입력/전송→HTTP 두 번→정확 사용량/부분본문→같은 공간 재열기/메인·상세 표시. 무한 복구 부정 대조는 controller 시험 |
| V04–V07 | E14/E15/E17 | 공급자 actual HTTP→Turn→controller ledger/Body→reopen, named event/terminal/delta/cache/empty args 정상·부정 대조 | provider_fixtures/e2e_parity_tests 및 실제 App의 Responses→shell_list→원장/Body→재열기/표시. malformed JSON은 host attempt 0·이력 1회 보존. Anthropic 캐시는 cap App 사례에서 정확 대조. 실제 외부 공급자 서비스 시험 아님 |
| V08 | E04/E13 | own deadline TIMEOUT→health/fallback 영향없음→settlement; 실제전송실패·cancel 대조→reopen | e2e_parity_tests와 실제 App의 자체 마감/HTTP500 대조·메인/풀 표시·재열기. 공개 wrapup=0으로 자식의 성공이 부모 health 오염을 숨기지 않게 함. 풀 조회 필드값은 시험 seed |
| V09–V11/V14 | E18/E19/E20/E25 | 실제 PTY/HTTP/file→tool.invoke/host completion→Blob/transcript→조회; cleanup·읽기경계 대조 | e2e_host_boundaries 및 실제 App composer→모델 도구 제공→HostBridge→원장/Blob→상세/미리보기/전체 복사→같은 공간 재열기. Read offset2와 긴 앞행 검출; limit만의 독립 경계는 host 시험. 파일 준비는 fixture이며 파일 선택기 입력 검사는 아님 |
| V12 | E03/E13 | 실제삭제→snapshot→egui 명시해제→create RPC→refs 재열기 | actual_app_*의 동일 fixture 실제 체크박스·명시 해제·생성/완료 선택·정확한 관계·재열기·표시 |
| V13 | E15/E05 | actual usageStats/day lookup→hot/cold/public restore→checkpoint/reopen→날짜/총합 표시 | parity_statistics/domain_ui_e2e_tests의 hot/cold/기간/default/재열기와 실제 App의 days=1 조회 버튼·by_profile 총합 표시·재열기. 날짜 입력값/조회 선택은 시험 seed로, 날짜 위젯 타이핑·App daily 값까지 검증한 것은 아님 |
| V15 | E18 | 실제 Service/model shell_open→반환 ID로 shell_read→원장/Blob/reopen·cleanup | 실제 App의 원래 ShellId 표시·다음 호출·70KiB drain/Blob·본문 읽기 ticket/owner·미리보기/원문 복사·재열기와 기존 Service/host 시험. 재시도·소유권·capability/설정 충돌 거부 포함 |
| V16 | E24 | createNotify→writer→조회/reopen; 업데이트 guard·masked 값 부정 대조 | domain_ui_e2e_tests. 실제 채널 폼 입력·외부 메일 발송은 미검증 |
| V17 | E08 | nonempty graph/nodes/lineage→공개 계약→reopen; 실제 조회 버튼→tree | domain_ui_e2e_tests와 native SourceOperations 시험. 각각의 성공을 동일 fixture 전체 경로로 합산하지 않음 |
| V18 | E11 | nonempty company/finding/asset mentions→계약·20+1페이지→reopen | domain_ui_e2e_tests 및 스키마 정상/부정 대조. 실제 mention picker 입력/표시는 미검증 |
| V19 | E01/객체·파일 저장 | 큰 체크포인트+미반영 로그 저널→Store 검증/재생→native Service ready→실제 화면·정상 종료; 잘못된 frame 원자적 거부 | Store 30개·Service 열기/중단 2개, 기존 작업 공간의 독립 읽기 전용/일반 복사본 실제 23화면. 원본·읽기 전용 파일 해시 유지. [V19 기록](e2e-corrections-20261017.md#v19-기존-작업-공간-열기-지연). 개별 기존 기능 행의 전체 경로 수락으로 승격하지 않음 |
| V20 | E11/대화 생성 | 실제 이전 이름 검증 실패→새 대화 버튼→Prepare/저장/선택→같은 공간 재열기/표시; 오류의 발생 명령 식별 | app_feedback_tests. 빈/공백/일반 제목과 모델 미실행 대조. 사용자 보고 자체는 현재 빌드에서 직접 미재현이며 확인된 이전 오류 잔류만 수정 |
| V21 | E11/명시적 대화 UI 개선 | 생성 폼·메시지·입력창의 동일 중앙 열, 좁은 창/하단 패널/첨부·전송 보존 | chat_column_tests 및 기존 composer/App 전송 회귀. 860 논리 픽셀은 읽기 열의 UI 설계 폭이며 제품 데이터 제한이 아님 |
| V22 | E03/E11/명시적 목록 UI 개선 | dark/light·idle/hover·선택/비선택·disabled 실제 체크박스 도형과 클릭 상태; 스타일 누출 없음 | theme_checkbox_tests 및 기존 목록/관리 회귀. 기본 위젯 전체의 사각 테두리를 복구하지 않음 |
| V23 | E04/E11/분석 상태 표시 | 저장된 작업 상태·최신 대화 실행→구체 대기/진행/마무리/종료 표시; 로컬 알림·중복/재열기 무음·실패/다른 작업/이전 epoch 대조 | analysis_status 및 actual_composer_displays_running_completed_failed_and_local_notice_without_reopen_replay. 대화 응답 종료와 전체 작업 분석 종료·호스트 정리 확인을 혼동하지 않음 |

실행 결과·증거 경로와 해시는 [수정 보고서](e2e-corrections-20261017.md)에 기록한다. 개별 행의 `missing_stages`는 단계별 미확인 목록이다. 서로 다른 fixture에서 여섯 단계 이름을 모두 관측해도 연결 성공은 아니다. 전체 경로 통과는 동일 흐름의 `connected_path: true` 근거가 있어야 한다. 아직 실행하지 않은 항목은 완료로 표시하지 않는다.


## 2026-11-24 SQLite 저장 전환 재검증

[SQLite 명세](sqlite-storage.md)에 따라 업무 저장을 SQLite v2로 변경했다. 원본의 객체·명령·도구 의미는 유지한다. 저장소 42건, 서비스 45건, 네이티브 UI 153건 및 표준 실행 파일 자체 검사를 수행했다. 기존에 연결 경로가 통과했던 11개 행만 동일 App fixture의 SQLite 재실행 근거를 추가했고 수락 범위를 확대하지 않았다. 개별 미검증 단계와 328개 잔여 판정은 유지한다. 초기 실패와 동시 실행 시간 초과는 최종 통과에 합산하지 않으며 상세 실행 근거·해시·제약은 `desktop/native/STATUS.md` 및 로컬 SQLite 검증 결과에 기록한다.

## 2026-11-24 preview.5 이후 안정판 수락 준비 현황

대상은 배포 소스 `ad9a20c1f1ea46580b6624ebd40c192a2d388569`이다. 이번 작업은 기존 검증 근거와 현재 코드의 확인·정리이며 제품 코드 수정이나 328개 경로의 신규 실행 검증을 수행하지 않았다. 아래 집계로 기존 개별 행의 판정을 승격하지 않는다.

### 집계와 의미

| 대상 | 수락 대상 | 제한된 연결 경로 통과 | 부분 경로 근거 | 검증표상 미검증 | 미완료 |
|---|---:|---:|---:|---:|---:|
| UI 명령 | 258 | 6 | 132 | 120 | 252 |
| 도구 | 81 | 5 | 76 | 0 | 76 |
| 합계 | 339 | 11 | 208 | 120 | 328 |

이는 328개의 결함이나 미구현 기능을 뜻하지 않는다. `미검증`은 이 검증표에 단계별 실행 근거가 등록되지 않았다는 뜻으로, 다른 계층 시험의 존재 여부와 구분한다. 반대로 최신 회귀 시험이 통과했다는 이유로 UI 입력부터 표시까지 검증된 것으로 추정하지 않는다. 600개 출처 행(261 HTTP 참조·258 UI·81 도구)은 서로 중복되므로 독립 기능 총수로 해석하지 않는다.

미완료 328개를 전부 추출한 [개별 목록 CSV](e2e-pending-acceptance-20261124.csv)는 행 ID·기능명·분야·상태·누락 단계·출처·대응 진입점·기준 commit/검증표 해시를 포함한다. 원본 검증표의 328개 ID와 순서·개수의 일치를 확인했다. 이 파일은 위 소스 시점의 스냅샷이며 향후 수락 판정의 원장은 기존 JSON/CSV다.

| UI 분야 | 미완료 | 부분 근거 | 표상 미검증 | 남은 확인의 예 |
|---|---:|---:|---:|---|
| 기본 상태·인증·업데이트 | 8 | 0 | 8 | 웹 인증·서버 업데이트의 네이티브 대체 정책과 상태 표시 |
| 작업·실행·보관·목표·통계 | 53 | 33 | 20 | 실행·중지·강제 정리·배치 보관/복원·삭제·목표/제약·통계 |
| 작업 파일 | 7 | 0 | 7 | 실제 선택기·업로드/다운로드·수정 충돌·취소·재열기 |
| 자산·회사·귀속 | 16 | 14 | 2 | 검색·범위·관계·회사 귀속·삭제 후 참조 유지 |
| 취약점·재검증·증거 | 24 | 8 | 16 | 상태/등급 수정·재검증·심화·증거 연결·내보내기 |
| 실행 기록·그래프·트래픽 | 19 | 14 | 5 | 그래프 선택·이력·사용량·트래픽 검색/본문/삭제 |
| 공통 설정·알림·LLM 연결 | 22 | 15 | 7 | 실제 폼 저장/활성화·공간 전환·재시작·알림 실패/재전송 |
| 대화·첨부·기존 채팅·GC | 17 | 6 | 11 | 이름·고정·모델 변경·첨부·중지·삭제/복원·늦은 응답 차단 |
| 에이전트·프롬프트·트리거 | 21 | 14 | 7 | 생성/수정/삭제·버전/초기화·트리거·공유·재시작 |
| 도구·MCP·Python | 13 | 6 | 7 | 구성 폼·검색/갱신·외부 실행·실패/취소·세션 정리 |
| ScopeSentry | 5 | 1 | 4 | 상태·프로젝트/작업·유형 선택·명시 동기화·오류·재열기 |
| Skill·파일·가시성 | 16 | 1 | 15 | ZIP 가져오기·충돌 선택·메타/파일 편집·가시성·사용량 |
| 승인·규칙·질문·심사 | 24 | 15 | 9 | 규칙 우선순위·승인/거부·bypass 범위·중복/만료·사용자 질문 |
| 명령·LLM 기록·토큰 | 7 | 5 | 2 | 기록 상세·원문·검색/페이지·삭제·모델별 사용량 |

| 도구 분야 | 미완료 | 대상 |
|---|---:|---|
| 도메인·에이전트 | 51 | TOOL01–51: 자산·사실·발견·목표·계획·작업 공유/제어·증거·구성 |
| 일반 실행·파일·메모리·계획 | 21 | TOOL52–74 중 완료된 Read/WebFetch 제외 |
| PTY 종료·입력 | 2 | shell_close, shell_send |
| 대기·웹 검색 | 2 | sleep, web_search |

현재 제한된 수락 11개는 작업 생성, 단일 작업 보관 복원, 보고서 조회의 특정 사례, LLM 프로필 삭제, 대화 생성/전송, Read, WebFetch, shell_list/open/read다. 이들 역시 해당 기능의 모든 입력·실제 공급자·OS·대용량·오류 조합까지 완료했다는 뜻은 아니다.

미완료 행에서 근거가 비어 있는 단계는 실제 입력 325, 명령 수락 140, 실행 120, 저장 140, 재열기 198, 표시 322개다. 한 행이 여러 단계에 포함되므로 합산하지 않는다. 각 수락은 정상·경계·오류·취소 시나리오에 대해 입력→공개 명령 수락→실제 효과→저장→재열기→실제 표시가 연결된 근거를 남긴다. 읽기/거부/대체 기능은 불필요한 쓰기를 만들지 않고 저장 불변·재열기 일관성과 예상 거부를 검증한다. 하나의 연결 시나리오가 여러 명령을 다룰 수 있으므로 328개 별도 시험 파일을 만들 필요는 없다.

### 숫자 밖에 남은 확인·구현 과제

1. **원본 API 대응 2건:** `POST /api/assets`의 수동 자산 등록과 `POST /api/companies/reattribute`의 회사 재귀속. 관련 도메인 변이·에이전트 도구가 있다는 사실만으로 수동 네이티브 업무 대응을 인정할 수 없다. 현재 원본 참조 행은 `대응 확인 필요`다. 실제 입력·명령·저장·표시 경로를 확정하거나 별도 명시적 범위 결정을 받아야 하며, 합의 없이 제외하지 않는다.
2. **별도 원본 참조 12행:** side question의 대화/작업/의도별 GET/POST/DELETE, side events/cancel, 로그 이력이다. 여러 원본 행이 동일 네이티브 핸들러를 공유한다. 328개에 독립 기능처럼 합산하지 않고 각 범위의 문맥 고정·취소·스트림·재열기·표시를 연결 확인한다. 현 폼 카탈로그의 route 등록 개수와 원본 참조 행의 개수는 서로 다른 단위다.
3. **명시적 변경:** Rust 통합 앱·SQLite·공통 설정·Brevduva 구조 참고·영문 프롬프트·승인/bypass·명시 재개·삭제/정리·UI 개선은 유지한다. 웹 인증/서버 자가 업데이트를 원래 방식으로 복원하는 것이 수락 목적이 아니다. 네이티브 대체 정책의 정상/예상 거부·재시작·표시로 변경점 통과 근거를 작성한다. 네이티브 추가 기능은 원본 339개 목록만으로 누락 없이 관리할 수 없어 별도 요구사항 수락 추적도 필요하다.
4. **데이터·실사용 내구성:** SQLite 이전/원본 보존·CAS·관계·WAL 복구의 기존 시험과 실제 기록 복사본 이전 근거는 있다. 최종 release에서 대용량 작업 공간의 열기/이전·지속 스트리밍·장시간 실행·메모리/디스크 증가, 디스크 부족/접근 실패/손상 시 오류와 복구, 전체 작업 공간/Blob·공통 설정 백업 복원, 실제 절전/재부팅 후 보존을 추가 수락한다. 프로세스 종료 시험을 물리적 전원 장애 시험으로 계산하지 않는다.
5. **실제 공급자·외부 도구:** 최신 배포는 로컬 공급자 fixture로 검증했다. 지원 대상으로 정한 실제 API 구성에서 일반 대화→도구 호출→결과 반영→보고서→재시작을 확인한다. 외부 도구/PTY·사용자 MCP·검색·Python의 실제 설치/미설치/권한 부족/취소를 시험하며 사용자 운영 사이트나 실제 키를 무단 사용하지 않는다.
6. **OS별 남은 구현:** `host/src/execution.rs`와 `pty.rs`는 Unix의 전체 자손 정리 완료를 확인하지 않으며 종료 leader의 회수와 구분한다. macOS/Arch에서 정리·삭제까지 보장할 수 있도록 해당 경계를 해결하고 이탈 자손과 PTY/MCP를 실기기에서 검증해야 한다. Linux `host/src/power.rs`는 suspend 통지 예산을 4초로 고정해 실제 logind 지연 예산과의 대조·반영 및 절전 복구 검증이 남아 있다. 현재 값을 임의로 늘리는 방식으로 해결하지 않는다.
7. **OS·배포 수락:** Windows 일반 사용자/새 프로필·비개발 PC·DPI/한국어 입력·파일 선택·트레이·자격 증명 재부팅·실제 절전·재실행·외부 자원 정리를 확인한다. 과거 간헐 자격 증명 시험 실패는 최신 재실행 성공과 원인 해소를 구분한다. macOS Intel/Apple Silicon, Arch X11/Wayland에서 같은 소스의 시험·빌드와 실제 창/입력/키 저장/종료를 확인하고 각각 배포 패키지·런타임/고지·무결성을 검증한다. 과거 버전 CI를 이번 소스 수락으로 대체하지 않는다.

### 현재 배포 근거와 안정판 전환 순서

`v0.2.0-preview.5`는 서비스 45·호스트 79·UI 153·공통 회귀 459개 시험 근거를 갖는다(묶음 간 중복 포함). 공통 459건은 같은 날 전체 실행 후 입력 183개 해시가 일치해 최종 보고서에서 재사용했고, 서비스/호스트/UI 및 실제 창은 최종 소스에서 재실행했다. 최종 실행 파일 자체시험·23개 화면·아이콘·정상 종료·런타임/고지 수집·대응 소스·ZIP 왕복·공개 다운로드 해시 검증을 통과했다. 이는 개별 원본 기능의 전체 수락과 별개의 배포 근거다.

기준 소스와 [게시 릴리스](https://github.com/froiibn/ARTEX-Desktop/releases/tag/v0.2.0-preview.5)의 바이너리 SHA-256은 `21ad9e3bd1620008f0e9ef4a378b15345a2cafb47405a2849c5ad60980731dba`다. 최신 배포 사실은 이 항목·릴리스·BUILD-INFO를 따르며 STATUS의 이전 로컬 빌드 해시/배포 미실행 기록은 당시 증거다. 최종 검증 보고서는 `desktop/native/test-results/3edded735f91426b95f37d231cac68d2/verification-result.json`, 패키지/조립 보고서는 `desktop/dist/packages/0.2.0-preview.5/package-result.json` 및 `desktop/dist/releases/v0.2.0-preview.5/release-result.json`에 있다. 사용자 데이터 없는 업로드·익명 다운로드 근거는 저장소 밖 `.organization/release-preview5-20261124`에 보존한다.

권장 순서:

1. 현재 시험 근거를 행별로 대조하고 유지/명시적 변경/대응 미확정을 분리한다. UI·도구의 입력/표시·재열기부터 연결하며 기존 계층 시험을 단순 합산해 완료 처리하지 않는다.
2. 작업 수명주기·권한·공통 모델/키·에이전트 공유·SQLite·보고서의 핵심 사용자 흐름과 실패/취소/복구를 먼저 수락하고, 나머지 UI/76도구 및 추가 참조를 빠짐없이 확인한다. opt-in/기본 미제공 도구의 정책을 수락을 위해 해제하지 않는다.
3. 위 OS 코드·장시간/부하·실제 공급자·깨끗한 환경·플랫폼별 패키지 과제를 해결하고 관련 코드 변경의 회귀를 재검증한다.
4. JSON/CSV와 증거를 갱신해 `node scripts/validate-e2e-feature-matrix.mjs --require-complete`가 통과해야 한다. 2026-11-24 이번 확인에서 일반 목록 검사는 통과했고 이 엄격 검사는 328개 잔여로 종료 1이다. 이 검사는 목록/근거 구조 gate이며 별도 API·플랫폼·추가 기능 수락의 대체물이 아니다. 현재 package/full regression 검사는 엄격 gate를 자동 실행하지 않으므로 안정판 전용 배포 조건에 이를 연결해야 한다.
5. 지원 OS와 알려진 제한을 명시하고, 수락한 고정 commit에서 회귀·패키지·소스·고지·체크섬·다운로드를 재확인한 뒤 안정판을 게시한다. 코드 서명·설치 프로그램·자동 업데이트는 개별 배포 정책으로 결정하며, 미구현이라는 이유만으로 일괄 필수 결함으로 추가하지 않는다. Windows 판정만으로 macOS/Arch까지 안정판이라고 표시하지 않는다.

## 2026-11-24 안정판 핵심 흐름 1차 수정·수락

위 준비 현황 이후 사용자 지시에 따라 실제 연결 시험과 결함 수정을 진행했다. 기준은 `ad9a20c1f1ea46580b6624ebd40c192a2d388569` + 현재 작업 트리다. 원본 manifest 287개 파일의 SHA-256이 모두 일치하며 원본은 수정하지 않았다. 제품 입력 201개의 해시를 저장소 밖 `.organization/stable-core-20261124/frozen-product-inputs.json`에 고정했다. 기존 준비 문서와 328행 CSV는 당시 스냅샷으로 보존한다.

### 확인한 결함과 수정

| ID | 재현 | 원인·최소 수정 | 검증 |
|---|---|---|---|
| SC01 | 실제 NativeApp 명령 준비에서 `agent.features.set`가 `Only registered user commands can be prepared`로 거부 | RPC 실행부에 있던 사용자 전용 명령을 Prepare의 동일 agent planner에 연결. 최초 feature 행 생성에도 대상 agent 버전을 CAS read-set에 포함 | 실제 서비스의 준비/저장, 동일 receipt 재전송의 버전 불변, agent actor의 변경 거부, 중간 설정 변경 뒤 오래된 요청의 VERSION_CONFLICT |
| SC02 | 유효한 opt-in·활성 도구를 모델에 제공하고 질문이 저장되어 대기했지만 일반 대화에는 답변 위젯이 없음 | 기존 owner/version-aware 질문·승인 위젯을 대화의 스크롤 이력에 연결. 선택한 대화/작업·현재 실행 epoch의 활성 run만 표시하며 읽기 전용 제출을 차단 | 실제 질문/도구 승인 입력→원래 user.answer Prepare→모델 재개→저장/재열기/표시. 다른 대화 격리·선택 실행 표시·중지 뒤 늦은 답변 거부·읽기 전용 위젯 무효과 |

기본 opt-in 비활성, 도구 enabled/binding, 명시 deny, plan/scope와 사용자 actor 권한은 유지했다. 계약·저장 스키마·의존성을 변경하지 않았다. SC01은 통합 앱에 정의된 사용자 기능 동의 프로토콜의 연결 보완이고, SC02는 원본 `norma/tool/ask.go`의 질문 대기→사용자 답변→계속 실행 의미를 네이티브 UI에 연결하는 수정이다. SDK 질문 기능 설정 폼 전체나 OS 키 입력 UI를 수락한 것은 아니다.

재현 로그는 `.organization/stable-core-20261124/reproduction-index.json`에 해시와 함께 보존했다. 최초 시험 하네스의 모듈 접근·새 창 측정/제목 hit-test, opt-in/도구 설정/정책 API 입력 구성과 예상 오류 코드 오류는 제품 결함 수나 통과 수에 포함하지 않는다.

### 실제 연결 수락 사례

시험은 별도 임시 프로필·SQLite 작업 공간, 메모리 SecretRef, loopback HTTP 공급자와 실제 NativeGateway를 사용한다. 실제 egui 위젯의 포인터·키 입력, NativeApp Prepare/목적/소유권, 실제 서비스 실행 결과와 SQLite 재열기·표시를 연결한다. 결과를 화면 projection에 주입하거나 도메인 commit으로 UI를 대신하지 않는다. 외부 사이트·실제 사용자 키·사용자 작업 공간은 사용하지 않았다.

| 사례 | 수락 행 | 실제 확인 범위·제한 |
|---|---|---|
| 모델 수정·저장·기본 활성화·공간 전환 | UI149, UI151 | 이름 TextEdit의 실제 키 입력, 저장 전 CAS, 저장/기본 지정 버튼, 전환 취소/확인, A/B 모델 응답, 동일 SQLite 읽기 전용 재열기·모델/답변 표시. API 키·주소 등 다른 필드는 fixture이며 OS 파일 선택기는 미검증 |
| 작업 제어·중지·삭제 | UI023, UI024 | 실제 재개/일시정지·typed 중지, 보류 모델 요청 취소, 다시 열기만으로 모델 재실행 없음, 실제 목록 메뉴/영향 조회/확인 후 삭제, 다른 작업 보존, 읽기 전용 재열기·표시. 이 사례에는 외부 도구 자손 프로세스를 시작하지 않았다 |
| 질문 도구와 답변 | TOOL52 | 명시적 기능/도구 opt-in 후 실제 제공 스키마 확인, 모델 호출, 현재 대화 질문·다른 대화 격리·선택 실행 화면, 답변 선택/제출, 다음 모델 요청의 답변 반영, 저장/재열기/표시 |
| 승인과 파일 도구·Bypass | TOOL74 | 승인 전 HostBridge 호출/파일 효과 0, 승인 뒤 정확히 한 번 Write, 실제 Read 결과가 모델과 화면에 표시, Bypass 실제 토글 뒤 명시적 deny의 Write 차단, 파일/이력 재열기, 새 공간으로 Bypass 전파 없음 |
| 응답 중지·늦은 답변 | UI174 | 실제 승인 대기 상태에서 응답 중지, run/question cancelled, 종료 상태 표시, 늦은 user.answer VERSION_CONFLICT·호스트 효과 0, 읽기 전용 재열기 후 질문/작업 자동 재생 없음 |

UI023은 사용자가 요청한 typed 중지·정리 확인을 포함하는 네이티브 작업 삭제 업무의 **변경점 통과**다. 기존 웹 API의 전송/반환 경로를 이 시험에서 호출한 것으로 기록하지 않는다. UI024는 실제 표시된 pause/resume와 관련 중지 흐름의 사례다. 각 통과는 기재한 입력/시나리오의 수락으로, 모든 조합과 실제 외부 공급자·OS·자손 정리 보장까지 확대하지 않는다.

Bypass `ui.setSettings`의 연결 사례와 `ui.saveTool`, `ui.createInterceptRule`, `ui.interceptSetToolConfig`의 실제 공개 설정/효과 근거도 해당 행에 추가했다. 일반 설정 전체나 설정 폼 입력을 대체하지 않으므로 이 네 행은 부분 수락으로 유지한다. `agent.features.set` 및 `user.answer` 등의 추가 native 프로토콜 근거는 원본 339개 목록에 새 행을 임의로 추가하지 않고 이 사례와 별도 회귀에 연결한다.

| 대상 | 제한된 연결 경로 통과 | 미완료 |
|---|---:|---:|
| UI 258개 | 11 | 247 |
| 도구 81개 | 7 | 74 |
| 합계 339개 | 18 | 321 |

7행의 근거를 추가했고 현재 미완료는 부분 근거 205개·표상 미검증 116개다. [최신 잔여 321개 CSV](e2e-pending-acceptance-core-20261124.csv)는 기준 commit과 dirty 상태·제품 입력/검증표 해시를 함께 기록한다. 과거 328개 CSV는 `ad9a20c` 기준의 정리 시점 기록으로 유지한다. 최신 JSON/CSV 원장을 검증한 일반 inventory gate는 통과했다. 엄격 gate와 원본 전체 동등성·안정판 수락은 아직 통과하지 않았다.

### 회귀·빌드 근거와 남은 범위

| 검사 | 현황 | 범위 |
|---|---|---|
| 원본 manifest | 287개 해시 일치 | 원본 출처 보존 확인 |
| 계약 | 1,283개 통과 | 기존 54table/571field·261route·258UI·81tool 유지 |
| 앱 서비스 | 45개 통과 | 실제 서비스/공통 설정/기존 native host 연결 |
| 호스트 | 79개 통과 | 기존 외부 프로세스·파일·네트워크 경계 |
| 네이티브 UI | 159개 통과 | 기존 153개 + 새 핵심 연결 5개 + 읽기 전용 질문 위젯 1개 |
| 공유 workspace | 최종 460개 통과 | 최초 1건 실패를 제외하고 기존 결과 재사용 없이 전체 재실행 |
| 실제 native 창 | 23개 화면·정상 종료 통과 | 중복 실행 전달·실행 파일/창 아이콘도 확인. 사용자 OS 입력 조작의 수락을 대신하지 않음 |
| 표준 실행 파일 | 빌드·해당 EXE 자체시험 통과 | 격리된 SQLite/프로필·로컬 공급자 요청 4회·승인/Bypass·도구·CAS·보관/복구·정상 종료 |

최초 전체 실행의 공통 회귀는 `unlimited_turns_make_a_real_model_request_past_one_hundred_while_finite_budget_wraps`의 로컬 HTTP 서버에서 Windows `WouldBlock`으로 실패했다. 단독 재실행 성공만으로 해결 처리하지 않았다. 같은 Rust 표준 라이브러리의 격리 probe에서 nonblocking listener의 수락 소켓을 그대로 읽으면 첫 바이트를 50ms 지연한 조건에서 즉시 실패하고, 명시적 blocking 모드에서는 정상 수신함을 확인했다. 해당 시험 서버에만 `set_nonblocking(false)`를 추가했으며 기존 5초 소켓/20초 요청 한도와 제품 코드는 바꾸지 않았다. 수정된 기존 턴 예산 시험과 이후 전체 회귀가 통과했다. 이는 제품 결함 2건과 구분한 시험 fixture 수정 1건이다.

서로 겹치는 묶음은 합산하지 않는다. 최초 실행은 `desktop/native/test-results/dbd473da85c145c7b8fc2ad454e76bdd`, 보완 후 통과한 실행은 `desktop/native/test-results/8137cdc6d9854af99d6fd88ca8c1c2fa`에 보존한다. `.organization/stable-core-20261124`의 초기/최종 제품 입력 manifest와 재현 로그가 각 소스·결과를 구분한다. 최종 전체 검증의 소스/리소스 250개 해시와 원본 287개 파일 해시를 확인했다. 일반 수락표 검사는 18/339로 통과했고 엄격 gate는 잔여 321개로 종료 1이다. 표준 실행 파일 식별은 아래 완료 결과를 따른다. 이번 작업은 기존 GitHub preview.5 릴리스 교체나 안정판 게시를 포함하지 않는다.

표준 빌드와 해당 EXE의 격리 자체시험은 종료 코드 0으로 완료했다. 실행 파일은 `desktop/dist/ARTEX-Windows/ARTEX.exe`, 64,271,269바이트, SHA-256 `93c0395a8ec66dde5f121f2f956b82c020a3ec709db5be44c57e8266a5a707fb`다. BUILD-INFO는 `ad9a20c` 기반 dirty 소스의 로컬 개발 빌드이며 배포 준비 완료를 뜻하지 않는다. 자체시험의 로컬 공급자 요청은 4회이며 실제 공급자 인증/모델 동작 검증으로 확대하지 않는다. 실행 기록·자체시험 JSON·최종 증거 목록은 저장소 밖 `.organization/stable-core-20261124/verification.json`에 연결했다.

남은 핵심 수락은 실제 키 저장/재부팅과 설정 폼 전체, 외부 도구 자손 실행까지 연결한 중지·정리/삭제, 완료된 사이트 분석·발견/증거·전체 보고서 내보내기의 동일 흐름, 대용량/장시간·오류 복구 및 나머지 321행이다. 원본 수동 자산 등록/재귀속 2건, 별도 원본 참조 경로, Unix 자손 정리·Linux 실제 절전 예산, macOS/Arch 실기기·패키징 과제도 그대로 남아 있다. 이 제한을 이번 7행 수락으로 해결했다고 간주하지 않는다.

## 2026-11-26 완료된 분석과 보고서 내보내기 연결 수락

기준은 동일한 `ad9a20c1f1ea46580b6624ebd40c192a2d388569` 기반 dirty 작업 트리다. 11월 24일 변경을 유지하고 이번에는 `desktop/native/src/app_site_report_e2e_tests.rs`와 OS 저장 대화상자 경계인 `desktop/native/src/app.rs`만 코드 변경했다. 제품의 rfd 선택·취소 동작은 유지하며 테스트에서만 thread-local 큐로 선택 결과를 공급한다. UI/서비스/저장/파일 capability를 대체하지 않는다. 새 제품 결함은 발견하지 않아 업무 로직·계약·저장 형식·의존성을 수정하지 않았다. 원본 manifest 287개 해시도 일치한다.

### 연결 경로와 수락 범위

실제 NativeApp 재개 클릭과 원래 Prepare/CAS→제공된 도구를 호출하는 loopback 모델→실제 NativeGateway Read→report_finding→그 반환 노드 ID/증거 버전으로 update_finding_report→goal_met의 즉시 종료→SQLite 조회→보고서 표시→실제 내보내기 버튼→원본 Body 스테이징/SaveFile→저장 취소→다른 작업 선택→같은 SQLite 읽기 전용 재열기/보고서/파일 저장을 하나의 사례에서 실행했다. 실제 모델 판단/웹사이트 스캔의 품질 검증은 아니다.

| 행 | 판정·확인 범위 |
|---|---|
| UI138 ui.report | 기존 변경점 통과에 완료 상태·실제 도구·발견/증거/개선안·P1·미검증 안내·파일 저장/취소·재열기 근거 추가 |
| TOOL19 goal_met | 명시적으로 planner에 바인딩한 호출→작업/실행 종료→보고서의 종료 상태/호출 기록→재열기 표시 |
| TOOL38 report_finding | 모델의 실제 호출→확인된 파일 관찰과 함께 발견/노드 저장→보고서의 요약/증거→재열기 표시 |
| TOOL49 update_finding_report | 실제 반환 NodeId/근거 버전→상세 개선 원문 저장→보고서/내보낸 파일/재열기의 동일 원문 |

목표는 공개 ui.addGoal로 준비하고 설정도 공개 명령으로 수행했다. 이 목표 생성 폼·도구 설정 폼·자동 reporter fan-out의 입력 수락으로 확대하지 않는다. 별도 reporter 트리거는 공개 설정에서 끈 단일 planner 사례다. goal_met와 보고서 도구의 기본 바인딩을 제품에서 변경하지 않았다. OS 저장 대화상자 자체, 대용량/다중 페이지, 실제 공급자·외부 사이트·사용자 키/작업 공간·macOS/Arch는 범위 밖이다.

네 개 도구의 실제 완료·저장 결과, 로컬 모델 요청 4회·Read 호스트 호출 1회를 확인했다. goal_met 이후 별도 마지막 모델 요청을 요구하지 않는다. 출력 파일 바이트는 표시한 전체 보고서 원문과 일치하며 취소는 성공 알림을 추가하지 않는다. 다른 작업 보고서에 이 발견이 섞이지 않고 재열기/조회/내보내기는 모델을 재실행하지 않는다.

실제 호스트가 있는 정상 종료는 native.lifecycle.cleanup 영수증을 저장한다. 종료 전 모든 객체 보존·추가 객체가 이 정리 확인 영수증 1개뿐임·확인 상태 true·세대 증가 1을 검사했다. 보고서는 자신의 고정 조회 세대를 표시하므로 뒤의 현재 UI 세대와 동일하다고 가정하지 않는다. 재열기 보고서는 이 조회 세대 문구만 달라지며 분석·증거·개선 원문은 같다. 임의 내용을 정규화해 누락을 가리는 비교를 사용하지 않는다.

이 사례의 추가 통과는 도구 3행이다. UI 258개 중 11개, 도구 81개 중 10개로 **21/339개 제한 수락, 318개 미완료**다. 318개는 부분 근거 202개·표상 미검증 116개이며 결함 318건을 뜻하지 않는다. [최신 잔여 CSV](e2e-pending-acceptance-report-20261126.csv)를 추가했고 11월 24일의 328/321행 CSV는 당시 스냅샷으로 보존했다. 전체 동등성·안정판 전환은 여전히 미수락이다.

### 회귀와 실패 로그의 구분

연결 사례의 최종 단독 실행은 `D:/Artex/.organization/stable-report-20261126/connected-pinned.log`다. 초기 fixture의 역할·명령·DTO·요청 수·종료 영수증/조회 세대 예상 오류는 제품 결함으로 계산하지 않으며 로그를 보존했다. Windows PowerShell 5.1의 Get-FileHash 모듈 탐색 실패도 제품 검증 이전의 환경 실패다. 설치된 pwsh로 실행했다.

첫 전체 UI 실행은 CI의 `RUST_TEST_THREADS=1`을 누락한 기본 병렬 실행이었다. 152개 통과·기존 8개 대기 시간 초과 실패였으며 새 보고서 사례는 통과했다. 이 실행은 전체 통과로 세지 않는다. 실패 근거는 `desktop/native/test-results/e48404d90a15430089c53159803ee003`에 보존했다. CI와 같은 단일 테스트 스레드 조건의 재실행은 UI 160개 전부 통과했다. 제품/fixture 시간 제한을 늘리지 않았다. 이 재실행이 병렬 부하 안정성을 입증하지 않는다.

공유 코드/계약/manifest 입력 183개의 해시 일치를 검사해 이전 전체 공유 회귀 460건만 재사용했다. 서비스·호스트·UI·실제 격리 native 창은 다시 실행했다. 최종 통과 실행은 `desktop/native/test-results/b4020e7bf64e421ba7a20aa080aff8dd/verification-result.json`이다. 실행 소스/리소스 250개 해시를 확인했고, 최종 연결 증거는 저장소 밖 `.organization/stable-report-20261126/connected-acceptance-final.json`에 기록했다.

| 검사 | 결과·근거 |
|---|---|
| 원본 manifest / 계약 | 287개 해시 일치 / 1,283개 스키마·예제·저널 검증 통과 |
| 서비스 / 호스트 | CI 단일 테스트 스레드에서 45건 / 79건 새로 통과 |
| 네이티브 UI | 160건 새로 통과. 위 기본 병렬 실패는 제외 |
| 공유 workspace | 460건 이전 통과 결과 재사용. 현재 183개 입력 해시 일치·누락 검사 통과 |
| 실제 격리 native 창 | 23개 화면·실행 파일/창 아이콘·중복 실행 전달·정상 종료 통과 |
| 통합 자체시험 / 기능 의존성 | debug 실행 파일 자체시험 통과 / 별도 엔진·WebView 제외 확인 |
| 표준 실행 파일 | build.py 최적화 빌드 및 해당 EXE 자체시험 통과. 로컬 공급자 요청 4회·승인/Bypass·도구·저장·복구·정상 종료 |

서로 겹치는 시험 묶음을 합산하지 않는다. 증거·제외 목록·고정 입력은 저장소 밖 `.organization/stable-report-20261126`에 보존하며 커밋·푸시·태그·릴리스 교체는 수행하지 않는다.

현재 실행 파일은 `desktop/dist/ARTEX-Windows/ARTEX.exe`, 64,273,200바이트, SHA-256 `e6a0be027d81c3a976d9aa624bab94d53a1e305f5533f6ca759c947e196349ca`다. BUILD-INFO는 `ad9a20c` + dirty의 로컬 개발 빌드이며 `distribution_ready: false`다. 자체시험의 종료 코드는 0이다. 최종 실행·해시·수락표/CSV 대응·원본 보존·실패 제외 근거는 저장소 밖 `.organization/stable-report-20261126/verification.json`에 연결했다. 318개 수락 행과 위에 명시한 추가 경계가 남아 있어 안정판 게시를 수행하지 않았다.


## 2026-11-27 전체 318행 추가 수락 완료

현재 최종 결과는 **339/339행, 잔여 0**이다. [최종 보고서](acceptance-final-20261127.md)에 수정 묶음·의도적 변경·최종 새 회귀·실패 제외·표준 실행 파일과 남은 출시 범위를 통합했다. [완료 행/사례/해시 CSV](e2e-acceptance-completed-20261127.csv)와 원래 전체 JSON/CSV의 개별 근거를 따른다. 과거 328/321/318행 잔여 CSV와 위 시점 기록은 당시 결과로 보존한다. named-operation 수락과 원본 Go 직접 차등/전체 물리 플랫폼 출시 수락은 구분한다.
