# ARTEX 데스크톱 상세 구현 명세 v1.0

현재 앱 구조는 [Rust 통합형 데스크톱 앱 설계](desktop-native-architecture.md)를 따른다. 이 문서의 파일 저장·도메인·도구 계약은 유지하되 React·Tauri·별도 엔진 및 UI용 파이프 RPC 항목은 이전 구현 기록이다. 새 앱의 UI와 내부 명령은 Rust 타입·같은 프로세스의 채널로 구현한다.

2026-10-12 확정. Rust 엔진·Tauri 2 호스트·기존 React 화면을 구현하는 규범 명세다. PostgreSQL 및 SQLite 없이 객체 그래프와 파일 저장으로 기존 업무 기능을 제공한다. 이 명세의 확정은 Rust 구현이나 세 OS의 배포 검증 완료를 의미하지 않는다.

이 문서와 `contracts/`를 함께 적용한다. 충돌 시 이 문서의 명시적 변경 규칙, `tools.v1.json`의 데스크톱 스키마, 생성된 원본 계약 순으로 우선한다. 원본 Go 코드의 스키마·핸들러·DTO·상태 검증은 아래 변경 사항을 제외하고 호환 기준으로 고정한다. 기존 아키텍처의 미정 표현은 이 문서의 v1 값으로 대체한다.

## 고정한 원본과 산출물

| 산출물 | 내용과 사용 방법 |
| --- | --- |
| [source-manifest.v1.json](contracts/source-manifest.v1.json) | 기능 기준 Go·SQL·TypeScript 파일의 SHA-256. 구현 도중 원본 변경을 조용히 기준에 섞지 않는다. |
| [data-model.v1.json](contracts/data-model.v1.json) | 54개 테이블, 최종 추가 열을 포함한 571개 필드, NULL·SQL 선언·참조 삭제 정책·이전 규칙·데스크톱 필드 스키마. |
| [entities.v1.schema.json](contracts/entities.v1.schema.json) | 기존 업무 레코드의 전체 저장 DTO와 실행·이벤트·전달·명령 영수증 등 공통 DTO. |
| [legacy-routes.v1.json](contracts/legacy-routes.v1.json) | 261개 HTTP 경로의 RPC 대응·전환 분류·핸들러 위치·고정된 Go 처리 계약. 직접 등록 243개와 별도 등록 함수의 18개를 포함한다. |
| [ui-dtos.v1.json](contracts/ui-dtos.v1.json) | 258개 화면 API 함수의 인자·확장 DTO 314개·결과 형태·원본 변환 코드. 숫자 ID·비밀·시각은 아래 v1 변환을 적용한다. |
| [ui-wire-overrides.v1.json](contracts/ui-wire-overrides.v1.json) | 화면 함수별 v1 ID 종류를 명시한 대응 목록. rpc 계약의 W* DTO는 원본 숫자 ID를 그대로 받지 않고 실제 typed ID·본문 참조로 변환한 wire 스키마다. |
| [rpc.v1.schema.json](contracts/rpc.v1.schema.json) | 요청·결과·오류·이벤트 봉투, 신규 메서드 및 기존 화면/경로의 메서드 등록부. |
| [storage.v1.schema.json](contracts/storage.v1.schema.json) / [collection-registry.v1.json](contracts/collection-registry.v1.json) | project·manifest·변경 payload 및 신규 컬렉션의 저장 DTO 등록부. |
| [agent-message.v1.schema.json](contracts/agent-message.v1.schema.json) | 자체 내부 통신 봉투와 요청 상태. Brevduva의 구조만 참고한다. |
| [tools-source.v1.json](contracts/tools-source.v1.json) / [tools.v1.json](contracts/tools.v1.json) | 실제 Go 생성자로 추출한 81개 원본 입력 정의와 데스크톱 입력·핸들러·효과·바인딩 대응. |
| [81개 도구 대응표](desktop-tool-mapping.md) | 각 도구의 한국어 기능, Rust 구현 경로, 제공 조건, 소스 위치. |
| [journal-vector.v1.json](contracts/journal-vector.v1.json) | 변경 프레임의 바이트 단위 고정 예제, 해시와 복구 판정 벡터. |

주 SQL에는 49개 테이블이 있다. `db/commands.go`의 `llm_records`, `db/llm_usage.go`의 `llm_usage`, `traffic/traffic.go`의 `exchanges`, `exchange_bodies`, `blob_refs`를 포함해야 54개다. SQLite FTS 가상 테이블 `ex_fts`는 재생성 가능한 검색 캐시이며 업무 레코드로 이전하지 않는다. 사용자 도구와 MCP에서 추가 발견한 도구는 고정된 81개 밖의 동적 등록부다. SDK의 예제용 `Agent` 도구도 현행 81개에 포함하지 않는다.

## 데이터 형식과 객체 소유권

Rust의 `WorkspaceStore`가 전체 정규 레코드를 소유한다. `EntityId<T>` 타입과 `HashMap<EntityId<T>, Record<T>>`를 사용하며 객체 간 관계는 ID다. 에이전트는 공유 포인터나 파일 핸들로 레코드를 수정하지 않는다. 불변 조회 세대와 단일 writer 명령 처리기를 통해 읽고 변경한다. `header`와 `data`의 기존 PK가 있는 경우 같은 객체를 가리켜야 한다.

모든 저장 레코드는 `{header,data}`다. `header`는 `id,collection,version,commit_seq,created_at,updated_at,legacy_key`를 가진다. 신규 실행·메시지·이벤트 등의 DTO도 `data`에 넣고 같은 헤더를 사용한다. `version`은 생성 1부터 객체가 변경된 트랜잭션마다 1 증가하고, 한 트랜잭션에서 같은 객체의 최종 변경은 하나로 합친다. `commit_seq`는 마지막 변경 트랜잭션 순번이다. 기존 `content_version`·`evidence_version`·`report_evidence_version`은 도메인 카운터이므로 별도로 유지한다.

| 형식 | v1 규칙 |
| --- | --- |
| ID | 컬렉션별 접두어 + `_` + CSPRNG 128비트의 소문자 32자리 hex. 예: `task_00000000000000000000000000000001`. 접두어는 data-model의 id_prefix를 사용한다. ID를 재사용하지 않는다. |
| 숫자 | int32는 JSON 정수. BIGINT·순번·버전·바이트 길이는 10진 문자열. 부호 있는 원본 카운터는 int64 범위, 저장 순번·버전은 uint64 범위. 초과·NaN·Infinity는 거부한다. |
| 시각 | UTC RFC3339 `Z`, 소수 초는 0~9자리. 원본 트래픽 `ts/captured_at`은 Unix **초**를 10진 문자열로 보존한다. 자동으로 밀리초로 해석하지 않는다. |
| NULL | 저장 DTO는 전체 필드를 포함한다. SQL NULL은 JSON null. PATCH는 누락=유지, null=nullable 필드 해제, 값=교체. 빈 배열과 null을 합치지 않는다. |
| 본문 | `Body={text:string,blob:null}` 또는 `{text:null,blob:BlobRef}`. desktop_schema에서 Body로 지정한 본문은 UTF-8 텍스트 64 KiB 초과 시 BlobRef. 바이너리는 크기와 무관하게 BlobRef. 빈 본문은 text=""다. 일반 name/summary/description 등의 문자열은 그대로 유지하고 프레임 제한을 검사한다. |
| 파일 참조 | `BlobRef={sha256,length,media_type}`. 해시는 실제 원본 바이트의 SHA-256. OS 경로는 저장된 참조·권한을 대신하지 않는다. |
| 비밀 | `SecretRef={secret_ref,hint}`. LLM 키·자산 인증·MCP 환경의 비밀·알림 인증·프록시 userinfo를 OS 저장소로 분리한다. hint로 원문을 복원할 수 없어야 한다. 구조화된 자격 증명은 writer 진입 전에 참조로 정규화하고 normalized_params·tool_use 기록에도 참조만 저장한다. |
| 자연/복합 키 | 원본 자연 키와 복합 PK는 data에서 유지하고 별도 인덱스로 검사한다. 관계 레코드의 헤더 ID는 별도 생성한다. settings.key/tools.key 등의 문자열을 임의 정수로 변환하지 않는다. |

`data-model.v1.json`의 `desktop_schema`는 저장 시 기본 필드 검사다. 아래 의미 규칙도 필수다. SQL CHECK·UNIQUE·FK는 원본 선언을 Rust 검증기와 인덱스로 옮기며 SQL 문자열을 런타임에 실행하지 않는다. FK가 없는 사용량 기록의 과거 ID는 강제 FK로 바꾸지 않는다. 삭제된 대상을 표시할 수 있는 출처다. 이전 시 해석 가능한 ID는 새 typed ID로 연결하고, 해석 불가능한 원문은 legacy provenance에 보존한다. `category_id_snapshot`도 과거 기록이므로 현재 카테고리의 존재를 강제하지 않는다.

`llm_records/llm_usage.task_id`의 TEXT는 Go의 호출 registry key이며 업무 Tasks FK가 아니다. 이를 그대로 opaque 문자열로 보존하고 필터/집계에도 사용한다. SDK의 background task_id가 ExecutionId인 것과 별개다. 외부 ScopeSentry ID와 화면 트리의 합성 ID도 workspace EntityId로 강제 변환하지 않는다. typed ID 변환 대상은 RPC W* DTO와 ui-wire-overrides의 정확한 필드 목록을 따른다.

