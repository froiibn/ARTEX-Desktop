# 전체 318개 수락 작업 결과 · 2026-11-27

기준 소스는 `ad9a20c1f1ea46580b6624ebd40c192a2d388569`와 기존 변경을 유지한 작업 트리다. 원본 ARTEX는 비교 입력으로만 사용했다. 기존 21행에 남은 318행을 더한 UI 258개·도구 81개의 수락 결과를 기록한다. 서로 겹치는 API/도구 목록을 독립적인 제품 기능 수로 합산하지 않는다.

## 완료 범위

| 대상 | 완료 | 잔여 |
|---|---:|---:|
| UI 명령 | 258 | 0 |
| 도구 | 81 | 0 |
| 전체 | **339** | **0** |

기존 21행을 새로 재검증하고 남은 **318행을 추가 수락**했다. 네이티브 대체 의미의 변경점 통과는 39행이다. [전체 JSON](e2e-feature-matrix.v1.json)·[전체 CSV](e2e-feature-matrix.v1.csv)·[이번 완료 행/사례/해시](e2e-acceptance-completed-20261127.csv)에서 행별 여섯 단계·전체 사례의 실행 로그·소스 해시·표시 범위와 제약을 확인할 수 있다. `node scripts/validate-e2e-feature-matrix.mjs --require-complete`의 판정은 339/339, 잔여 0이다.

각 행의 통과는 실제 NativeApp 입력·확인/Prepare·정책과 버전 검사 → 실제 서비스/도구 실행 → SQLite/Blob 또는 네이티브 파일 저장 → 같은 작업공간 재열기 → 해당 결과의 실제 egui 표시가 연결된 사례에 한정한다. 조회는 비어 있지 않은 저장 자료·범위 필터·정확한 결과 필드로 확인했고, 삭제는 대상 제거와 다른 작업/권한/파일 보존을 확인했다. 등록 여부, 단위 시험, 성공 접두사만으로 행을 승격하지 않았다.

원본 웹 서버의 로그인·전역 선택·목록 폴링·파일 전달·서버 자체 업데이트는 이미 합의한 단일 Rust 네이티브 앱의 해당 업무 경로로 판정했다. 이런 행은 `변경점 통과`와 실제 대체 경로를 기록했다. 호출하지 않은 원본 RPC나 HTTP 요청을 실행한 것으로 기록하지 않았다. 보관 복구의 ID 재발급은 기존 tombstone/anti-ABA 설계에 따른 변경이며, 내용·소유권·관계와 출처 ID의 일관성을 검증했다.

## 수정한 문제

