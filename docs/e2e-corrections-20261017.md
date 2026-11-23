# 종단 간 검증 기준에 따른 수정·회귀 검증

기준: `a585764e9a88f5b3e4f39c5a787e4080b6b4c610` + 시작 당시 작업 트리. 기존 F/R 수정·원본·고정 과거 보고서는 보존한다. 먼저 [원본 기능별 검증표](e2e-feature-verification.md)와 261 경로/258 화면 명령/81 도구 목록을 작성한 뒤 제품을 수정했다. 명시적 추가·개선 18개 항목은 같은 문서의 유지 목록에 기록했다.

## 변경 원인과 보존한 의미

| ID | 수정 | 경계·보존 사항 |
|---|---|---|
| V01 | archive_wire에서 Source의 공개 숫자 통계 요약을 projection | 내부 version 2 finding/profile/day/skill 통계와 패키지·원장은 그대로 보존. 새 패키지의 토큰/취약점 합계를 공개 DTO로 정확히 투영하며 빈 통계로 오류를 숨기지 않음 |
| V02 | deleteLLMProfile 결과의 deleted를 실제 profile typed ID 계약으로 정합 | 원본 numeric→typed ID라는 승인된 변경에 맞춤. generator 규칙 2줄/정확한 DTO·overlay만 수정, 원본 ui-dtos 및 source manifest 유지. 숫자 성공값을 만들지 않음 |
| V03 | 출력 상한 max_tokens/length의 확정 답변 뒤 원본 resume nudge를 기록하고 계속 | 원본 harness와 같은 최대 3회 이어쓰기. 모델명별 예외·동일 요청 재생성 없음, 복구는 같은 논리 turn이며 실제 각 HTTP 사용량/부분 답변 보존. 본문 없는 유효 상한도 controller로 전달 |
| V04 | Responses delta/item을 조립하여 terminal output 누락/[]를 보충 | 실제 관측만 사용, 완전한 terminal snapshot 우선, 잘린 도구 JSON 실행 금지, 이미 관측한 출력이 있으면 빈 snapshot을 재생성 근거로 사용하지 않음 |
| V05 | SSE event 이름을 Responses discriminator로 사용하고 unnamed는 JSON.type fallback | 원본 우선순위 유지, 멀티라인/CRLF/UTF8 framing과 잘못된 event JSON의 거부 유지 |
| V06 | Anthropic 미캐시+cache read+creation을 총 input으로 정규화 | JSON/SSE/실패 관측 경로 공통. Chat/Responses는 이미 총합인 input에 캐시를 중복 가산하지 않음 |
| V07 | 완료된 Chat/Responses 도구 호출의 빈/공백 인자를 {}로 해석 | 없는 인자와 미완성 비어 있지 않은 JSON을 혼동하지 않음. 잘린 JSON은 실제 도구 실행 전에 실패 |
| V08 | 실제 run/task 마감에서 온 TIMEOUT을 provider health/failover보다 먼저 settlement 처리 | 실제 실패 호출/사용량은 기록하되 자체 예산 소진으로 공급자 실패·backup credential 전환·재시도를 만들지 않음. 미래 마감의 실제 전송 실패/취소 대조 유지 |
| V09 | PTY drain은 유지 중인 요청 이후 전체 출력을 반환하고 반환한 만큼 cursor 전진 | 원본 drain 의미. non-drain pagination/omitted·expired cursor·ownership·외부 종료 증명 유지 |
| V10 | WebFetch RECON은 주석·script src·inline endpoint·form·meta·link를 압축 출력 | 원본 endpoint/form/comment 의미 및 dedup/UTF8/최종 출력 예산 유지. 긴 script 전체 복사로 뒤의 보안 신호를 밀어내지 않음 |
| V11 | 긴 선행 행을 bounded reader로 완전히 소비하여 offset에 도달 | 원본 2000바이트 행 preview를 UTF8 경계에서 보존, 64KiB 결과 예산·binary/invalid text·capability/취소 경계 유지 |
| V12 | 없는/삭제·보관된 source 및 없는 profile 선택을 초안에 표시하고 명시 해제 제공 | snapshot 갱신으로 초안을 조용히 폐기하지 않음. 다른 선택/이름/목표·고급 옵션은 유지하고 해제된 ID만 생성 명령에서 제거 |
| V13 | 양수 days는 원본 rolling timestamp, 비양수는 각 조회의 원본 기본 기간 | hot 행은 timestamp로 먼저 필터, Source cold 집계는 원본처럼 UTC daily bucket으로 병합. profile 전역 누계와 원장 과거 ID를 바꾸지 않음 |
| V14 | Content-Type dispatch만 ASCII 소문자로 정규화 | 보고된 원문 header 유지. HTML 변환/RECON과 static asset 거부도 동일한 case-insensitive dispatch 사용 |