### 컬렉션 대응과 필수 불변식

| 기존 테이블 그룹 | Rust 도메인 / 규칙 |
| --- | --- |
| companies, company_scope | `domain::companies`. name/nkey 중복 판정, domain/ip/cidr/icp/keyword 범위. 회사 삭제 시 scope cascade, assets.company_id null. |
| assets, task_asset_links, task_scope | `domain::assets`. 타입별 자연 키로 upsert. 공용 자산 소유권과 작업 연결·범위를 구분한다. |
| tasks, task_categories, task_templates, task_relations, task_llm_profiles | `domain::tasks`. 작업 생성·exploration·origin fact·직접 원본·회사 범위·LLM 체인을 같은 변경으로 만든다. |
| explorations, exploration_nodes, exploration_edges, exploration_anchors, task_constraints, main_sessions | `domain::exploration`. kind/state/payload와 관계 의미를 함께 검사. main session 교체가 공용 그래프를 초기화하지 않는다. |
| activity, conversation_activities, conversations | `domain::conversations`. tool_use_id별 결과 연결, 행 요약/본문 분리, 완료 시각·토큰 유지. |
| findings, finding_retests, traffic_evidence_snapshots, finding_traffic_bindings | `domain::findings/retests`. 별도 FindingId/NodeId, 불변 증거, 증거 버전과 보고서 기준 버전 유지. |
| task_archives | `domain::archives`. 의존 작업·상태 검사와 보관/복원/삭제 진행 기록. Go 아카이브 v1~v3 가져오기, 자체 형식은 artex-task/1. |
| agents, agent_prompts, agent_prompt_vars, agent_triggers, scheduler_state | `engine::agents/scheduler`. prompt 버전, agent.current_prompt_id는 해당 agent의 prompt만 허용. 트리거 상태와 실행 세대를 저장한다. |
| llm_profiles, llm_profile_health, llm_usage, llm_records | `engine::llm`. 키 참조, 프로필 체인, 항상 기록하는 사용량과 선택적 요청·응답 기록을 구분한다. health는 재구축 가능하며 전 실행의 단조 시계를 새 실행에 적용하지 않는다. |
| mcp_servers, mcp_tools_cache, tools, agent_visibility, agent_skill_visibility, tool_usage, skill_usage | `domain::configuration/tools`. 사용자 편집·enabled·바인딩·Skill 가시성을 보존. discovery cache는 재생성한다. |
| intercept_rules, intercept_pending, asset_intercept_rules, task_intercept_rules | `engine::policy`. 규칙 우선순위·범위·block/allow/ask·승인/타임아웃을 유지하고 승인 결과는 run/invocation에 묶는다. |
| side_question_sessions, side_question_requests | `engine::side_question`. 부모 문맥·run·generation 스냅샷, 중복 client_id, 취소·중단·답변·사용량 보존. |
| notification_channels, notification_events, notification_deliveries | `domain::notifications`. 채널 인증 참조, 이벤트의 최초 fan-out, 재시도 시각·시도 횟수·전달 상태를 저장한다. |
| settings, server_logs | `configuration` / `diagnostics`. 서버 로그인/JWT·기존 selfupdate 키는 사용하지 않는다. 나머지 설정은 기능별 어댑터로 유지하고 전역 OS 경로 설정은 호스트 설정으로 분리한다. logs는 회전 파일/재구축 가능한 조회다. |
| exchanges, exchange_bodies, blob_refs | `domain::traffic`. 요청·응답 원본을 BlobRef로 저장, 트래픽 메타데이터·본문 참조 관계 유지. SQLite/FTS는 자체 메모리/파일 검색 캐시로 교체한다. |

새 정규 컬렉션은 `agent_runs,tool_invocations,domain_events,agent_messages,agent_deliveries,conversation_requests,steering_messages,command_receipts,user_questions,memory_records,resource_files,agent_features,task_runtime`다. AgentInbox/AgentOutbox는 전달·메시지를 기준으로 재구축하는 조회 투영이며 별도 정규 파일을 쓰지 않는다. `resource_files`는 `{resource_kind,resource_key,relative_path,blob,revision}`로 Skill/MCP 설치 리소스를 관리한다. `traffic_records`는 exchanges와 exchange_bodies를 하나의 capture 레코드로 결합한 읽기 투영이다. 원본 두 레코드의 관계와 출처는 가져오기 목록에 보존한다. 실행 권한 모드·pre_plan_mode·approved_target_mode는 Run DTO에, 질문 generation은 UserQuestion DTO에 명시적으로 저장한다. Actor.kind는 user/engine/agent이며 자동 이벤트에 가짜 에이전트를 생성하지 않는다.

원본의 `ON DELETE CASCADE/SET NULL/RESTRICT`를 그대로 변경 집합으로 계산한다. 복합 FK와 추가 ALTER FK도 포함한다. 강제 삭제를 시작하기 전에 실행 중 자식·보관 의존성·증거 참조를 검사한다. 회사·프로필·카테고리 삭제는 nullable 참조 해제, task hard delete는 exploration/노드/관계의 원본 cascade와 명시적 cleanup 옵션을 따른다. soft delete는 원본 deleted_at 및 의도 deleted 상태를 유지한다. 대화 삭제 시 활성 재검증을 stopped로 전환하는 SQL 트리거를 같은 삭제 명령에 포함한다.

SQL updated_at 트리거는 엔진이 트랜잭션 시각을 한 번 읽어 대체한다. `sync_task_asset_links`는 정규 task_asset_links 변경으로 대체하고 assets.task_ids는 파생 조회다. 가져오기 때 양쪽을 대조하여 살아 있는 array 연결을 legacy 출처로 보충한다. dangling array ID를 유효 관계처럼 만들지 않는다.

### 자산 정규화와 중복

DomainKey는 trim → lowercase → 마지막 점 **한 개** 제거다. IPKey는 trim이다. 기존 IP 텍스트의 표현을 몰래 합치지 않는다. 새 cidr 비교는 파싱한 네트워크 값, c_segment는 IPv4 /24·IPv6 /48이다. RootDomain은 기준 Go dependency의 public suffix 목록을 고정한 리소스로 사용하며 IP/분류 불가 호스트는 자신을 root로 둔다. URL은 기존 normalizeURL처럼 scheme/host 소문자, query가 없는 단독 `/` 제거만 한다. query 순서·escape·path 대소문자를 바꾸지 않는다. endpoint method는 대문자, params 이름은 trim/lowercase이며 userId/user_id/uid는 별개다.

| 자산 타입 | 고유 키 |
| --- | --- |
| root_domain / ip | domain / ip |
| subdomain | domain + COALESCE(record_type,"") |
| app | bundle_id가 있으면 bundle_id, 없으면 app_name |
| service:http | url |
| service:other | COALESCE(domain,"") + COALESCE(ip,"") + port + service_name |
| endpoint | url + method |

명시적 회사 선택은 scope 재계산으로 제거하지 않는다. extra는 기존 얕은 객체 병합, task 연결·배열 필드의 중복과 병합 순서는 db/assets.go의 고정 계약을 유지한다. 원본이 보존한 비정상 URL/IP를 가져오기에서 폐기하지 않는다. 검증 경고와 기존 키를 기록하고 신규 입력 검증·자동 회사 귀속에서는 제외한다. NULL을 빈 문자열로 치환하는 것은 명시된 자연 키 비교에서만 수행한다.

### 노드 payload와 상태

payload는 원본 JSON을 손실 없이 보존하되 업무 필드는 kind별로 해석한다. begin은 시작 메타데이터, goal은 text/vulnclass, intent는 summary/plan·우선순위와 실행 문맥, fact는 summary/detail/evidence/confidence, finding은 vulnclass/name/severity/summary/evidence, hint는 text/traffic_refs, digest는 요약과 구성 node_id/content_version이다. `agent/tools.go`와 digest 코드의 입력/결과 계약을 사용한다. 알 수 없는 확장 키는 보존하고 소유권·ID·증거·계획 권한을 우회하는 용도로 해석하지 않는다.

| 종류 | 허용 상태 / 전이 |
| --- | --- |
| begin | open |
| goal | open → met 또는 abandoned; 사용자 편집으로 명시적 재개 가능 |
| intent | open → running; running → paused/done/blocked/exhausted/stopped; paused → running 또는 stopped; blocked/exhausted/stopped → open은 새 run 생성; soft delete → deleted |
| fact | origin 또는 confirmed/dismissed; origin은 작업 초기 설명·목표이며 실행 결과로 재해석하지 않음 |
| finding | confirmed/dismissed; Finding.status와 별도 |
| hint | active → consumed |
| digest | active → superseded; 구성 노드 원본을 삭제하지 않음 |

edge rel은 spawns/derived_from/yields/proves/covers. src≠dst, ID 존재와 적절한 kind 검사. 현재 작업의 새 intent가 직접 원본 fact를 derived_from 근거로 사용하는 것은 허용하지만 원본 노드를 수정하지 않는다. proves는 현재 작업 goal과 허용된 fact/finding 증거, yields는 실행 의도와 결과를 연결한다. 커버리지는 coverage_enabled일 때 현재 범위·anchor/covers를 기준으로 계산한다.