| 묶음 | 문제 | 수정 의미 |
|---|---|---|
| A01 | 트리거 소유권·CAS | agent_key 기준 저장 조건을 재열기 시 불러오고 버전 검사 포함 |
| A02 | 삭제 결과 ID 계약 | 원본 Go가 반환하는 assetrule/trigger/conversation 객체 ID를 개수가 아닌 typed ID로 선언 |
| A03 | nullable 폼 값 보존 | 그리기만 해도 null이 빈 문자열로 바뀌던 동작 수정 |
| A04 | 신규 사용자 도구 생성 | 기존 시스템 도구만 saveTool, 새 도구는 createCustomTool |
| A05 | 회사·작업 범위 표시 | kind별 canonical domain/net/value 필드 사용 |
| A06 | 실제 UI 접근 누락 | taskScope/taskConstraints/frontier/activity/audit/trafficDeleteHosts/GC/health 실제 관리 메뉴 노출 |
| A07 | PTY 새 명령 대기 | 이전 유휴 시간을 새 명령 quiet 간격으로 재사용하지 않음 |
| A08 | 증거 연결 해제 | immutable 증거 본문/해시/헤더는 유지하고 unreferenced_at 수명주기만 허용 |
| A09 | 탐색 노드 설명 표시 | payload.summary/text를 kind 앞에 표시 |
| A10 | 도구·자산 식별 표시 | 원본과 같이 도구 key+설명, 자산 canonical domain/IP/app 식별값 표시·검색 |
| A11 | MCP 공개 목록 계약 | 공개 DTO name/description 정렬, 실행용 schema는 내부 캐시에 보존 |
| A12 | ScopeSentry 네이티브 경계 | 이미 typed 인자인 operation을 params에 다시 주입하던 동작 제거 |
| A13 | 자기 일시정지 결과 보존 | 활성 snapshot의 동일 CAS 트랜잭션에 취소/실행 영수증/대화/감사 결과 저장 |
| A14 | GC·알림 시험 Prepare 등록 | 존재하던 effect handler를 UI command registry에 등록 |
| A15 | 네이티브 업데이트 정책 | 서버 자가 교체 대신 버전/패키지 링크·명시적 미구성 거절, 데이터 보존 |
| A16 | 첨부의 모델 전달·Read 허가 | 첨부가 저장/표시만 되고 모델에 누락; admission Body를 BlobRef로 오독하던 경로 수정 |
| A17 | 발견 사항의 영향 자산 연결 누락 | report_finding의 원본 priority/origin/evidence와 중복제거된 영향 자산 anchor를 같은 CAS 계획에 복구 |
| A18 | 후속 탐색의 자산·출처 누락 | deepenFinding payload와 anchors에 원본 출처 ID/자산을 전달; 기존 empty-anchor 데이터는 명시적 후속 동작에서만 복구 |
| A19 | 승인 실행 상태 DTO 불일치 | 저장 상태를 유지하고 공개 상태만 completed→succeeded/error→failed로 매핑 |
| A20 | SDK ExecutionId를 TaskId로 잘못 검사 | TaskOutput/TaskStop 입력 핸들에만 좁은 예외; Host run/epoch 소유권은 유지 |
| A21 | Skill 성공 영수증의 도구·감사 연결 손실 | 표준 transcript_batch를 재사용해 결과·지침·audit을 같은 원자적 완료에 보존 |
| A22 | paused Worker 메시지와 연속 이력 누락 | 원본처럼 독립 continuation run/userturn을 정리확인/CAS 뒤에만 저장; 같은 task/intent 부모 이력을 순서대로 전달; typed NodeID 결과 계약 복구 |
| A23 | TaskStop 빈 문자열 의미 | 원본의 명시적 빈 문자열=자기 run 전체중지 입력을 계약/생성규칙에 복구; 다른임의문자열/TaskID는 여전히 거절 |
| A24 | MCP 삭제 후 고아 접근 권한 | 실제 즉시 삭제 뷰에서2개 고아권한 재현; 같은 계획에서 해당 mcp의 권한만 삭제하고 다른서버/캐시/CAS 의미 유지 |
| A25 | Skill 삭제 후 이름 기반 권한 잔류 | 실제 파일 삭제/읽기전용 재열기 뒤 enabled/disabled2행 재현; Prepare pin/preflight/성공한 동일name 완료에만 같은 원본CAS로 권한정리 |
| A26 | 보관 일괄 결과의 ArchiveId 계약 | ArchiveBatchItem.archive_id 숫자 선언을 native typedarchiveID로 변환; 원본T38정의는보존 |
| A27 | 작업 제약 목록 식별 텍스트 | task_constraints.text를 좁은컬렉션 조건으로 목록·검색에 노출, 명시적이름/첫줄/100Unicode제한 보존 |
| A28 | 자산에 연결된 탐색 참조 DTO | genericnode 대신 원본 CoverageAssetRef conciseDTO; 모든종류 id는 graphNode typedID |
| A29 | 일괄 일시정지와 자동 시간 집계 버전 충돌 | running/live/known task의 derived clock bookkeeping만 같은writer의 현재CAS로원자계산; UI읽기버전 및다른객체/복구/재개CAS와 logicalreceipt 재시도보존 |
| A30 | 후속 탐색 출처 ID의 보관·이전 재매핑 | source_finding_id/source_finding_node_id를 원본 numeric/native import와 portable archive복구의 기존 ID 매핑에 포함; literal text·evidence·package bytes는 그대로 보존 |

신규 라이브러리나 런타임 패키지 버전을 올리지 않았다. 네이티브 테스트에서 이미 존재하는 로컬 Domain/Store 크레이트와 기존 잠금 버전의 ZIP을 직접 참조하도록 개발 의존 간선만 추가했다. 사용자 작업공간·실제 API 키·운영 사이트를 수정하거나 검사 입력으로 사용하지 않았다.

### 일괄 정지의 시간 집계와 버전 검사

`ui.controlTasksBatch`의 일시정지는 자동 시간 집계가 갱신하는 `task_runtime` 때문에 정상 입력도 버전 충돌로 실패했다. UI의 읽기 버전을 새 값으로 바꾸거나 자동 재시도하지 않는다. 네이티브 단일 writer는 실행 중·유효·집계 상태 known인 대상의 시간 행에만 현재 집계 버전을 내부 의존성으로 사용한다. 계획의 시간 행 변경은 현재 내용의 `last_active_clock` 해제뿐이어야 하며 마지막 시간 분율과 남은 예산을 같은 트랜잭션에 계산한다.