## 확장 검증에서 발견한 추가 연결 결함

| ID | 실제 재현과 원인 | 최소 수정·재검증 |
|---|---|---|
| V15 | 실제 native Service의 `shell_open`이 ExecutionId(`exec_`)를 세션 ID로 반환해 다음 `shell_read`의 ShellId 계약(`shell_`)에서 거부됨. 기존 host 직접 호출 시험은 이 공개 경계를 지나지 않아 놓침 | 세션 ID와 실행 원장 ID를 분리. 동일 실행의 재시도는 같은 세션·spool을 반환하고 run/epoch/capability/config 충돌은 거부. monitor/kill은 원장 ExecutionId 유지. 수정 전 실패 로그 보존 후 같은 모델→open→read 경로와 cleanup 검증 |
| V16 | 새 알림 채널에도 기존 채널의 전송 대상 변경 시 자격 증명 교체 규칙을 적용해, 인증이 필요 없는 정상 email 설정을 거부 | 생성에서만 기존 자격 증명 재사용 guard를 제외. 업데이트 guard·masked 값·SecretRef 검사 유지. 생성/이름변경/재열기와 잘못된 masked 값·대상 변경의 거부 대조 |
| V17 | 실제 goal 간선이 있는 graph/nodes/lineage 조회가 `{from,to,rel}`를 반환하여 고정 UI EdgeDTO `{src,dst,rel}` 결과 계약에서 거부 | 공통 UI projection만 교정. 저장 키 `src_id/dst_id` 및 도구의 distilled graph projection은 유지. 실제 공개 생성→간선 있는 조회→재열기와 native 조회 버튼→결과 tree 표시 검증 |
| V18 | 회사가 실제 검색되는 `chatMentions` 응답에서 typed ID가 남아 있는 numeric ID 계약과 충돌. 빈 검색 결과만으로는 드러나지 않음 | 실제 finding/company/asset 종류의 ID만 허용하는 결과 override와 생성 규칙을 정합. 잘못된 종류·형식·numeric ID의 거부, 실제 비어 있지 않은 검색·페이지·재열기 대조 |

도구 `graph_overview/get_task_graph`의 raw graph 반환 의심은 실제 공개 dispatch가 `tool_projection::query→overview`를 사용함을 확인해 결함에서 제외했다. 원본과 같은 목표·힌트·frontier·finding·coverage·related task 요약을 검사하는 업무 assertion을 보강했다. 직접 내부 함수만 보고 제품 경로의 결함으로 판정하지 않았다.

새 비자명 로직에는 원본 심볼/불변식을 설명하는 주석을 붙였다. 저장 schema version·원본 81 입력 계약·외부 패키지·일반 빌드 의존 그래프는 유지했다. 실제 App 시험의 HostGateway 구현에서 기존 로컬 artex-runtime을 직접 참조하기 위해 native manifest의 dev-dependency와 lock의 루트 의존 간선만 추가했다. 해당 시험용 두 파일 외 Cargo manifest/lock은 시작 해시를 유지하며 일반/빌드 그래프는 변경 전후 바이트가 같다. 기존 자료에 덮어쓰는 데이터 패치는 하지 않았다.

## 재발 방지를 위한 검사 연결