Task.status는 created/running/paused/done/failed/timeout, Exploration.status는 open/achieved/failed다. paused/queued·queued_at/queue_mode는 서로 모순되지 않게 명령에서 같이 갱신한다. pause는 실행 중 도구의 취소/정리를 완료한 뒤 paused로 확정하며 불확실한 외부 효과는 run.recovery_required로 남긴다. timeout은 마감 이후 신규 작업 도구 실행을 막고 설정된 wrapup만 허용한다. 완료/취소 이벤트와 completed_at은 같은 변경에 포함한다. 순번·시계는 engine epoch마다 새로 연결하며 절전 복귀 시 UTC deadline과 대조한다.

task_runtime은 task별 하나다. 새 작업은 timeout_policy=active_elapsed, 가져온 작업은 legacy_absolute와 기존 deadline_at을 보존한다. active_elapsed는 task.running 동안 OS별 절전 제외 단조 시계로 누적하며 queued/사용자 pause/절전 시간은 제외한다. 실행 중 협업 응답 대기는 active에 포함한다. 시작·정지·도구 경계 및 최대 5초 간격으로 누적 시간을 저장한다. 재시작 시 last_active_clock을 다른 epoch의 시계에 연결하지 않고 accounting_state를 대조한다. 미확정 실행 구간의 시간이 불명확하면 recovery_required로 표시해 확인 후 재개한다. 표시 deadline_at은 활성 상태의 남은 예산에서 계산하며 원본 절대 마감 정책과 구분한다. 메시지의 UTC deadline은 작업 pause로 연장되지 않는다.

새 작업의 plan_heartbeat_seconds는 **max(입력,600)**, 미지정/0/음수도 600이다. SQL 기본 300과 실제 Go 보정 600의 차이는 새 Go 동작을 따른다. 가져온 기존 값을 일괄 변경하지 않는다. agent.run_seconds 기본 1200, max_turns=0은 원본 무제한 의미를 유지한다. source_task_ids는 중복 제거 후 최대 8, 자기 참조 금지, 원본 존재·삭제/보관 상태 검사. parent_ref는 표시 관계이며 공유 권한을 추가하지 않는다. 회사 개수·각 핸들러 페이지/입력 제한은 고정된 Go 검증을 유지한다.

### 취약점·증거·재검증

FindingId와 FindingNodeId를 별도 타입으로 쓴다. report_finding 결과는 둘을 모두 제공한다. v0 update_finding_report.finding_id는 **NodeId**이므로 v1은 finding_node_id로 이름을 바꾸며 finding↔node 유일 매핑을 통해 갱신한다. get_finding_traffic/bind_finding_traffic는 **FindingId**를 받는다. 잘못된 종류의 ID를 숫자 모양만으로 추측하지 않는다.

Finding.status는 pending/in_progress/confirmed/resolved/fixed/false_positive/ignored/duplicate/risk_accepted, severity는 critical/high/medium/low다. evidence/report 원문을 보존하고 nullable task/node 참조와 asset_ids를 검사한다. 증거 바인딩은 baseline/proof/verification/supporting, 미지정 supporting, 같은 finding+snapshot 중복 금지, position/id 정렬이다. 모든 traffic_refs가 준비되지 않으면 해당 바인딩 명령은 전부 실패한다. 다른 일반 배치 도구의 항목별 부분 성공과 구분한다.

증거 파일을 먼저 저장·동기화하고 finding/node/anchors/bindings/evidence_version/이벤트를 한 트랜잭션으로 확정한다. 증거 변경마다 evidence_version 증가, update_finding_report는 해당 버전을 **필수**로 비교하고 report_evidence_version에 저장한다. 새 증거 이후의 낡은 보고서 저장은 EVIDENCE_CONFLICT다. 동일 증거의 재바인딩은 기존 note/role을 조용히 덮어쓰지 않는다. capture 삭제가 증거 파일을 삭제하지 않는다. GC는 모든 바인딩·보관·두 체크포인트 세대·재생 저널 참조를 검사한다.

원본 TrafficSnapshotID의 64자리 해시를 legacy_key로 보존한다. 가져온 스냅샷의 원본 정규화·해시를 검증하되 새 EntityId와 내용 BlobRef 해시를 구분한다. 표시용 UTF-8 정제와 원본 바이너리 보존을 분리한다. 새 캡처 원본을 DB의 텍스트 제약에 맞추려고 손실 변환하지 않는다.

재검증은 Finding별 pending/running 최대 하나, conversation 고유 연결, 시작 시 finding/evidence/현재 제약의 스냅샷을 저장한다. 도구는 엔진이 주입한 conversation/run에서만 대상을 얻는다. 입력으로 다른 취약점으로 전환할 수 없다. verdict=reproduced/fixed/inconclusive와 비어 있지 않은 summary/evidence를 저장한다. 완료 fixed는 Finding.status=fixed와 같은 변경이고 다른 판정은 기존 status 유지. 중단/대화 삭제/장애는 stopped/failed 및 종료 시각을 저장한다.

### LLM·도구 정책·기록

프로필 format은 openai/anthropic/openai-responses. base_url/proxy/model/thinking_type/reasoning_effort/context_window/max_tokens/streaming/priority/pool_exclude/session_header_key를 유지한다. retry_*_attempts: 0=전역 상속, -1=재시도 비활성, 양수=명시 횟수. interval_ms: 0=기존 지수 backoff, 양수=고정 간격. task_llm_profiles의 순서·ready/quota_exhausted·active profile·chain revision을 함께 변경한다. 스트리밍 결과나 tool_use를 내보낸 뒤 프로필을 바꿔 같은 요청을 다시 실행하지 않는다.

사용량은 성공·실패 모두 ledger로 기록하며 원본처럼 best-effort 실패를 별도 진단한다. 확정 응답 후 보존을 보장하는 업무 변경과 사용량 best-effort를 구분한다. 원본 사용량·기록의 과거 task/profile 정보는 삭제 후에도 조회할 수 있다. 상세 요청/응답 수집 스위치·토큰 집계 기준·캐시 토큰 의미는 유지한다. 오래된 prompt/detail은 큰 본문 파일로 분리한다.

툴 제공 집합은 실행 role에 조립된 도구 ∩ enabled/agent binding ∩ 기능 활성화 ∩ MCP/Skill visibility다. 권한은 input까지 검사한 뒤 승인→실행 직전에 다시 검사한다. 기본 도구 정의의 사용자 편집은 설명과 스칼라 기본값만 바꾸고 핸들러·필드 타입·required·ID 종류는 변경하지 않는다. 원본의 누락/빈 문자열/null 스칼라 default 주입 의미를 유지하며 객체/배열은 default 주입하지 않는다. 일반 RPC PATCH의 null 규칙과 도구 default 규칙을 구분한다.

## RPC 프레임과 공통 처리

호스트↔엔진 전용 stdin/stdout 파이프에 **4바이트 little-endian unsigned 길이 + 그 길이의 UTF-8 JSON**을 쓴다. 길이는 JSON 바이트 수이며 1~1,048,576, BOM/추가 줄 구분자 없음. stdout 로그·셸 출력 금지. 부분 읽기를 누적하고 잘못된 길이/UTF-8/JSON은 연결을 닫는다. 요청 id는 epoch 안에서 고유한 1~128자 문자열이다. JSON-RPC 2.0 봉투에 protocol=artex-rpc/1.0 필드를 추가한다.

system.hello에서 버전 교집합·host epoch·capabilities를 교환하고 엔진은 engine_epoch, engine_version, store_format, max_frame_bytes=1048576, max_blob_chunk=65536, read_only, 현재 workspace/commit_seq를 반환한다. 교집합 없으면 VERSION_MISMATCH로 종료한다. 모든 작업 공간 요청에 workspace_id와 engine_epoch를 넣고 다른 공간/이전 epoch 요청은 거부한다. hello/open/create 등 연결 설정 요청은 두 필드 null을 허용한다.

영속 변경은 command_id와 관련 **모든** 기존 객체의 expected=[{collection,id,version}]을 요구한다. 생성은 expected=[]와 자연 키 유일 검사를 사용한다. 배열 안 같은 collection+id 중복을 거부한다. expected는 비교 시점의 정책/참조 객체도 포함하며 writer에서 검사한다. 요청 상태가 message_id를 키로 쓰는 경우에도 agent_messages와 conversation_requests의 버전을 혼동하지 않는다. object_refs와 DomainEvent.subject_collection도 같은 컬렉션 명시 규칙을 사용한다. engine 내부 호출도 같은 명령 경로를 사용한다.

명령 순서는 입력·권한 검사 → 알려진 command_id 대조 → 현재 version 검사 → 전체 변경 집합 검증 → 참조 파일 동기화 → 저널 동기화 → 메모리/인덱스 일괄 반영 → 응답이다. 동일 command_id의 normalized_params·method·workspace·expected가 같으면 과거 영수증을 반환한다. 객체 키 순서는 무시하고 배열 순서는 유지하며 스키마 정규화 후 수치를 비교한다. 다른 내용이면 COMMAND_ID_REUSE. 이미 성공한 명령의 재전송을 현재 expected mismatch로 거절하지 않는다.