작업·실행·의도·그 밖의 설정 버전, 시간 행 ID 집합, 누락·다른 작업·0(부재)·미래 버전, 일시정지 상태와 복구/재개 명령은 기존 검사를 유지한다. 내부 영수증의 `$artex_writer_clock_v1`에는 이때 분류한 시간 행 ID만 남긴다. 재시도는 그 행의 분류 당시 writer 버전 이하인 양수 관찰 버전만 같은 논리적 명령으로 비교하며 입력·ID·순서·다른 버전은 정확히 일치해야 한다. 기존 영수증은 기존의 엄격한 비교를 따른다. 실제 집계 tick·부분 실패·예산·재열기·중복 효과 없음과 반대 경계를 시험했다.

Skill 삭제에서 호스트 오류·다른 세대·다른 리소스·충돌이 발생하면 권한 행을 임의로 제거하지 않는다. 늦은 오류 전 파일 효과가 이미 일어났을 수 있으며 파일 시스템 롤백을 보장하지 않는다. 불확실한 prepared 영수증과 재실행 차단을 유지한다. TaskStop의 accepted/stop_requested도 중지 요청 수락이며 실제 종료·출력 회수·정리 확인은 이후 TaskOutput/TaskList에서 구분했다.

## 최종 실행 근거

| 검사 | 결과 |
|---|---|
| 원본 출처 | 287개 해시 일치, 원본 미수정 |
| 계약 | 1,283개 스키마·정상/오류 예제·저널 바이트 검증 통과 |
| native-core | 45개 통과 |
| native-host | 81개 통과 |
| native-ui | 245개 통과 |
| shared-regression | 491개 통과 |
| 별도 알림 양성 | 1개 통과; 기본 UI 회귀에서 제외된 로컬 webhook opt-in 사례 |
| 격리 native 창 | 23개 화면, 아이콘, 중복 실행 전달, 정상 종료 통과 |
| 고정 입력 | 261개 소스/리소스, 검사 전후 SHA-256 일치 |

실행 근거: [공개 검증 요약·행별 해시](verification/acceptance-20261127.json), SHA-256 `6572ba52a08b283445c2526d9069a08a9daa709ba9e9089a0d657be8df08f969`. CI와 같은 `RUST_TEST_THREADS=1` 조건으로 서비스·호스트·UI·공유 workspace 전체를 새로 실행했고 과거 공유 회귀 결과를 재사용하지 않았다. `RUST_TEST_NOCAPTURE=1`로 실제 행별 결과 관찰도 보존했다. 별도 알림 로그와 before/after 입력 검증은 저장소 밖 `.organization/acceptance-all-20261126/config/notification-final-frozen-run3-proof.json`에 기록했다.

행별 연결 근거는 저장소 밖 `.organization/acceptance-all-20261126/connected-acceptance-final.json`과 저장소 안의 전체 JSON/CSV에 연결한다. 불필요한 사용자 데이터·프로필·비밀 저장소를 Git에 추가하지 않는다.

시험 묶음은 실행 경로가 겹치므로 합산하지 않는다. 헤드리스 시험은 실제 egui 위젯 이벤트와 서비스·저장·호스트를 실행하되 OS 파일 선택기의 반환 경로와 로컬 프로토콜/모델 응답을 fixture 입력으로 사용한다. 결과·성공 상태·스토어 영수증을 화면에 주입하지 않는다. 별도 격리 native 창의 23개 화면·아이콘·중복 실행 전달·정상 종료도 확인했다. 사용자 앱의 화면 자동 조작은 중단 지시를 유지했다.

알림 양성 시험은 해당 시험 자식 프로세스에서만 로컬 webhook 허용 환경을 사용하고 원래 환경으로 복구했다. 기본 회귀에서는 로컬 주소 차단 정책을 바꾸지 않았다. 실제 외부 수신자에게 알림을 보내지 않았다. 모델·검색·webhook 시험의 자격 증명은 격리한 합성 SecretRef와 메모리 해석기를 사용했다. 로컬 인증 헤더·HTTP 전달의 성공을 실제 OS 비밀 저장소나 외부 계정 인증의 수락으로 해석하지 않는다.

### 실패와 제외한 실행