- `rpc/src/e2e_parity_tests.rs`: 실제 HTTP 공급자→controller→usage/history→checkpoint/reopen, 상한/정상·반복 한도·terminal 축약·자체 마감/실제 실패 대조.
- `rpc/src/domain_ui_e2e_tests.rs`: 실제 공개 명령 admission/결과 계약→writer→archive worker→조회/reopen. 내부 plan 성공을 UI 성공으로 계산하지 않음.
- `host/tests/e2e_host_boundaries.rs`: 실제 ConPTY·파일·로컬 HTTP 효과와 오류/정리 대조.
- `native-core/tests/e2e_host_paths.rs`: UI와 같은 typed Service 명령→실제 NativeGateway/HostBridge→결과 Blob/transcript→close/reopen. 비밀 해석만 fixture이며 OS 자격 증명 저장소를 건드리지 않음.
- native `task_creation` 시험: 실제 egui 선택 해제/생성 효과와 다른 초안 값 유지.
- native `app_workflow_e2e_tests`: 실제 App의 원래 Form/Panes 효과→pinned read/CAS/Prepare→서비스→완료 처리→같은 작업 공간 닫기/재열기→실제 egui 표시. 삭제 클릭 직후 동시 수정에서도 원래 CAS가 유지되어 충돌하고 초안이 보존되는 부정 대조를 포함한다. Windows headless 1600×6000 fixture이며 실제 OS 창 배치 수락 검사는 아니다.
- `rpc/src/tool_e2e_tests.rs`: 개별 도구의 공개 호출·CAS·업무 값·Blob·checkpoint/reopen·중복 요청 원자성. 모델 offer/실제 UI 클릭까지 검사한 것과 구분.
- macOS/Arch CI의 storage-only 검사를 shared workspace 전체로 확장하여 provider/RPC 회귀도 실행하도록 했다. Windows는 기존 `verify.ps1 -FullRegression`에 포함된다. CI 실행/배포를 이번 로컬 검사로 대신하지 않는다.
- `scripts/validate-e2e-feature-matrix.mjs`: 원본 목록 누락·중복과 통과 행의 단계별 근거를 검사한다. `--require-complete`는 정적·계층 시험만 있는 항목을 안정판의 전체 기능 통과로 세는 것을 막는다.

## 실패 기록과 검증 근거 교정

첫 전체 실행 `61660a4d2f1d460e98b84957e48243c4`는 강화한 그래프 coverage assertion에서 실패했다. 원본 `agent/planner.go`의 SetOwnerNode, `agent/tools_insert.go`의 anchorOwner, `db/task_assets_context.go`의 tested 집계를 대조한 결과, fixture의 service는 기본 origin fact에 연결되므로 전체 2개 중 1개 검증됨(0.5)이 맞다. 이 값은 현재 구현에 맞춰 임의 변경한 기대가 아니라 고정 원본에서 확인한 기준이다. 제품을 바꾸지 않고 시험 기대와 근거 주석을 교정했다. 관련 source task에는 실제 열린 intent가 있음을 먼저 검사하여 빈 데이터로 공유 범위 검사가 통과하지 않게 했다. 첫 실패 로그와 원본 대조 근거를 보존하며 그 실행을 성공 집계에 넣지 않는다.

초기 외부 UI fixture는 실제 위젯 효과를 가져왔지만 공개 명령을 재생할 때 새 CAS/owner/epoch를 만들었다. 따라서 원래 UI 봉투 전달을 확인하는 전체 경로 근거로는 부족했다. 이를 부분 경로로 낮춘 기록을 남기고, 앱의 실제 Prepare/poll/완료 처리를 사용하는 위 시험으로 보강했다. 서로 다른 graph 조회/표시·재열기 fixture도 전체 경로 통과로 합산하지 않는다. 검증표의 전체 경로 통과에는 단일 `connected_path` 근거가 필요하다.

실제 App 시험의 초기 두 시도는 fixture의 보관 전 pause 누락과 pause 인자 `taskIds` 오기 때문에 실패했다. 제품 변경 없이 공개 계약에 맞게 fixture를 교정했으며 각 source/hash/log를 보존했다. 이와 별도로 초기 보관 fixture 실패 두 건의 raw 로그가 담당자의 재실행으로 덮어쓰인 사실은 시도 이력에 명시했다. 소실된 로그를 복구했거나 그 시도가 통과했다고 주장하지 않는다.

두 번째 전체 실행 `d8c813cbb0304d969db427d9cc9dda94`에서는 실제 App 시험 준비의 일반 pause가 runtime 버전 충돌로 거부됐다(UI 121 통과/1 실패). 공개 작업 생성은 첫 intent가 없어도 running 상태를 만들며, 준비 중 시간 집계가 계속돼 task_runtime 버전이 바뀌었다. 시험의 context 작업은 생성 직후 실제 `작업 중지·정리` 버튼으로 공개 RunControl을 완료하고 paused/queued=false/활성 clock 해제를 확인하도록 교정했다. 원래 버전 보호·목표 생성/삭제/복원·충돌 대조는 유지했다. 제품을 바꾸거나 충돌을 무시·자동 재시도하지 않았으며 두 번째 전체 실패 로그도 보존한다.