무변경 읽기는 read_seq를 반환한다. 변경 응답의 read_seq는 commit_seq다. JSON-RPC 오류는 정수 code/message/data이며 data에 문자열 code/retryable/recovery_required/details를 넣는다. 숫자·문자열 코드 대응은 rpc 계약의 error_codes로 고정한다. 외부 프로그램 stderr를 민감 값 제거 없이 message에 넣지 않는다. timeout/cancel은 실행 완료와 다르며 확정한 명령을 롤백하지 않는다. 재연결 시 command.status로 성공 여부를 확인하고 같은 command_id로 재전송한다. 외부 실행의 미확인 효과에는 자동 재시도를 하지 않는다.

입력 큐는 연결당 최대 256 요청·16 MiB, 응답/이벤트 큐는 16 MiB. 초과 시 BUSY와 retry_after_ms 반환. 호스트·엔진 양쪽의 읽기 펌프를 항상 유지한다. 변경 응답은 버리지 않고, 지연된 UI 진행 이벤트만 합칠 수 있다. 이벤트 소비자가 지속 지연하면 해당 구독을 해제하고 RESYNC_REQUIRED를 보낸다. 기본 RPC timeout 30초, 최대 1시간이며 긴 대기 작업은 실행/요청 ID로 비동기 등록한다.

### RPC 메서드와 결과

`rpc.v1.schema.json.methods[method]`로 params를 검사한 뒤 도메인 검증한다. `ui.<api 함수명>`은 기존 화면 facade의 인자 이름을 유지하며, 결과는 ui-dtos의 같은 operation.result_schema에 v1 변환을 적용한다. 여러 화면 alias가 같은 도메인 명령을 호출할 수 있으므로 등록 메서드 수는 업무 기능 수와 다르다.

`legacy.<Go handler>`는 비교/이전용 facade다. params={route_id,path,query,body,blob_ref?}이며 route_id는 registry에 고정된 값만 받는다. 같은 Go handler가 PATCH/DELETE/PUT 또는 여러 부모 문맥을 처리하는 경우 route_id로 정확한 branch를 선택한다. path 키는 원본 경로 placeholder, query 키/배열은 기존 URL query의 논리 값, body는 고정된 Go JSON 요청 DTO다. raw URL을 엔진이 해석하거나 HTTP 서버로 재호출하지 않는다. registry의 고정 Go contract와 해당 DTO/검증 branch를 동일한 도메인 서비스로 옮긴다. HTTP status는 Result 또는 아래 오류 코드로 변환한다. 두 facade가 저장을 직접 처리하는 구현은 금지한다.

| 메서드 | 결과 / 처리 |
| --- | --- |
| system.hello / ping / shutdown | 위 협상 결과 / {engine_epoch,commit_seq,uptime_ms} / {accepted:true}. shutdown 완료는 파이프 종료와 호스트 wait로 판정. |
| workspace.create/open/info | {workspace_id,name,format_version,commit_seq,read_only,recovery,capabilities}. 경로는 호스트가 준 directory capability로만 연다. |
| workspace.close/checkpoint/recover | {closed:true} / {generation,base_seq,manifest_sha256} / {mode,valid_generations,last_valid_seq,corruption,lost_range,recovery_copy_capability}. |
| query.objects | {items:[RecordEnvelope],read_seq,next_cursor}. limit 기본 100/최대 500. at_seq가 보유하지 않은 세대이면 RESYNC_REQUIRED. |
| command.status | {state:unknown/pending/committed,commit_seq?,result?,result_ref?}. unknown은 외부 작업 미실행의 증거가 아님. |
| rpc.cancel | {accepted:boolean,committed:boolean}. pending은 제거, executing은 취소 토큰, committed는 원래 결과 유지. |
| events.subscribe/unsubscribe | {subscription_id,after_seq,high_watermark} / {removed:boolean}. cursor 만료 시 RESYNC_REQUIRED. |
| blob.begin/write/commit/read | {transfer_id,next_offset} / {next_offset} / BlobRef / {offset,length,total_length,data_base64,eof}. |
| tool.invoke | {invocation_id,state,result?,result_ref?}. 지속 실행은 상태 이벤트와 조회 DTO로 완료. 동일 run+tool_use_id+ordinal은 같은 invocation. |
| agent.send/inbox/claim/acknowledge/cancel_request | {message_id,delivery_ids,request_id?} / {items,next_cursor,read_seq} / {claimed,owner,claim_deadline} / {state} / {state}. |
| user.answer | {accepted,state}. generation·run·question 대상이 모두 일치해야 함. |
| agent.features.get/set | {agent_id,questions,plan_mode,memory}. agent별 선택 기능 설정, 신규/가져온 기본값은 모두 false. set은 해당 설정 객체의 version을 비교. |
| host.secret.set/delete | {secret_ref,hint} / {deleted:boolean}. 값은 호스트에서 끝내며 엔진/화면 조회 응답에 원문을 반환하지 않음. |
| host.execute/status/stop | {execution_id,state} / {execution_id,state,exit_code?,output_ref?,last_cursor} / {accepted,state}. engine→host 전용. |
| host.update.check/apply | {configured,available,package_id?,version?} / {accepted}. 서명키/배포 위치 미설정이면 configured=false 및 FEATURE_DISABLED. |
| ui.* / legacy.* | 원본 결과 DTO + v1 ID/본문/비밀/시각 변환. streaming 응답은 run_id/subscription_id, 다운로드/업로드는 BlobRef/capability로 대체. |

공통 오류는 INVALID_REQUEST, INVALID_PARAMS, METHOD_NOT_FOUND, VERSION_MISMATCH, WORKSPACE_NOT_OPEN, STALE_EPOCH, NOT_FOUND, VERSION_CONFLICT, EVIDENCE_CONFLICT, COMMAND_ID_REUSE, PERMISSION_DENIED, FEATURE_DISABLED, BUSY, CANCELLED, TIMEOUT, SIZE_LIMIT, IO_FAILURE, CORRUPT_STORE, RECOVERY_REQUIRED, RESYNC_REQUIRED, EXTERNAL_FAILURE다. 400→INVALID_PARAMS, 404→NOT_FOUND, 409→VERSION_CONFLICT 또는 원본 도메인 충돌 details, 403→PERMISSION_DENIED, 503→BUSY/FEATURE_DISABLED. IO_FAILURE/CORRUPT_STORE는 쓰기 중지와 recovery_required=true, 단순 실행 실패는 EXTERNAL_FAILURE다. 새 epoch의 stale 요청은 command 재조회 후 새 request id로 다시 보낸다.

### 원본 API의 전환과 DTO 변환

261개 경로는 엔진 242, 이벤트 4, Blob/capability 7, 호스트 업데이트 4, OS 인증으로 제거 4로 분류된다. server.go의 직접 등록 243개뿐 아니라 registerFindingTraffic의 7개, registerSideRoutes의 11개를 펼친 집계다. 세부 disposition은 대응표의 고정 값이다. `/api/chat`의 POST streaming도 이벤트 구독으로 전환하며 activity/log/sidequestion 이벤트와 함께 처리한다.

화면 authStatus/login/initPassword/changePassword는 등록된 호환 이름을 유지하더라도 FEATURE_DISABLED(OS_SESSION_AUTH)로 종료하며 화면에서 호출을 제거한다. selfupdate check/apply/rollback/stream은 자체 서명 업데이트의 host facade로만 변환한다. 원본 GitHub 업데이트 실행을 재사용하지 않는다. activity/log SSE·chat streaming은 events.subscribe, 파일 multipart는 blob transfer+host capability, 트래픽 본문은 blob.read로 대체한다. terminal/WebSocket의 바이트 흐름은 호스트 PTY frame/cursor 구독으로 옮기며 shell session owner run을 검사한다. sidequestion streaming은 side_question 요청 상태와 같은 event 채널을 사용한다.

v1 DTO 변환은 양 facade와 도구에 공통 적용한다. Entity DTO의 id와 *_id는 해당 객체의 접두어 문자열, *_ids는 그 배열이다. 원본 TypeScript의 number형 category/profile/company/MCP/rule/archive ID도 문자열로 바꾼다. `TaskOutput/TaskStop.task_id`는 업무 TaskId가 아니라 **ExecutionId**, shell session_id는 ShellId다. trace.step_id/step_ids와 업무 Activity DTO의 seq는 ActivityId, 대화 Activity의 seq는 ConversationActivityId, list_facts.before는 NodeId다. 이전 구간은 해당 객체의 원본/생성 순번으로 찾으며 source facade의 before=0은 전송 어댑터에서 누락으로 정규화한다. 임의 hex ID의 사전 순서를 시간으로 사용하지 않는다. 트래픽/증거 본문의 byte offset은 10진 문자열, 파일 Read의 행 offset과 제한된 chunk length는 int32다. 이벤트·로그 순번, wire 버전·토큰 누적·전체 길이는 10진 문자열, 페이지/port/status/제한은 int32다. 개별 토큰 행 int32를 집계하면 문자열로 반환한다. 저장의 원본 INT 버전 카운터와 wire 문자열 버전은 facade에서 명시적으로 변환한다. epoch 시각 형식이 다른 원본 API는 엔진에서 UTC Timestamp로 정규화하며 UI에서 숫자 Unix를 추측하지 않는다.