수정 전 실제 실패, 잘못된 fixture·원본 DTO와 다른 시험 기대값·컴파일 오류 및 조정 중 중단한 자체 시험은 로그를 보존하고 최종 통과 집계에서 제외했다. 대표적으로 ScopeSentry의 원본 숫자 상태에 문자열을 넣은 fixture, 본문을 주지 않는 승인 실행 메타데이터 조회에 본문을 기대한 시험, PID를 반환하지 않는 Monitor에 PID를 기대한 시험, 전송 request ID를 재사용한 명령 재시도 시험을 제품 결함과 구분했다. 미검사 자산 시험은 fact anchor가 있는 service를 미검사로 기대한 오류를 고쳐, 실제 untested root_domain 포함과 tested service 제외를 함께 검사했다. Windows 검색 시험 서버는 nonblocking listener에서 받은 소켓을 명시적으로 blocking으로 전환해 HTTP 헤더가 분할 도착해도 기존 제한 시간 안에서 읽도록 했다. 두 경우 모두 제품 의미와 제한 시간·인증·실제 결과 검사를 유지했다. 정보 창 고지 시험은 기존 크기의 화면에서 실제로 스크롤하고 클리핑 영역 안에 표시된 문구를 관찰하도록 수정했다. 고지 링크나 화면 높이를 바꿔 시험을 통과시키지 않았다. 공유 도메인의 발견 사항 삭제 시험은 report_finding이 생성한 자산 연결을 raw fixture로 중복 삽입하던 부분을 제거하고 실제 연결의 유일성·삭제 CAS·연쇄 정리를 검사했다. 취소 시험에서만 사용하는 ScopeSentry 가짜 호스트가 실제 ui.stop 요청을 처리하지 못해 외부 스레드가 실패하던 문제도 수정했다. 기존 요청 시간 안에서 취소한 command·workspace·epoch와 실제 stop 호출이 일치하는지 확인하며, 외부 스레드 실패를 바깥 시험의 통과로 숨기지 않는다. 원본 계약이나 성공 조건을 완화하지 않았다.

## 표준 실행 파일

표준 출력은 `desktop/dist/ARTEX-Windows/ARTEX.exe`다. `python desktop/native/scripts/build.py`로 최적화 빌드했고 이 실행 파일 자체의 격리 `--self-test`와 PE 아이콘 리소스 검증도 통과했다.

- 크기: 63,692,735바이트
- SHA-256: `380dc8942ad58cc8fc52eac9de706bdd225404dc962d16a4c5c970d4accd8733`
- 자체 시험: 실제 로컬 공급자 요청 4회, 도구·승인/Bypass·CAS·저장·보관/복구·정리 확인·정상 종료
- 자체 시험 종료 코드: **0**. 실제 OS 프로세스 핸들을 유지하고 종료까지 기다려 확인했다. 실행 근거는 로컬 검증 기록에 보관했다.
- BUILD-INFO: `ad9a20c1f1ea46580b6624ebd40c192a2d388569` + dirty, `local-development-build`, `distribution_ready: false`

첫 검증 보조 스크립트는 PowerShell의 GUI 실행 파일 호출 직후 결과 파일을 읽어 일찍 종료됐다. 제품 자체 시험은 성공 파일을 남겼지만 그 실행의 종료 코드를 최종 근거로 사용하지 않았다. 같은 EXE 바이트를 새 격리 작업공간에서 다시 실행하고, 명시적 프로세스 종료 대기로 최종 성공을 확인했다.

일반 로컬 빌드이며 버전 태그·커밋·푸시·GitHub 릴리스 업로드는 이번 작업에서 수행하지 않았다. 이전 출력 복사본을 만들지 않고 표준 위치를 교체했다.

## 별도 출시 수락

339행은 기재한 명명된 UI·도구의 연결 시나리오 수락이다. 원본 Go 런타임 자체를 실행한 완전한 차등 시험, 모든 매개변수·오류 조합, 실제 외부 공급자/서비스 인증, 기본 다중 에이전트 전체 fan-out, 장시간·대용량·절전/전원 장애, OS 선택기·키 저장/재부팅, macOS·Arch 실기기·설치·서명은 별도 출시 점검이다. Windows의 결과로 다른 OS의 성공을 선언하지 않는다. Unix 자원 정리의 기존 unconfirmed 경계도 유지한다.

원본 HTTP 출처 261행 중 별도 수동 API 2개(`POST /api/assets`, `POST /api/companies/reattribute`)는 원래 웹 UI 호출부가 없는 경로로, 기존 별도 대응 기록을 유지했다. 이를 318행에 끼워 넣거나 완료로 계산하지 않았다. 목록/예외/validator를 변경해 통과 수를 늘리지 않았다. 최종 원본 전체 동등성·모든 플랫폼 안정판 게시를 이 제한된 행 수락만으로 선언하지 않는다.


## 게시용 기록

위 실행 파일 해시는 preview.6 버전 표기를 적용하기 전의 로컬 개발 빌드에 대한 고정 기록이다. preview.6 배포 파일은 같은 기능 수정에 버전 표기·컴파일 경로 제거·정적 런타임·정확한 소스 commit을 적용해 다시 패키징하므로 해시가 다르다. 실제 배포 commit·실행 파일·대응 소스 ZIP·고지·체크섬은 해당 릴리스의 BUILD-INFO.json과 SHA256SUMS.txt를 따른다. 공개 검증 요약은 비밀·프로필·원시 로컬 로그를 포함하지 않는다.