추가 App 공급자/본문 시험의 준비 오류도 제품 결함과 구분했다. 완전한 retry 객체가 필요한 공개 설정에 일부만 보낸 입력, Chat 전용 관리 drawer를 다른 화면에도 가정한 진입, 기본 닫힘인 결과 scalar를 펼치지 않은 표시 검사, 내부 host의 capability 보강 이후 입력을 원본 도구 입력 `{}`와 혼동한 기대를 고정 계약·실제 호출부에 맞춰 교정했다. 작업 폴더 안에 준비한 파일도 첨부 capability 없이 절대 경로로 요청하면 정상 거부된다. 해당 양성 시험은 실제 허용된 상대 경로를 사용하며 제품의 경로 보호를 완화하지 않는다.

도구 이력은 원문 부분문자열만 찾지 않고 해당 대화의 `kind=tool_result`와 실제 `tool_use_id`로 선택한다. 본문 전체 복사는 private transcript envelope가 아닌 공개 tool-result 평문 JSON과 실제 host 원문을 대조한다. Store의 실행 종료 직후 한 frame만으로 비동기 history 표시를 단언하던 검사는 실제 App의 자연 조회·poll·glyph 표시 완료를 제한 시간 안에 기다리도록 했다. 수동 새로고침·캐시 주입·결과 합성으로 성공시키지 않는다.

표시 검사에서는 글꼴의 논리 여백과 실제 그려진 행의 잉크 경계를 구분했다. PTY 진단은 같은 TextShape의 두 연속 행에 `PTY_HEAD_`와 `SENTINEL`이 soft wrap된 사실도 확인했다. 시험 helper가 각 행 사이에 실제 없는 개행을 넣어 실패시킨 것이므로, 실제 glyph를 연결하되 hard newline과 화면 밖 glyph의 대체 문자는 보존하도록 교정했다. galley 원문 전체·서로 다른 Shape·숨겨진 행을 이어서 표시 성공으로 세지 않는다. 이전 helper 원문과 해당 좌표 로그도 별도 보존했다.

PTY 열기 자체의 표시도 drain 결과에서 추론하지 않고 해당 activity와 실제 반환 SessionId로 별도 확인한다. 출력 관측은 실제 retained cursor 범위에서만 수행하고, 휠 이동은 실제 repaint 위치가 정착한 뒤 다음 영역을 확인한다. 모델 요청에 실제 도구 정의가 제공됐는지도 함께 검사한다. 각 시도의 로그·당시 소스를 저장소 밖에 보존하며 실패·시간 제한 시도를 통과 수에 넣지 않는다.

## 검증 결과

최종 Windows 전체 실행 **`c023d6f2bce54d7f95b50afd1193856e`**은 `RUST_TEST_THREADS=1`, Cargo `--locked`로 통과했다. [고정 실행 보고서](../desktop/native/test-results/c023d6f2bce54d7f95b50afd1193856e/verification-result.json)에 각 명령·exit code·소스와 실행 파일 SHA-256을 보존했다. 최초 전체 실패 실행과 targeted fixture 실패는 이 통과 집계에서 제외했다.

| 실제 검사 | 통과 시험 수 |
|---|---:|
| 네이티브 서비스(단위+실제 Service 통합) | 27 |
| 기본 legacy client 없는 호스트 | 77 |
| 네이티브 UI | 129 |
| 공통 workspace 전체 회귀 | 410 |

이 시험군은 같은 소스를 중복 검사하므로 합산한 숫자를 기능 수로 제시하지 않는다. 빌드·기능 의존성 확인·격리 통합 self-test·실제 23개 화면 렌더링·실행 파일/창 아이콘·정상 종료·두 번째 실행의 정확한 read-only 요청 GUI 수신도 통과했다. 두 번째 실행에서 실제 사용자가 전환 대화상자를 수락하는 경로까지 자동화한 것은 아니다. 네이티브 창 캡처는 [검증 이미지](../desktop/native/test-results/c023d6f2bce54d7f95b50afd1193856e/native-window.png)에 있다.