RPC body 내 SecretRef와 api_key_hint만 조회 가능하다. 키 입력 UI는 host.secret.set 후 참조를 profile 변경 명령에 넣는다. host.secret.set의 원문 입력은 호스트에서만 처리하며 엔진 저널·command_receipts·진단에 기록하지 않는다. 호스트 비밀 생성의 중복 키는 상위 command_id+필드 경로이며 OS 저장소의 메타데이터로 같은 값에 같은 SecretRef를 반환한다. 같은 키에 다른 값은 재사용 충돌이다. 인식된 자격 증명 필드를 도구 기록·내보내기에서 평문으로 재직렬화하지 않는다. 캡처 원본이나 사용자의 자유 텍스트에 포함된 문자열 전체가 자동 비밀 분류된다는 보장은 하지 않으며 기존 원문 보존 정책을 유지한다. `host.*` 실행/비밀/업데이트 메서드는 방향별 allowlist이며 UI에 일반 명령 실행 권한을 주지 않는다. File/Blob/FormData/AbortSignal 인자는 schema의 x-native-capability로 표기된 로컬 전송 어댑터 슬롯이다. File 등 객체를 JSON으로 직렬화하지 않고 transfer/capability·rpc.cancel로 변환한다.

기존 목록의 정렬·필터·limit/offset/page 기본값은 frozen handler 계약을 유지한다. 신규 객체 조회는 cursor를 사용한다. 커서는 {workspace,collection,read_seq,filter_hash,last_sort_key}를 opaque base64url로 인코딩하고 해당 엔진 세대 내 검증한다. 다중 페이지는 같은 read_seq를 유지하고 보유하지 못하면 재조회한다. 사용자 편집이 반영된 결과를 이전 cursor에 섞지 않는다.

## 저장 형식과 파일 배치

```text
workspace/
  project.json
  locks/writer.lock
  store/manifests/00000000000000000001.json
  store/snapshots/00000000000000000001/<collection>.ndjson
  store/journal/00000000000000000001.jnl
  objects/blobs/<sha256-first-2>/<sha256>
  records/                 # 조회용 재생성 가능한 목록/offset 인덱스
  traffic/                # 캡처 조회/검색 캐시, 정규 상태는 store+blobs
  resources/skills/       # resource_files의 검증된 materialization
  backups/
  tmp/
```

project.json={format:"artex-workspace",format_version:1,workspace_id,name,created_at}. 업데이트는 동일 볼륨의 임시 파일+sync+atomic replace. OS 파일 잠금을 유지하는 writer.lock은 PID/host epoch의 진단 정보를 포함하며 존재 여부로 잠금을 판정하지 않는다. 설치 경로·외부 선택 파일·심볼릭 링크/재분석 지점을 저장 경로로 맹신하지 않는다.

### 트랜잭션 저널

모든 숫자 바이너리 필드는 little-endian, 오프셋은 0부터다. `.jnl`은 아래 프레임들의 연결이며 별도 파일 헤더는 없다. 파일 이름은 첫 프레임의 seq를 20자리 0 채움한 값이다. 첫 프레임도 이전 세그먼트의 마지막 hash를 잇는다. 최초 프레임의 previous hash는 32바이트 0이다. checkpoint에서 시작하는 replay는 manifest가 제공한 base_frame_sha256을 사용한다.

| 오프셋 | 바이트 | 필드 |
| --- | --- | --- |
| 0 | 8 | ASCII ARTEXJ01 |
| 8 | 2 | format_version=1 |
| 10 | 2 | flags=0; 알려지지 않은 값 거부 |
| 12 | 4 | payload_length, 최대 16,777,216 |
| 16 | 8 | workspace commit_seq; 이전 확정 seq+1 |
| 24 | 16 | transaction_id CSPRNG 128비트 원시 값 |
| 40 | 32 | 이전 전체 프레임의 SHA-256 |
| 72 | 32 | payload 실제 바이트의 SHA-256 |
| 104 | payload_length | UTF-8 JSON payload |
| 104+payload_length | 8 | ASCII ARTEXEND |

전체 frame 해시는 header+payload+footer의 SHA-256다. 체크섬은 JSON 객체 재직렬화 결과가 아니라 **실제 저장 바이트**에서 계산한다. 프레임 payload={format_version:1,committed_at,command_id,method,normalized_params,expected,mutations,events,deliveries,result}. mutations는 `{op:"put"|"delete",collection,id,expected_version,new_version,record?}` 배열이고 delete는 tombstone을 남긴다. put은 record가 필수, delete는 record를 보내지 않는다. 신규 expected_version=0/new_version=1, 갱신/삭제는 new_version=expected_version+1. record는 완성된 RecordEnvelope다. events/deliveries는 같은 seq의 완성 RecordEnvelope이며 mutations와 중복 적용하지 않는다. 성공 영수증은 payload의 command_id/params/result/committed_at에서 생성하고 checkpoint에 포함한다. 큰 result는 `{result_ref:BlobRef}`이며 blob을 먼저 저장한다.

단일 append/write-all 이후 파일 sync_all을 완료해야 변경이 확정된다. 쓰기 오류가 발생하면 그 스트림을 폐쇄하고 쓰기를 중지한다. 부분 프레임을 성공으로 응답하지 않는다. 메모리 상태는 sync 이후 일괄 교체하고 알림은 마지막에 보낸다. 소스별 부분 성공 도구도 한 저장 프레임에 성공 항목만 넣고 실패 항목과 응답 영수증을 함께 보존한다. 16 MiB를 넘는 값은 Body/blob으로 분리한다. 관계 수 자체가 초과하는 bulk 요청은 SIZE_LIMIT를 반환하거나 명시적 job/batch 단계로 나누며 중간 단계를 전체 완료로 응답하지 않는다.

256 MiB 이상 세그먼트는 다음 트랜잭션 전에 새 파일로 회전하며 한 프레임을 분할하지 않는다. 새 파일 및 디렉터리 항목을 sync한 뒤 그 파일에서 확정 응답한다. blob은 tmp→hash 검증→동기화→동일 볼륨 rename→디렉터리 동기화 후 참조한다. Windows는 native flush/rename 계약을 구현하고 Unix는 file+directory fsync를 구현한다. 각 지원 파일 시스템에서 장애 시험을 통과해야 해당 보존 보장을 선언한다.

### 체크포인트와 manifest

manifest 필드는 format="artex-checkpoint",format_version=1,workspace_id,generation,base_seq,base_frame_sha256,created_at,files:[{path,sha256,length,record_count}],journal_start_seq,command_receipt_count,manifest_sha256이다. generation/seq/길이/개수는 10진 문자열. manifest_sha256은 해당 키를 뺀 manifest 객체를 **UTF-8 키 바이트 순 정렬·공백 없는 JSON**으로 직렬화한 해시이며 manifest의 모든 숫자는 정수 문자열 또는 format_version=1이어서 부동소수점 정규화가 필요 없다. 파일 목록은 path 순서로 정렬한다. 상대 path만 허용하고 traversal·중복·링크를 거부한다.

seq=N의 불변 상태를 빌려 각 컬렉션 NDJSON을 id의 UTF-8 바이트 순서로 생성한다. 각 행은 RecordEnvelope JSON과 LF, 파일은 BOM 없음. 삭제 표식은 별도 `_tombstones.ndjson`에 `{collection,id,version,commit_seq,deleted_at,legacy_key}`로 저장하며 같은 ID를 다시 만들지 않는다. 대형 본문은 BlobRef로 분리되어 있으므로 순차 생성·읽기가 가능하다. 스냅샷 파일별 바이트 SHA-256/개수/길이를 검사하고 모든 파일 sync → snapshots 세대 디렉터리 sync → manifest 임시 파일 sync → manifest rename → manifests 디렉터리 sync 순으로 설치한다. 동시 새 명령은 N 이후 저널로 계속 쓴다. manifest가 없는 새 snapshot은 미완료 세대다.

최소 **두 유효 체크포인트**와 더 오래된 세대의 N+1부터 최신까지 저널 체인을 보존한다. 두 세대가 완성되어야 그 이전 파일/영수증 데이터 정리를 시작한다. checkpoint 생성 실패는 기존 세대/저널에 영향을 주지 않는다. v1의 command receipts는 workspace 생존 기간 동안 보존하고 compact 시 결과 본문만 blob으로 옮긴다. 임의 24시간 만료로 재전송 변경이 재적용되는 구현은 금지한다.

### 시작·복구·GC

시작은 잠금 획득→project 형식→manifest 해시/파일/세대→정규 레코드 불변식→base 이후 저널 체인→참조 blob 존재·해시→실행 복구→쓰기 가능 여부 순이다. 마지막 세그먼트 EOF에서 헤더/payload/footer가 **미완성**이면 원본을 복구본으로 보존한 뒤 그 tail만 제외할 수 있다. 길이는 완성됐으나 해시/footer/seq가 틀린 프레임, 중간 구멍, 참조 blob 손상은 read_only corruption이며 뒤의 변경을 추정 적용하지 않는다.

최신 snapshot이 깨져도 이전 snapshot과 모든 이후 저널로 동일 최신 seq를 복원할 수 있으면 정상 복구한다. 체인이 부족하면 last_valid_seq/lost_range를 표시한 복구 복사본만 제공하고 묵시적으로 오래된 상태를 정상 성공처럼 열지 않는다. 명령이 디스크에 남았으나 응답 전 종료된 경우 replay하고 영수증으로 중복 응답한다. acknowledged seq 이후 손실이 발생하면 지원 저장 환경의 장애로 보고한다.

