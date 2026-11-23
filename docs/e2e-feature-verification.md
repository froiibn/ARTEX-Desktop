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