계약 검사는 1,279개 스키마 컴파일/예제 검사를 통과했다. 원본 54개 표·571개 필드·261개 경로·258개 화면 API·81개 도구의 고정 목록을 유지한다. 원본 287개 파일·원본 DTO/도구 목록과 네이티브 시험용 두 파일을 제외한 Cargo manifest/lock은 작업 시작 해시와 같다. 실제 App 시험에서 기존 로컬 artex-runtime을 직접 참조하도록 native dev-dependency와 lock의 루트 의존 간선만 추가했다. 외부 패키지·버전·기능은 추가하지 않았으며 일반/빌드 의존 그래프는 변경 전후 바이트가 같다. 공개 결과 계약 수정은 W353.deleted와 W340.id/종류별 ID 상관 제약으로 한정했고 두 계약 사본은 바이트가 같다. 전체 실행 전 동결한 소스·스크립트 241개 및 실행 보고서 입력 222개의 SHA를 대조했다.

[개별 검증표](e2e-feature-matrix.v1.json)의 현황은 {"출처 대응": 259, "대응 확인 필요": 2, "미검증": 132, "부분 경로 통과": 198, "변경점 통과": 1, "E2E 통과": 8}다. 261개 HTTP 행은 업무 출처이며 독립 실행 성공 수가 아니다. 화면 명령+도구 339행 중 **9행만** 실제 입력부터 같은 작업 공간 재열기·표시까지 연결된 제한 시나리오 근거가 있다. 나머지 **330행은 전체 경로 수락 미완료**이며, 부분 경로 성공은 기능 결함 수와 같지 않다. 일반 목록 검사는 통과했고 `--require-complete`는 이 미완료 항목 때문에 의도대로 실패한다. 전체 회귀 통과를 원본 전체 동등성이나 안정판 완성으로 해석하지 않는다. 339행 gate는 필요조건이며, 별도 legacy UI로 노출된 원본 참조 12행 및 수동 관리 API insertAssets/reattribute 두 행의 대응·실행 확인도 남는다. 원본 웹 호출부를 찾지 못했다는 이유로 이 두 API를 통과시키지 않았다.

별도 targeted 근거는 실제 App 9시험(기존 선택/삭제/복원·CAS 충돌 2, 공급자/사용량/health 5, 파일·HTTP·PTY 2), 공개 domain RPC 5시험, 공개 도구 56개 정상+3개 거부(2시험), 실제 Service/host 3시험과 provider/HTTP/controller 경계 시험을 포함한다. 이들도 위 전체 실행과 겹치므로 합산하지 않는다. 실제 App의 현재 소스·로그 근거는 method-evidence-v8.json과 native-app-final-evidence.json, 도구 56개 최신 근거는 tool-e2e-summary-v2.json이다. 새 App 시험은 실제 composer 입력·전송→로컬 공급자→host/원장/사용량→같은 공간 재열기→본문 표시와 큰 Blob의 실제 읽기 ticket·전체 복사까지 확인한다. 사용량·풀·상세 조회의 일부 입력값은 시험이 초안에 seed했으며, 실제 날짜/조회 필드 타이핑까지 검사했다고 주장하지 않는다.

원본 Go 실행·macOS/Arch 실기기 빌드/창·실제 LLM 서비스/ScopeSentry/외부 메일 발송은 실행하지 않았다. 사용자의 앱·키·실제 사이트·작업 공간에 접근하지 않았다. CI의 공통 회귀 확대는 소스에 반영했으나 GitHub에서 실행하지 않았다. 커밋·푸시·태그·릴리스 변경도 하지 않았다. 저장소 밖 근거 묶음 `../.organization/e2e-vfixes-20261017`의 result.json/verification-reference.json과 각 실패·성공 로그를 함께 보존한다.

## V19 기존 작업 공간 열기 지연

2026-10-18 사용자 재현에서 네이티브 서비스가 Opening에 오래 머물렀다. 이 절은 위 V01–V18의 고정 검증 뒤에 추가한 별도 수정 기록이다. 당시의 사용자 데이터 미접근 설명도 해당 과거 검증 범위로 보존한다. 이번에는 실행 중인 앱을 관찰하고 실제 작업 공간의 로컬 복사본으로 열기를 재현했으며, 원본 저장 자료를 직접 패치하지 않았다. 대상 HEAD는 `a585764e9a88f5b3e4f39c5a787e4080b6b4c610`이며 이전 작업의 미커밋 변경은 유지했다.