재시작 후 queued는 정책 재검사 후 재개할 수 있지만 running/waiting 외부 실행은 host execution_id와 대조한다. 종료/효과가 불명확하면 interrupted 또는 recovery_required. 임의 명령 자동 재실행 금지. pending 질문/메시지는 deadline·run/epoch와 대조하고 사용자에게 재개 가능 상태를 보여준다. 완료된 claim은 다시 실행하지 않는다.

GC는 active state·두 snapshot·필요 저널·ready archive·참조 evidence의 reachability를 합산한다. 준비됐으나 미참조 blob은 유예 24시간 후 수거한다. 삭제 계획을 저장하고 대상 hash를 재확인해 unlink하며 삭제 실패는 재시도 기록으로 남긴다. cache 삭제는 정규 상태를 바꾸지 않는다. capture 삭제와 evidence 삭제의 소유권을 합치지 않는다.

작업 보관은 paused/terminal, queue 해제, 직접 live dependent가 없음을 확인한다. 여러 작업 보관은 dependents부터 순서대로 예약하고 cycles는 의존성 오류다. artex-task/1 패키지는 ZIP의 manifest+NDJSON+blob 목록과 해시를 포함하며 키/CA 개인 키를 제외한다. 원본 v1~v3 아카이브의 actual format_version을 확인한다. SQL 기본 2만 보고 최신 Go 형식 3을 누락하지 않는다. 복원은 원본 보관 파일을 변경하지 않는 staging 검증과 한 명령의 참조 교체이며 진행 실패를 ready로 오인하지 않는다.

## 에이전트 공유와 저장된 활성화

`SharedContextService.read(ContextScope)`는 요청자의 agent/run/current_task와 현재 read_seq를 받아 현재 작업 + 명시적 직접 원본을 조회한다. 같은 task의 기록은 공통 정규 레코드를 읽는다. source 관계는 한 단계만 적용하며 A→B→C에서 A에 C를 제공하지 않는다. inherited=true/source_task_id와 증거 출처를 유지한다. 관계 해제·권한 철회·삭제는 다음 조회 및 도구 실행 직전 반영한다.

원본 의도 결과/trace는 done/blocked/exhausted/stopped일 때만 노출하며 state 검사와 detail 조회를 같은 snapshot에서 수행한다. thinking/usage를 상속 결과에 넣지 않는다. 현재 running으로 재개된 원본의 기록은 숨긴다. folded digest는 원본 작업 자신의 접힌 보기와 구성원 검사를 사용한다. 직접 source의 source digest를 재귀 확대하지 않는다. 일반 source 조회와 플랫폼 도구의 명시적 대상 task 권한을 분리한다.

기존 NotifyFinding/NotifyDone/Goal/Hint와 on_finding/on_goal_met/on_task_timeout/on_tool_call/on_task_create/interval을 `DomainEvent`로 매핑한다. event는 확정된 subject_id/version을 참조한다. interval event의 activation_key는 trigger_id+예정 UTC slot, 객체 event는 trigger_id+event_id+recipient다. 같은 key는 단 한 delivery를 만든다. task/agent 정책 검사와 delivery 생성은 같은 writer transaction이다.

trigger_run_mode serial은 agent별 active run 하나, parallel은 max_parallel(기본 5)까지. merge all은 같은 agent의 pending 활성화를 묶고, by_task는 agent+task, none은 event별 유지한다. 병합된 delivery의 원본 event ID 목록은 모두 보존한다. reporter는 parallel/none/5 및 report_finding 완료 트리거를 유지한다. 관찰(receipt)은 처리 완료가 아니다. on_tool_call은 확정된 tool_result 완료를 기준으로 하고 tool_use만 기록된 실행을 완료 트리거로 발화하지 않는다.

claim은 writer에서 pending/만료된 claim을 CAS하고 owner={kind:"agent",agent_id,run_id,engine_epoch}, claim_deadline=현재+30초를 저장한다. 활성 run은 10초마다 갱신한다. 완료는 대상 효과/답변과 delivery.completed를 같은 변경으로 저장한다. 메모리 wake-up은 최적화이며 유실돼도 pending 스캔으로 복원한다. claim이 재배정돼도 외부 실행은 invocation/execution_id 대조 없이 새로 호출하지 않는다.

steer_work는 현재 실행 중 intent의 **target_run_id**에 FIFO ordinal로 저장한다. 다음 tool.invoke 전에 적용·consumed를 저장하고 prompt를 갱신한다. 다른 run으로 바뀌거나 종료되면 dropped다. 이미 호출한 외부 명령을 뒤로 취소한 것처럼 처리하지 않는다. kill_work/pause_task는 해당 run 취소 요청과 프로세스 감독을 연결한다.

## 내부 메시지 프로토콜 artex-agent/1.0

Brevduva의 identity/channel/send/request/reply/report/receipt/acknowledge 구조를 활용한 **ARTEX 자체 내부 프로토콜**이다. Brevduva 서버·수신 데몬·MCP·SDK·계정·네트워크 연동을 구현 의존성에 추가하지 않는다. 문법과 전달 보장은 이 문서가 정의하며 Brevduva 외부 서비스와 wire 호환을 약속하지 않는다.