원인은 `Store::apply_payload`가 저널 frame마다 전체 Snapshot과 tombstone을 복제하고 `validate_snapshot`을 반복하던 것이다. 격리 복사본의 최신 체크포인트에는 활성 레코드 58,092개와 삭제 흔적 913개가 있었고 이후 저널은 3,547 frame이었다. 그중 3,546개는 신규 로그 추가, 1개는 변경 없는 명령 영수증이며 모두 event/delivery가 없었다. 실행 중 스레드 스택에서도 replay → apply_payload → validate_snapshot → validate_record → 정규식 생성이 확인됐다. writer 파일 잠금 대기로 오판하거나 원본 잠금을 삭제하지 않았다.

제품 수정은 `desktop/crates/store/src/lib.rs`와 새 `replay_logs.rs`에 한정했다. 현재 모델에서 참조·추가 고유성 제약이 없는 신규 `server_logs` 추가와 명령 영수증에만 증분 재생을 적용한다. envelope/secret·schema·ID·버전·순번·중복·삭제 ID 재사용과 영수증 검증을 모두 마친 뒤 적용하여 잘못된 frame의 일부만 남지 않게 한다. 미래 모델에 관계·고유성 제약이 추가되면 기존 전체 검증 경로를 사용한다. 수정/삭제·event/delivery·관계 객체도 기존 경로를 유지한다. 체크포인트 검증, 저널 해시·CRC·순번, 최종 그래프·Blob 검증, 명령 중복 처리, 불변 조회 세대와 writer/CAS 보호를 유지했으며 저장 버전·공개 계약·외부 의존성은 바꾸지 않았다. checkpoint 강제 생성이나 저널 삭제로 증상을 숨기지 않았다.

최종 소스에서 Cargo `--locked`, 단일 시험 스레드로 Store 30개와 실제 Service 열기/재열기·읽기 전용·CAS/영수증 보존 및 Opening 중단 fence 시험 각 1개가 통과했다. 신규 Store 회귀 4개는 기존 전체 검증과의 결과 비교, 기존 레코드 복제 방지, 12종 잘못된 frame의 원자적 거부, 수정/삭제/관계 frame의 기존 경로, 2,000개 로그 저널 뒤의 잘못된 frame 및 읽기 전용 재열기를 포함한다. 읽기 전용 코드 검토에서 발견한 미래 영수증 제약 보호도 최종 소스에 반영했다.

초기 실패는 통과로 계산하지 않았다. 시험 fixture의 Rust borrow 오류와 Windows byte-range lock 보유 중 재읽기 실패를 각각 교정하고 이전 소스·로그를 보존했다. 수정 debug 빌드의 실제 창 검사는 기존 90초 마감에 걸려 실패했다. 최적화 빌드의 첫 읽기 전용 실행은 23개 화면을 표시했지만 Store-only 복사본에서 생략한 `resources/work` 때문에 `Unknown directory capability`가 발생해 실패로 기록했다. 원본 폴더 구조와 work 파일 193개를 해시 검증하여 두 격리 복사본에 보완했으며 제품 권한 보호나 마감 시간을 완화하지 않았다.

최종 최적화 실행 파일로 보완한 읽기 전용 복사본은 11.291초, 독립된 일반 모드 복사본은 37.169초에 실제 23개 화면·스크린샷·오류 없음·정상 종료 검사를 통과했다. 이는 시작부터 모든 화면 순회와 종료까지의 관측 시간이며 열기만의 정밀 벤치마크는 아니다. 실행 파일 SHA-256은 `535e24e29018af9f6c96e255f8e5b0843c07f3934258e7edeaa012436d8f1c2f`다. 원본과 읽기 전용 복사본의 저장/Blob 896개 및 work 193개 파일은 모두 해시가 유지됐다. 일반 모드의 정상 bootstrap·복구·저장은 독립 복사본에서만 일어났다. 사용자 API 키를 해석하거나 실제 모델·사이트·도구 실행을 요청하지 않았다.

이번 근거는 저장소와 작업 공간 열기 경로의 한정 검증이다. 이전 전체 회귀를 새 소스의 전체 통과로 재사용하지 않으며 원본 전체 동등성·안정판·macOS/Arch 실기기를 보장하지 않는다. 원본 데이터·진단 프로필·화면은 Git에 추가하지 않는다. 저장소 밖 `../.organization/open-loading-20261017-ccdf3f96`에 실패/성공 로그·당시 소스·데이터 해시·실행 결과를 보존하며 커밋·푸시·릴리스 업로드는 하지 않았다.