참고 출처는 [froiibn/brevduva](https://github.com/froiibn/brevduva)이며, 후속 라이선스 점검에서 Apache-2.0 LICENSE와 실제 NOTICE를 확인했다. [원문·출처 기록](licenses/third-party-notices.md#brevduva-프로토콜-구조-참고)을 보존한다. ARTEX 내부 프로토콜의 구현과 전체 프로젝트 라이선스는 별도로 구분한다.

AgentMessageRouter는 workspace의 `agent_messages/agent_deliveries/conversation_requests`를 관리한다. sender는 kind=agent와 유효한 agent_id/run_id/epoch를 인증된 문맥에서 주입하고 입력 sender를 그대로 믿지 않는다. recipients는 존재하고 활성화/통신 권한이 있는 agent IDs, channel은 `workspace/<id>/task/<id>` 또는 `workspace/<id>/agents`, topic은 1~128자다. object_refs는 허용 조회 권한을 추가하지 않는 읽기 참조다. 결론을 기록할 때 현재 객체가 참조 version과 다르면 VERSION_CONFLICT로 재확인한다. 객체 변경은 도메인 명령으로만 수행한다. 메시지 body text는 64 KiB 이하, 초과는 BlobRef. recipients 최대 64, 빈 값·중복은 금지한다.

| kind / 작업 | 상관 ID·완료 의미 |
| --- | --- |
| send | correlation_id=null, final=false, outcome=none. 수신 대상별 delivery 생성. |
| request | message_id가 request ID, correlation_id=null, final=false, outcome=none. deadline 필수, completion_mode one/all과 expected_reports를 명령 등록 시 고정. |
| reply | correlation_id=원 request ID, 원 recipients의 agent만 답변. final=true이면 outcome ok/error/cancelled가 필요하며 요청 완료 판정에 포함. |
| report | 원 request에 correlation_id 연결. progress는 final=false/outcome=none, 종결 보고는 final=true/outcome 지정. 진행 보고만으로 요청을 닫지 않음. |
| receipt | 원 message ID와 correlation 연결, final=false/outcome=none. 업무 답변/완료가 아니라 해당 recipient의 관찰 기록. |
| broadcast | recipients를 전송 시 정책·채널 membership에서 고정한다. 이후 가입자에게 과거 메시지를 재전달하지 않음. correlation_id=null, final=false. |
| agent.acknowledge | delivery ID·owner run을 CAS. observed는 관찰, completed는 send/broadcast의 소비 완료. request의 답변 없이 completed로 요청 자체를 닫지 못함. |

request의 completion_mode/expected_reports는 ConversationRequest 등록 옵션이며 v1 기본은 one, 다수 전원 답변은 all이다. 별도 설정을 보내려면 agent.send.params에 message와 request_options를 넣는 등록 계약을 사용한다. one은 첫 유효 종결 답변으로 completed/failed/cancelled, all은 고정된 expected_reports 각각의 종결 답변이 있어야 종료하며 error가 하나라도 있으면 failed다. 한 agent의 첫 final 답변을 저장하고 같은 내용 재전송은 같은 답변을 반환한다. 다른 final 답변은 ALREADY_COMPLETED domain 오류다. 기존 종료 상태를 뒤집지 않는다.

request 상태: open→claimed→waiting→completed/failed/cancelled/expired, 재시작으로 복구 불가한 owner는 interrupted. claim은 **recipient별 delivery**에 적용하므로 all의 한 수신자가 다른 수신자를 막지 않는다. sender가 기다리는 동안 run은 waiting이며 worker 병렬 슬롯을 반환한다. all의 일부 수신자 실패/만료 결과도 terminal_reports에 남긴다. one 완료 이후 늦은 reply/report는 요청에 붙은 late=true 진단 기록으로 남기며 완료 결과를 바꾸지 않는다.

deadline 기본 120초, 최소 1초·최대 3600초, 부모 request/run의 마감보다 늦을 수 없다. 경과 시 expired와 만료 이벤트를 같은 변경으로 저장한다. 전송·reply·acknowledge는 현재 read scope와 활성 run을 재검사한다. run이 끝난 sender를 이용한 새 request는 거부하고 원 request의 durable 결과 조회는 허용한다.

command_id는 명령 재시도 ID, message_id는 저장된 메시지 ID, correlation_id는 원 요청/메시지 ID다. 같은 command_id로 중복 전달 레코드를 만들지 않는다. 같은 message_id를 다른 body로 제출하면 COMMAND_ID_REUSE. delivery_id와 activation_key로 중복 소비를 판정한다. 전달은 durable at-least-once, 내부 결과 적용은 명령/종결 답변 중복 검사로 한 번이다. 외부 셸·웹 호출에 exactly-once를 선언하지 않는다.

자기 요청 금지, 중첩 hop_count 최대 16, 활성 요청의 wait-for graph에 순환이 생기면 REQUEST_CYCLE을 반환한다. sender와 recipient가 서로 대기하더라도 진행 report/receipt는 실행 슬롯 없이 처리한다. 수신 권한 철회/대상 삭제는 pending delivery cancelled, 요청은 나머지 completion_mode와 terminal 상태로 다시 판정한다. 재시작 시 open/claimed 요청의 deadline·owner epoch를 검사하고 live claim은 갱신, 죽은 claim은 interrupted/재배정으로 남긴다. Inbox는 recipient별 pending/관찰/완료를, Outbox는 sender의 메시지·응답·만료를 같은 read_seq로 조회한다.

공유 변경 전달 예: worker가 fact와 `FactRecorded` 및 planner delivery를 한 프레임으로 확정 → planner claim → SharedContextService에서 그 version을 조회 → 필요한 추가 검사를 request → 다른 agent report(progress) → reply(final) → planner의 새 intent와 처리 완료를 같은 변경으로 확정한다. UI가 꺼져 있어도 중앙 객체와 전달 기록으로 복구된다.

## 81개 도구의 실행 계약

모든 이름·입력 필드·중첩 배열·필수 필드·모델용 설명은 고정 원본과 대응표에 있다. v1 ID 전환 및 update_finding_report 변경을 제외한 기존 기능과 부분 성공 계약을 유지한다. 각 핸들러는 `ToolContext={workspace_id,task_id?,exploration_id?,intent_id?,conversation_id?,agent_id,run_id,policy_revision}`를 엔진에서 받아 input과 섞지 않는다. retest 도구와 source 읽기 권한도 이 context를 사용한다.

공통 ToolResult는 `{content:[ContentBlock],is_error:boolean,extra:[ConversationMessage]}`다. 원본 ContentBlock의 text/thinking/tool_use/tool_result를 유지하며 저장 text/thinking은 Body, provider 경계에서 원문 문자열로 복원한다. thinking signature와 tool_use id/name/input, tool_result tool_use_id/content/is_error를 보존한다. extra는 role=user/assistant와 content 목록이며 원본 순서로 tool_result 직후 삽입한다. 원본 tool_use_id와 연결되는 tool_result 메시지로 돌려준다. 입력/실행 오류도 is_error 결과와 ledger를 남기고 RPC 자체 오류와 구분한다. batch 도구는 원본 ids/errors 순서를 보존하고 실패 항목의 객체를 만들지 않는다. report_finding는 FindingId·NodeId·evidence_status를 구조 결과에 제공하며 호환 text 첫 줄은 NodeId임을 명시한다. 외부 도구 결과를 숨겨 실패를 성공으로 바꾸지 않는다.

| 도구군 | 구현을 확정한 경계 |
| --- | --- |
| 자산/그래프/목표/지침/취약점 | WorkspaceStore command와 SharedContextService, source 읽기 전용·종류별 ID·입력별 partial success. |
| 작업 지정/제어/보고서/재검증 | task target 권한, run 취소, 별도 finding/node 매핑, evidence CAS, conversation 결합 대상. |
| traffic_search/get/blob | 선택 capture 서비스의 metadata/body 조회. 원본 traffic_blob의 기본/최대 8 KiB는 모델 도구에서 유지하고 RPC blob.read 최대 64 KiB와 구분. |
| Read/Write/Edit/MultiEdit/LS/Glob/Grep | 선택된 작업 폴더와 명시적 capability 검사, 기존 상대경로/검색·행 의미 유지. MultiEdit의 순서·매치 검사 후 같은 파일의 최종 내용을 원자 교체. |
| Bash/Monitor/TaskOutput/TaskStop/TaskList | OS 프로세스 감독, durable execution_id/invocation, 출력 커서·최종 exit code·취소. Windows는 PowerShell, macOS/Arch는 Bash. |
| shell_open/send/read/close/list | Windows ConPTY+Job object, Unix PTY+프로세스 그룹. shell session은 run 소유. cursor 범위 밖은 CURSOR_EXPIRED. drain read는 해당 session 직렬화. |
| WebFetch/web_search | 설정된 검색 백엔드/네트워크·proxy·타임아웃, 취소·크기 제한. 원본 Python/DDGS 등 선택 환경은 호스트에서 고정된 실행 환경으로 관리. |
| Skill/create_skill/update_skill_file | 이름·상대 경로 검증, UTF-8 파일을 resource_files+blob로 확정하고 검증된 materialization 제공. File system 직접 수정으로 가시성 정책을 우회하지 않음. |
| create/update custom tool·MCP | 기존 kind/exec/schema/transport/visibility를 유지. command/script/http, stdio/http/sse. 변경 후 실행은 새 config revision 검사, 키는 SecretRef. |
| SearchExtraTools/ExecuteExtraTool | agent별 발견된 이름·스키마 공개와 run unlock set. Execute는 발견·바인딩·승인·schema revision 재검사. |
| TodoWrite/sleep/Compress | Run.todos의 content/status 목록 교체·취소 가능한 타이머·원본 대화 보관 후 요약. todo status는 pending/in_progress/completed. Compress는 raw input 관대한 parser 의미를 유지하고 단순 JSON Schema 검사로 원본의 배열/문자열 입력을 배제하지 않음. |

기존 미연결 SDK 5개는 **구현, 기본 비활성화, agent별 opt-in**으로 확정한다. agent_features의 questions/plan_mode/memory를 각각 false로 초기화하며 설정을 켜야 도구를 조립한다. AskUserQuestion은 1~4 질문·선택·자유 입력 UI를 user_questions에 저장하고 기본 24시간 대기/취소를 제공한다. pending은 실행 슬롯을 점유하지 않는다. EnterPlanMode는 run의 권한 모드를 plan으로 저장해 읽기와 질문만 허용한다. ExitPlanMode는 계획을 저장하고 사용자 응답이 승인일 때만 기존 prePlan/target 모드로 전환한다. 원본 nil approver 자동 승인 경로는 데스크톱에서 제공하지 않는다. run 권한 enum은 default/acceptEdits/bypassPermissions/plan/dontAsk/auto이며 사용자 정책 없이 agent가 bypass로 승격할 수 없다.

RecallMemory/RecordMemory는 agent별 memory_records의 name/description/type/content를 조회·upsert하며 user/feedback/project/reference와 version을 검사한다. type 미지정은 project, name은 trim 후 비어 있지 않은 Unicode 문자열이고 고유 키는 agent_id+name+type이다. 한국어 이름을 SDK의 ASCII slug 파일명으로 축약해 충돌시키지 않는다. 다른 agent의 기억을 자동 노출하지 않고 공유는 명시적 객체/메시지로 한다. 동일 name/type upsert의 충돌은 version 비교로 처리하고 세션 이후에도 보존한다. 현재 미연결 기능이므로 이 명시적 이름/저장 규칙을 신규 계약으로 적용한다. goal_met은 구현하되 원본처럼 기본 무바인딩으로 두며 수동 정책 활성화가 필요하다.

한 invocation의 중복 판정 키는 run_id+tool_use_id+ordinal이다. 객체 변경 도구는 invocation 결과·도메인 변경·이벤트를 한 프레임으로 저장한다. 외부 효과 도구는 prepared→승인→host.execute→running→completed/failed 기록으로 진행하며 prepared가 외부 실행 완료를 뜻하지 않는다. 같은 execution_id 재전송은 상태 조회/기존 결과 반환, 내용 변경은 거부다. 승인 범위가 바뀌면 실행 직전에 재검사한다. run 종료 이후 새 tool/PTY/추가 도구 unlock은 허용하지 않는다.

read 도구는 불변 같은 세대에서 병렬 실행 가능하나 plan mode 변경·drain read·문맥 압축·메모리 쓰기·TodoWrite는 직렬화한다. 원본 IsReadOnly({})/IsConcurrencySafe({}) 추출 값은 샘플이며 입력별 권한·상태 변경을 판단하는 근거로 쓰지 않는다. 파일·셸은 OS 권한으로 실행되며 capability 검사가 OS 샌드박스라는 의미는 아니다.

## 가져오기와 구현 통과 조건

가져오기는 원본 DB를 변경하지 않는 read-only repeatable-read export, 트래픽 recorder 정지 또는 일관된 SQLite 백업과 본문 파일 확보로 시작한다. 실제 스키마 column 목록을 계약과 대조해 누락/추가 열을 migration report에 기록한다. legacy_id_map의 key는 `{source_instance,collection,legacy_primary_key}`이고 BIGINT는 문자열, 복합 키는 필드 순서가 고정된 JSON 배열이다. 재시도는 같은 매핑을 재사용하며 동일 ID 객체를 중복 생성하지 않는다.

회사의 두 참조, source relation, payload 내부 ID, findings asset_ids, snapshot hash, bindings, task archives, usage 출처, 대화/run와 sidequestion generation을 이차 검증한다. staged 작업 공간에서 개수·관계·자연 키·blob 크기/해시·unknown field 보존을 확인한 후 project/manifest를 설치한다. 원본 schema가 없는 선택 llm_records/usage는 missing으로 기록하고 빈 컬렉션으로 생성한다. 원본 개인 키와 비밀은 OS 저장소로 옮기고 작업 파일/내보내기에 넣지 않는다. Linux Secret Service 미사용 시 입력을 세션에만 보관하고 pending secret refs로 비활성화하며 평문 파일 fallback을 만들지 않는다.

| 수락 시나리오 | 필요한 결과 |
| --- | --- |
| C01 전체 자료형/54개 컬렉션 round-trip | 571개 필드의 NULL/배열/큰 정수/시각/확장 JSON/비밀 참조 검증, 원본 불변식 일치. |
| C02 81개 도구 normal/invalid/batch | 원본 결과/partial success 대조, v1 ID 변경 명시, 모든 이름 등록, 정책별 제공 목록 일치. |
| C03 source A→B→C | A에 B만 보임, source 변경·해제 반영, terminal trace 필터·thinking/usage 제외·쓰기 거부. |
| C04 finding/report/evidence | 두 ID 혼동 거부, evidence_version 충돌, capture 삭제 이후 증거 보존, 원자적 retest fixed. |
| C05 저장 경계별 강제 종료 | blob/append/footer/sync/메모리/응답/checkpoint 각 경계에서 성공 응답된 변경 유지; 미확인 명령 영수증 재조회. |
| C06 tail/중간 손상/두 세대 | 미완성 tail만 제외, 완성 프레임 손상은 read_only, 이전 세대+저널로 최신 복구, 누락을 명시. |
| C07 메시지·트리거 | 저장 후 wake-up 전 종료, claim 중 종료, duplicate send/reply, progress와 final 구분, one/all·만료·순환 요청·권한 철회. |
| C08 외부 도구 | 세 OS의 timeout/프로세스 그룹 정리/PTY 키·UTF-8·출력 cursor, 재시작 불명확 실행 자동 재실행 금지. |
| C09 질문/계획/기억 | 5개 opt-in 기본 off, UI 재접속 후 질문 복원, 실제 승인 전 쓰기 금지, 기억 충돌·agent 분리. |
| C10 이전/보관/재시도 | legacy map 재사용·54컬렉션 개수 대조·v1~v3 archive·secret 분리·실패 staging 보존. |
| C11 화면/API | 261 route disposition·258 facade 함수의 커버리지, HTTP/JWT/SSE 교체, 큰 본문 capability, 이벤트 gap 재조회. |

수락 시나리오는 구현과 검증의 시험 계약이다. Rust/Tauri 구현과 Windows fixture의 최신 결과는 desktop/implementation-status.md, desktop/feature-parity.md 및 desktop/tests/verification-result.json에 기록한다. 다른 OS 패키지·장시간·절전·물리 파일 시스템 장애 검증을 Windows 검사로 대신하지 않는다.

구현 단위는 desktop/crates의 core·store·rpc·agent·runtime·tools·host·importer·migrate와 app/web이다. 데이터/RPC/저장/내부 통신/81개 핸들러 계약은 v1로 고정하고 데스크톱 추가 계약은 정본과 사본을 함께 갱신한다. 최소 OS·배포 서명·수치 성능 기준은 패키지 검증의 릴리스 입력이며 위 계약을 미정으로 되돌리지 않는다.

### 구현에서 확정한 데스크톱 경계

- 역할 기본 프롬프트는 번역된 Go 상수 연결식 전체와 일치해야 한다. Reporter 이전 시스템 기본값의 알려진 잘림만 다음 버전으로 보정하며 사용자 텍스트·삭제·이력은 보존한다. 저장된 사용자 템플릿이 빈 문자열이거나 렌더링에 실패하면 역할 기본값으로 대체한다. 실제 실행 변수만 사용하고 미리보기 값은 주입하지 않는다.
- 제약(계획자/작업자 설정), 파일 출력, 증거 ID/CAS·트래픽 설정, 목표 범위, 승인된 지연 도구 목록은 코드가 실행 문맥에 맞춰 덧붙인다. 현재 파일 실행 경로는 작업별 공유 폴더이며 원본의 intent별 독점 폴더와 다르다. 작업자에게 run ID별 파일명과 공유 파일 보존을 지시한다.
- Run의 선택 필드 context_snapshot에는 승인된 skill_catalog_names, Skill 메타데이터(name/description/mcps), 선택 skill_catalog_error 코드만 저장한다. 필드가 없는 이전 Run도 유효하다. 네이티브 카탈로그 조회는 비동기로 실행하고 epoch와 작업 공간 세대가 바뀐 결과를 폐기한다. 현재 공개 설정은 매 호출에 다시 확인한다.
- Skill은 승인 리소스를 내용 해시별 .artex-skills 폴더로 실제 실행 폴더에 고정한다. 성공한 Skill 영수증과 독립 사용자 지침을 하나의 커밋에 저장하고 결과 뒤에 전달한다. 원문은 Blob·체크포인트·문맥 압축 후에도 보존한다. MCP 잠금 해제는 같은 허용 대화/직접 부모 문맥의 완료된 Skill 영수증과 이름이 일치하는 지침으로만 복원하며, 명시적 도구 거부와 공개 취소를 우선한다.
- 저장된 도구 스키마의 scalar 기본값은 생략/null/빈 문자열에만 적용한다. 명시적인 0/false/문자열을 유지하고 객체·배열 기본값은 주입하지 않는다. 실제 검증·승인·네이티브 실행 입력이 같은 규칙을 사용한다.

- 정상 workspace.close/system.shutdown은 작업을 paused로 전환하고 마지막 활동 시간의 분율을 같은 CAS에 확정한다. 미확정 외부 효과는 recovery_required로 남기며 체크포인트 성공 후 OS writer 잠금을 해제한다. 종료 오류를 성공 응답으로 숨기지 않는다.
- 비정상 종료는 기존 epoch의 queued/running/waiting run을 interrupted로 기록하고 작업을 일시정지한다. task.recover_budget 및 ui.recoverTaskBudget는 사용자만 호출하며 use_recorded 또는 누적값보다 작지 않은 override로 불명확 시간을 확인한다. 복구 후 task.resume는 별도로 명시한다.
- 공급자 스트리밍 진행은 run당 64개/각 16KiB, 전체 16MiB 큐에 제한한다. 부분 활동에는 재실행할 provider message나 tool input을 넣지 않는다. 검증된 최종 assistant message와 부분 삭제는 같은 커밋이며 ActivityReplaced 사건으로 삭제된 UI cursor를 재시작 후에도 해석한다. 실패/취소 부분은 진단으로만 유지한다.
- tool argument는 완성된 Turn 검증 전 실행하지 않는다. 공급자가 이미 의미 있는 출력을 보낸 실패는 이전 의도를 자동 재요청하지 않는다. Thinking/signature/usage는 grouped provider message를 통해 source trace나 내부 object reference로 유출하지 않는다.
- NOA는 run마다 설정을 한 번 캡처한다. 최근 5개 메시지와 마지막 5,000토큰 및 tool pair를 보호한다. 압축 원문은 해시 blob/archive로 보존하고 자동 요약은 도구 없는 실제 모델 요청 후 원본 fingerprint CAS를 확인한다.
- 큰 native 입력/결과는 소유 문맥에 묶인 hash/length transfer로 최대 64MiB, 청크 최대 64KiB로 이동한다. WebView에 임의 파일 경로 capability·비밀 해석·raw tool·agent 발신자 권한을 노출하지 않는다.
- 오프라인 artex-migrate는 원본 JSON·archive·PostgreSQL/SQLite를 읽고 새 staged workspace에 검증 후 설치한다. 키는 OS vault에 넣으며 unavailable/pending 키는 재입력이 필요하다. 목적지를 덮어쓰지 않고 실패 staging을 재사용한다.
- 화면의 편집 기준 버전은 해당 DTO를 읽은 세대에 고정한다. 백그라운드 조회는 편집 기준을 갱신하지 않으며, 성공한 자기 저장만 기준을 진행시킨다. 충돌 후 재제출은 차단하고 사용자가 입력을 버리는 명시적 재조회로 복구한다. 봉인된 MCP 환경/추가 도구 헤더는 UI와 SDK·이전·Store 검증 경계에서 같은 SecretRef 규칙을 적용한다.

재생성은 `node scripts/generate-desktop-contracts.mjs`, `node scripts/define-desktop-contracts.mjs`, 검사에는 `node scripts/validate-desktop-contracts.mjs`와 `node scripts/check-language.mjs`를 사용한다. Go 정의를 다시 추출할 때는 scripts/desktop-tools-export_test.go.txt를 임시 server/desktop_contract_dump_test.go로 복사하고 ARTEX_CONTRACT_DUMP를 tools-source.v1.json 경로, ARTEX_PG_DSN을 파싱 불가능한 contract-export-no-database로 지정해 `go test ./server -run '^TestDesktopContractDump$' -count=1 -v`만 실행한 뒤 임시 파일을 제거한다. 이 추출기는 생성자만 호출하며 도구/외부 명령/DB 변경을 실행하지 않는다.