## V20–V23 사용자 시험 피드백 4건

2026-10-20 사용자가 누적한 네 항목의 구현을 승인했다. HEAD `a585764e9a88f5b3e4f39c5a787e4080b6b4c610` 및 기존 미커밋 수정은 보존한다. 원본·저장 형식·RPC 계약·의존성·Cargo lock을 변경하지 않고 네이티브 UI와 그 회귀 시험에 한정했다. 저장소 밖 근거 묶음은 `../.organization/test-feedback-20261020-88707427d15149a69640e32b7855cc36`다.

| 항목 | 코드에서 확인한 원인과 변경 | 판정 범위 |
|---|---|---|
| V20 첫 대화의 `name is required` | 현재 `ui.createConversation`은 `name`을 받지 않는다. 빈/공백 제목은 기본 제목을 쓰며 수정 전 최적화 실행 파일의 새 시험 공간에서도 생성이 성공했다. 별도로 이전 설정·에이전트 생성의 이름 오류가 성공한 대화 생성 뒤에도 남는 전역 오류 표시 경로를 확인했다. 새 foreground command는 이전 실패를 발생 작업과 함께 Activity에 보존하고 새 요청의 오류 표시를 소유한다. 실패에는 실제 발생 명령을 표시한다. 필수 이름 검증을 제거하거나 `title`을 `name`으로 바꾸지 않았다. | **사용자 사례의 직접 원인은 미확정**이다. 확인한 잔류·귀속 문제만 수정했다고 보고하며 현재 생성 정상 경로와 실제 이전 이름 오류 경로를 검증한다. |
| V21 중앙 대화 정렬 | 생성 폼·메시지·입력창을 메인 영역의 같은 중앙 읽기 열에 배치했다. 최대 860 논리 픽셀의 UI 설계 폭을 쓰고 좁은 창은 가용 폭을 유지한다. 기존 입력 높이 측정·첨부·전송·전사 스크롤을 재사용한다. | 사용자가 명시한 UI 변경. 460/1000/1800 폭의 실제 egui 도형·클립·중앙 좌표와 기존 최소 창/하단 drawer 회귀를 검사한다. |
| V22 보이지 않는 목록 체크박스 | 기존 공통 테마의 inactive 테두리 없음과 패널색 fill 때문에 비선택 아이콘이 배경과 같았다. 실제 목록 Checkbox에만 범위가 제한된 배경·외곽선·선택 표시를 적용했다. | 사용자가 명시한 UX 변경. dark/light·idle/hover·선택·disabled 도형·대비·실제 클릭 토글과 부모 스타일 보존을 검사한다. 전체 버튼에 사각 프레임을 복원하지 않았다. |
| V23 분석 종료 식별 | 작업 상태·개별 실행 상태·도구 상태가 흩어졌고 명령 응답을 일률적으로 `완료`라고 표시했다. 명령은 `요청 반영`으로 구분하고 작업 상태 카드·최근 작업 요약 및 대화 최신 root 응답 상태를 표시한다. 승인/질문 대기·진행·마무리·완료·실패·시간 종료·중지/일시정지·이전 epoch 복구를 구분하며 관측한 종료 전이는 로컬 상단 알림과 Activity에 남긴다. 처음 열기와 같은 저장 세대는 반복 알림을 만들지 않는다. | 빈 실행 목록·개별 worker 완료·발견 사항 0개를 전체 분석 성공으로 해석하지 않는다. 대화 응답 완료와 전체 task 분석 완료는 별개다. 남은 자식 실행 및 외부 자원 정리 미확인은 숨기지 않는다. pause/stop이 같은 저장 상태를 사용하는 경우 영구적으로 구별했다고 주장하지 않는다. |

`analysis_status.rs`는 저장된 scope/epoch·task·run·질문·도구·활동만 읽는다. 가짜 진행률·완료 결과·새 모델 호출·호스트 정리 성공을 만들지 않으며 Blob 본문을 UI 스레드에서 읽지 않는다. 상태 카드가 최소 창의 작업 탭을 밀어내던 회귀를 발견해 기존 제어/탭 다음에 상세 카드를 배치했다. 대화 헤더의 상태는 메시지 입력 영역과 함께 중앙 열을 따른다. 전이 알림은 현재 세션의 로컬 UI 기능이며 외부 메일·OS 보안 설정을 변경하지 않는다.

실제 App 회귀는 이전 `ui.createAgent`의 `name is required`를 원래 Prepare/Service 경로에서 발생시킨 뒤 실제 대화 생성 버튼으로 빈/공백/일반 제목 대화를 저장·선택·재열기·표시한다. 에이전트 생성의 필수 검증은 여전히 실패하며 대화 생성만으로 LLM을 실행하지 않는다. 별도 실제 입력창/전송 시험은 loopback 모델을 기다리는 동안 실행 중 표시를 확인하고 응답 저장 후 완료/상단 알림, 다음 HTTP 500 응답 후 실패 표시, 같은 공간 재열기 후 과거 알림 미재생을 확인했다. 사용자 키·실제 사이트·사용자 작업 공간은 이 시험에 사용하지 않았다.

초기 실패는 보존하고 통과에 합산하지 않았다. `chat-column-v1`은 기존 제목의 centered sized-label glyph 위치를 열의 왼쪽 좌표로 오해한 시험 오류였으며 생성 폼의 왼쪽·입력창 중앙·폭/클립·메시지 좌표 검증은 유지했다. 첫 전체 UI 실행은 144개 중 141개 통과/3개 실패였다. 한 건은 위 최소 창 task 탭 회귀로 제품을 수정했고, 두 건은 egui가 아이콘 전에 그리는 투명 AtomLayout frame을 checkbox 아이콘으로 선택한 시험 오류였다. 실제 `icon_width` 정사각형을 정확히 하나 식별하도록 고쳤으며 fill/stroke/대비/클릭 기대값은 완화하지 않았다. 최적화 self-test 최초 호출은 GUI subsystem 실행 파일을 직접 호출하여 exit code가 null이었으므로 성공 종료 근거로 쓰지 않는다. 새 격리 공간의 두 번째 실행은 Process handle·WaitForExit·실제 보고서를 함께 확인했다.

최종 Windows UI 회귀는 **145개 통과/0개 실패**(229.58초)다. 최적화 빌드와 그 실행 파일 자체의 비대화형 통합 self-test도 통과했다. self-test는 실제 로컬 공급자 요청 4회, 실제 native 도구, 81개 등록, 명령/파일 CAS, 승인·bypass·중지/정리 증거·삭제/복원·저장/재열기·정상 종료를 검사한다. 해당 실행 파일 SHA-256은 `6dbf74ce19351de584e0f70654385f295f2a21ec63edee5015f3dfceeb00b395`다. 별도 feedback App 회귀 2개는 전체 145개와 겹치므로 합산하지 않는다. 최종 native 입력 해시가 빌드·시험 후에도 유지됨을 대조했다. Store/Host/전체 shared suite를 새로 실행했다는 주장은 하지 않는다.

개별 검증표에는 `ui.createConversation`과 `ui.sendConversationMessage`의 실제 App 근거를 추가했다. 생성은 시험이 준비한 제목·프로필 입력과 실제 버튼 클릭의 제한 시나리오이며, 전송은 실제 입력창 TextEvent와 전송 클릭을 쓴다. 창 자동 조작 수락과 구분한다. 이 시나리오 추가로 전체 경로 수락은 **10/339행**, 나머지 **329행**은 미완료다. 원본 전체 동등성·안정판 통과로 해석하지 않는다. 최종 근거는 외부 묶음의 `result.json`, `native-ui-final-v4.log`, `release-self-test-v2-result.json`, `final-native-input-hashes.json` 및 실패 로그를 함께 따른다.

사용자가 Esc로 Computer Use를 중단한 뒤 **“화면 조작은 중단하고 나머지 검증 계속”**을 선택했다. 이후 데스크톱 입력·창 자동 조작·GUI smoke 캡처를 재개하지 않았다. 수정 전 격리 창은 관찰했지만 **수정 후 실제 OS 창의 시각 수락 검증은 미실행**이다. 실제 egui 도형·클릭/서비스/저장 경로 시험 및 비대화형 통합 실행과 구분한다. Windows 한정 결과이며 macOS/Arch 실기기·실제 외부 LLM·전체 원본 기능 동등성을 보장하지 않는다. 커밋·푸시·릴리스 업로드는 하지 않았다.
