# 데스크톱 도구 대응 명세

2026-10-12. 정의는 ARTEX 51개 + SDK 30개 = **81개**다. 사용자 도구·MCP에서 추가 발견한 도구와 새 내부 메시지 RPC는 이 숫자에 포함하지 않는다. 모든 81개 핸들러를 구현 대상으로 확정한다. 현재 미연결 SDK 5개는 선택 기능으로 구현하고 기본 비활성화한다. 실제 제공 목록은 실행 문맥·에이전트 바인딩·기능 설정·권한 검사를 모두 통과한 교집합이다.

[상세 구현 명세](desktop-implementation-spec.md)의 공통 결과·권한·저장·중복·취소 규칙을 적용한다. 전체 원본 입력 스키마와 v1 스키마, 기본 도메인 바인딩, 소스 위치, 구현 모듈은 [tools.v1.json](contracts/tools.v1.json)에 있다. 빈 입력으로 조사한 read_only/concurrent 값은 참고 자료이며 실제 호출의 권한 또는 병렬성 판정으로 사용하지 않는다.

| 번호 | 이름 | 기능 | Rust 핸들러 | 원본 기본 도메인 바인딩/제공 경로 | 효과 | Go 근거 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `add_company_scope` | 회사와 회사별 자산 범위를 등록·보강 | `domain::assets::add_company_scope` | mainagent, planner, auto | workspace_command | [agent/tools_insert.go:378](../../original/agent/tools_insert.go#L378) |
| 2 | `add_hint` | 현재 작업에 계획 지침과 증거 연결 추가 | `domain::exploration::add_hint` | mainagent | workspace_command | [agent/tools.go:1617](../../original/agent/tools.go#L1617) |
| 3 | `add_intent` | 실행할 의도와 부모·자산 관계 생성 | `domain::exploration::add_intent` | mainagent, planner | workspace_command | [agent/tools.go:1092](../../original/agent/tools.go#L1092) |
| 4 | `add_task_hint` | 지정 작업에 지침 전달 | `domain::tasks::add_task_hint` | auto | workspace_command | [server/orchestration.go:367](../../original/server/orchestration.go#L367) |
| 5 | `add_task_scope` | 현재 작업의 실행 대상 범위 추가 | `domain::assets::add_task_scope` | mainagent, planner | workspace_command | [agent/tools_insert.go:432](../../original/agent/tools_insert.go#L432) |
| 6 | `bind_finding_traffic` | 취약점에 트래픽 증거를 복사·연결 | `domain::findings::bind_finding_traffic` | auto, reporter | workspace_command | [server/finding_workflow.go:146](../../original/server/finding_workflow.go#L146) |
| 7 | `create_custom_tool` | 명령·스크립트·HTTP 사용자 도구 등록 | `domain::configuration::create_custom_tool` | auto | workspace_command | [server/platform_tools.go:198](../../original/server/platform_tools.go#L198) |
| 8 | `create_mcp` | MCP 서버 설정 등록 | `domain::configuration::create_mcp` | auto | workspace_command | [server/platform_tools.go:295](../../original/server/platform_tools.go#L295) |
| 9 | `create_skill` | Skill 지침 파일 생성 | `domain::configuration::create_skill` | auto | workspace_command | [server/platform_tools.go:82](../../original/server/platform_tools.go#L82) |
| 10 | `delete_assets_by_host` | 호스트에 속한 자산 삭제 | `domain::assets::delete_assets_by_host` | auto | workspace_command | [server/platform_tools.go:43](../../original/server/platform_tools.go#L43) |
| 11 | `expand_digest` | 압축 요약의 원본 구성 노드 조회 | `domain::exploration::expand_digest` | mainagent, planner | read | [agent/tools_digest.go:121](../../original/agent/tools_digest.go#L121) |
| 12 | `get_finding_retest_context` | 현재 재검증의 취약점·증거 문맥 조회 | `domain::retests::get_finding_retest_context` | retester | read | [server/finding_retests.go:146](../../original/server/finding_retests.go#L146) |
| 13 | `get_finding_traffic` | 취약점의 증거 목록과 본문 구간 조회 | `domain::findings::get_finding_traffic` | auto, reporter, worker, planner, mainagent, pentest | read | [server/finding_traffic.go:369](../../original/server/finding_traffic.go#L369) |
| 14 | `get_task_graph` | 지정 작업의 탐색 그래프 조회 | `domain::tasks::get_task_graph` | auto, reporter | read | [server/orchestration.go:351](../../original/server/orchestration.go#L351) |
| 15 | `get_task_node_detail` | 지정 작업의 노드 상세 조회 | `domain::tasks::get_task_node_detail` | auto, reporter | read | [server/orchestration.go:411](../../original/server/orchestration.go#L411) |
| 16 | `get_task_worker_trace` | 지정 작업의 의도 실행 기록 조회 | `engine::workers::get_task_worker_trace` | auto, reporter | read | [server/orchestration.go:382](../../original/server/orchestration.go#L382) |
| 17 | `get_worker_output` | 의도의 실행 결과 조회 | `engine::workers::get_worker_output` | mainagent, planner | read | [agent/tools.go:1741](../../original/agent/tools.go#L1741) |
| 18 | `get_worker_trace` | 의도별 실행 기록 필터 조회 | `engine::workers::get_worker_trace` | mainagent, planner, worker | read | [agent/tools.go:1822](../../original/agent/tools.go#L1822) |
| 19 | `goal_met` | 현재 작업의 전체 목표 달성 선언 | `domain::exploration::goal_met` | 별도 조립·정책 | workspace_command | [agent/tools.go:1206](../../original/agent/tools.go#L1206) |
| 20 | `graph_overview` | 현재·직접 원본 작업의 그래프 요약 조회 | `domain::exploration::graph_overview` | mainagent, planner | read | [agent/tools.go:342](../../original/agent/tools.go#L342) |
| 21 | `insert_assets` | 유형별 자산 병합 등록과 출처 연결 | `domain::assets::insert_assets` | mainagent, worker, auto, pentest | workspace_command | [agent/tools_insert.go:123](../../original/agent/tools_insert.go#L123) |
| 22 | `kill_work` | 실행 의도 중단 | `engine::workers::kill_work` | planner | workspace_command | [agent/tools.go:1677](../../original/agent/tools.go#L1677) |
| 23 | `list_assets` | 자산 ID·유형·DSL 조건 검색 | `domain::assets::list_assets` | mainagent, planner, worker, auto, pentest | read | [agent/tools_insert.go:531](../../original/agent/tools_insert.go#L531) |
| 24 | `list_companies` | 회사 이름 검색 | `domain::exploration::list_companies` | planner, auto, pentest | read | [agent/tools_insert.go:588](../../original/agent/tools_insert.go#L588) |
| 25 | `list_facts` | 현재·직접 원본 작업의 사실 검색 | `domain::exploration::list_facts` | mainagent, planner | read | [agent/tools.go:940](../../original/agent/tools.go#L940) |
| 26 | `list_findings` | 현재·직접 원본 작업의 취약점 조회 | `domain::findings::list_findings` | mainagent, planner, worker, pentest | read | [agent/tools.go:902](../../original/agent/tools.go#L902) |
| 27 | `list_goals` | 현재 목표와 달성 상태 조회 | `domain::exploration::list_goals` | planner | read | [agent/tools.go:1149](../../original/agent/tools.go#L1149) |
| 28 | `list_llm_profiles` | 사용 가능한 모델 프로필 조회 | `domain::tasks::list_llm_profiles` | auto | read | [server/orchestration.go:223](../../original/server/orchestration.go#L223) |
| 29 | `list_task_findings` | 지정 작업의 취약점 조회 | `domain::findings::list_task_findings` | auto, reporter | read | [server/orchestration.go:359](../../original/server/orchestration.go#L359) |
| 30 | `list_task_worker_traces` | 지정 작업의 실행 기록 목록 조회 | `engine::workers::list_task_worker_traces` | auto, reporter | read | [server/orchestration.go:395](../../original/server/orchestration.go#L395) |
| 31 | `list_tasks` | 전체 작업 목록 조회 | `domain::tasks::list_tasks` | auto | read | [server/orchestration.go:177](../../original/server/orchestration.go#L177) |
| 32 | `list_untested_assets` | 현재 범위의 미검사 자산 조회 | `domain::assets::list_untested_assets` | mainagent, planner | read | [agent/tools_insert.go:488](../../original/agent/tools_insert.go#L488) |
| 33 | `node_detail` | 허용된 노드와 연결·상세 조회 | `domain::exploration::node_detail` | mainagent, planner, worker | read | [agent/tools.go:993](../../original/agent/tools.go#L993) |
| 34 | `pause_task` | 지정 작업을 일시 정지 | `domain::tasks::pause_task` | auto | workspace_command | [server/orchestration.go:332](../../original/server/orchestration.go#L332) |
| 35 | `prove_goal` | 사실·취약점 증거로 목표 달성 입증 | `domain::exploration::prove_goal` | planner | workspace_command | [agent/tools.go:1158](../../original/agent/tools.go#L1158) |
| 36 | `record_fact` | 의도 결과를 사실·자산 관계로 저장 | `domain::exploration::record_fact` | mainagent, worker | workspace_command | [agent/tools.go:1354](../../original/agent/tools.go#L1354) |
| 37 | `record_finding_retest_result` | 재현·수정·불확정 판정과 증거 저장 | `domain::retests::record_finding_retest_result` | retester | workspace_command | [server/finding_retests.go:170](../../original/server/finding_retests.go#L170) |
| 38 | `report_finding` | 취약점 노드·레코드·증거를 함께 등록 | `domain::findings::report_finding` | mainagent, planner, worker, auto, pentest | workspace_command | [agent/tools.go:1220](../../original/agent/tools.go#L1220) |
| 39 | `search_all_worker_traces` | 허용된 실행 기록에서 문자열 검색 | `engine::workers::search_all_worker_traces` | mainagent, planner, worker | read | [agent/tools.go:1931](../../original/agent/tools.go#L1931) |
| 40 | `search_task_worker_traces` | 지정 작업의 실행 기록 검색 | `engine::workers::search_task_worker_traces` | auto, reporter | read | [server/orchestration.go:403](../../original/server/orchestration.go#L403) |
| 41 | `set_constraints` | 허용·금지 제약 추가 | `domain::exploration::set_constraints` | mainagent, goals | workspace_command | [agent/tools.go:1568](../../original/agent/tools.go#L1568) |
| 42 | `set_goals` | 목표·취약점 분류 추가 | `domain::exploration::set_goals` | mainagent, goals | workspace_command | [agent/tools.go:1475](../../original/agent/tools.go#L1475) |
| 43 | `spawn_task` | 독립 작업·직접 원본 관계 생성 후 실행 예약 | `domain::tasks::spawn_task` | auto | workspace_command | [server/orchestration.go:242](../../original/server/orchestration.go#L242) |
| 44 | `steer_work` | 현재 실행의 다음 도구 호출 전 수정 지시 | `engine::workers::steer_work` | mainagent, planner | workspace_command | [agent/tools.go:1707](../../original/agent/tools.go#L1707) |
| 45 | `traffic_blob` | 해시 본문의 구간 읽기 | `domain::traffic::traffic_blob` | worker, reporter | read | [traffic/traffic.go:2079](../../original/traffic/traffic.go#L2079) |
| 46 | `traffic_get` | 트래픽 요청·응답 상세 조회 | `domain::traffic::traffic_get` | worker, reporter | read | [traffic/traffic.go:2058](../../original/traffic/traffic.go#L2058) |
| 47 | `traffic_search` | 호스트·문자열·본문 조건으로 트래픽 검색 | `domain::traffic::traffic_search` | worker, reporter | read | [traffic/traffic.go:2007](../../original/traffic/traffic.go#L2007) |
| 48 | `update_custom_tool` | 사용자 도구 정의와 바인딩 수정 | `domain::configuration::update_custom_tool` | auto | workspace_command | [server/platform_tools.go:224](../../original/server/platform_tools.go#L224) |
| 49 | `update_finding_report` | 발견 노드에 대응하는 취약점 보고서 수정 | `domain::findings::update_finding_report` | auto, reporter | workspace_command | [server/orchestration.go:427](../../original/server/orchestration.go#L427) |
| 50 | `update_mcp` | MCP 설정 수정 | `domain::configuration::update_mcp` | auto | workspace_command | [server/platform_tools.go:313](../../original/server/platform_tools.go#L313) |
| 51 | `update_skill_file` | Skill 상대 경로 파일 수정 | `domain::configuration::update_skill_file` | auto | workspace_command | [server/platform_tools.go:124](../../original/server/platform_tools.go#L124) |
| 52 | `AskUserQuestion` | 사용자에게 선택 질문을 제시하고 응답 대기 | `tools::interaction::ask_user_question` | 선택 활성화 | run_state | [norma/tool/ask.go:43](../../original/norma/tool/ask.go#L43) |
| 53 | `Bash` | 단일 셸 명령 실행·백그라운드 실행 | `tools::process::bash` | 별도 조립·정책 | external_effect | [norma/tool/bash.go:48](../../original/norma/tool/bash.go#L48) |
| 54 | `Compress` | 원본 대화를 보관하고 범위를 요약으로 교체 | `engine::context::compress` | 별도 조립·정책 | run_state | [norma/noa/compress.go:10](../../original/norma/noa/compress.go#L10) |
| 55 | `Edit` | 파일의 일치 문자열 교체 | `tools::files::edit` | 별도 조립·정책 | external_effect | [norma/tool/files.go:197](../../original/norma/tool/files.go#L197) |
| 56 | `EnterPlanMode` | 에이전트를 계획 권한 모드로 전환 | `tools::interaction::enter_plan_mode` | 선택 활성화 | run_state | [norma/plan/plan.go:83](../../original/norma/plan/plan.go#L83) |
| 57 | `ExecuteExtraTool` | 검색으로 해제한 추가 도구 실행 | `tools::deferred::execute_extra_tool` | 별도 조립·정책 | run_state | [norma/tool/deferred.go:18](../../original/norma/tool/deferred.go#L18) |
| 58 | `ExitPlanMode` | 계획을 제시하고 사용자 승인 후 권한 모드 복원 | `tools::interaction::exit_plan_mode` | 선택 활성화 | run_state | [norma/plan/plan.go:102](../../original/norma/plan/plan.go#L102) |
| 59 | `Glob` | 경로에서 파일 이름 패턴 검색 | `tools::files::glob` | 별도 조립·정책 | read | [norma/tool/search.go:20](../../original/norma/tool/search.go#L20) |
| 60 | `Grep` | 파일 내용을 정규식으로 검색 | `tools::files::grep` | 별도 조립·정책 | read | [norma/tool/search.go:141](../../original/norma/tool/search.go#L141) |
| 61 | `LS` | 디렉터리 항목 조회 | `tools::files::ls` | 별도 조립·정책 | read | [norma/tool/ls.go:15](../../original/norma/tool/ls.go#L15) |
| 62 | `Monitor` | 명령 출력을 감시하고 조건 충족 알림 | `tools::process::monitor` | 별도 조립·정책 | external_effect | [norma/tool/monitor.go:15](../../original/norma/tool/monitor.go#L15) |
| 63 | `MultiEdit` | 파일에 여러 문자열 교체를 순서대로 적용 | `tools::files::multi_edit` | 별도 조립·정책 | external_effect | [norma/tool/multiedit.go:16](../../original/norma/tool/multiedit.go#L16) |
| 64 | `Read` | 파일의 행 구간 읽기 | `tools::files::read` | 별도 조립·정책 | read | [norma/tool/files.go:16](../../original/norma/tool/files.go#L16) |
| 65 | `RecallMemory` | 에이전트의 장기 기억 검색 | `tools::memory::recall_memory` | 선택 활성화 | read | [norma/memory/tools.go:22](../../original/norma/memory/tools.go#L22) |
| 66 | `RecordMemory` | 분류된 장기 기억 저장 | `tools::memory::record_memory` | 선택 활성화 | workspace_command | [norma/memory/tools.go:53](../../original/norma/memory/tools.go#L53) |
| 67 | `SearchExtraTools` | 추가 도구 검색과 실행 스키마 공개 | `tools::deferred::search_extra_tools` | 별도 조립·정책 | run_state | [norma/tool/deferred.go:17](../../original/norma/tool/deferred.go#L17) |
| 68 | `Skill` | 허용된 Skill 지침 로드 | `tools::skills::skill` | 별도 조립·정책 | read | [norma/skill/skill.go:103](../../original/norma/skill/skill.go#L103) |
| 69 | `TaskList` | 현재 실행의 백그라운드 명령 목록 조회 | `tools::process::task_list` | 별도 조립·정책 | read | [norma/tool/tasklist.go:13](../../original/norma/tool/tasklist.go#L13) |
| 70 | `TaskOutput` | 백그라운드 명령 출력 조회·완료 대기 | `tools::process::task_output` | 별도 조립·정책 | run_state | [norma/tool/taskoutput.go:15](../../original/norma/tool/taskoutput.go#L15) |
| 71 | `TaskStop` | 백그라운드 명령 중단 | `tools::process::task_stop` | 별도 조립·정책 | external_effect | [norma/tool/taskstop.go:12](../../original/norma/tool/taskstop.go#L12) |
| 72 | `TodoWrite` | 현재 실행의 할 일 목록 교체 | `tools::todos::todo_write` | 별도 조립·정책 | run_state | [norma/tool/todo.go:55](../../original/norma/tool/todo.go#L55) |
| 73 | `WebFetch` | 웹 문서를 요청하고 텍스트 추출 | `tools::web::web_fetch` | 별도 조립·정책 | external_effect | [norma/tool/webfetch.go:46](../../original/norma/tool/webfetch.go#L46) |
| 74 | `Write` | 파일 내용 저장 | `tools::files::write` | 별도 조립·정책 | external_effect | [norma/tool/files.go:148](../../original/norma/tool/files.go#L148) |
| 75 | `shell_close` | PTY 세션과 프로세스 종료 | `tools::pty::shell_close` | 별도 조립·정책 | external_effect | [norma/tool/shell_tools.go:312](../../original/norma/tool/shell_tools.go#L312) |
| 76 | `shell_list` | 현재 실행의 PTY 세션 목록 조회 | `tools::pty::shell_list` | 별도 조립·정책 | read | [norma/tool/shell_tools.go:336](../../original/norma/tool/shell_tools.go#L336) |
| 77 | `shell_open` | 대화형 PTY 세션 시작 | `tools::pty::shell_open` | 별도 조립·정책 | external_effect | [norma/tool/shell_tools.go:89](../../original/norma/tool/shell_tools.go#L89) |
| 78 | `shell_read` | PTY 출력 커서·화면 읽기 | `tools::pty::shell_read` | 별도 조립·정책 | run_state | [norma/tool/shell_tools.go:263](../../original/norma/tool/shell_tools.go#L263) |
| 79 | `shell_send` | PTY에 텍스트·키·바이트 전송 | `tools::pty::shell_send` | 별도 조립·정책 | external_effect | [norma/tool/shell_tools.go:180](../../original/norma/tool/shell_tools.go#L180) |
| 80 | `sleep` | 취소 가능한 시간 대기 | `tools::timer::sleep` | 별도 조립·정책 | run_state | [norma/tool/sleep.go:19](../../original/norma/tool/sleep.go#L19) |
| 81 | `web_search` | 설정된 검색 백엔드로 웹 검색 | `tools::web::web_search` | 별도 조립·정책 | external_effect | [norma/tool/websearch.go:77](../../original/norma/tool/websearch.go#L77) |

`workspace_command`는 객체·관계·이벤트를 같은 저장 명령으로 확정한다. `external_effect`는 외부 실행 ID와 승인·완료 기록을 사용한다. `run_state`는 문맥·대기·권한 모드를 변경하며 읽기 전용 도구처럼 병렬 실행하지 않는다. SDK 기본 도구는 별도 실행 조립 조건을 따른다. 예를 들어 worker는 기본 MultiEdit·Glob·Grep를 제외하며, PTY·검색·트래픽은 해당 기능 활성화가 필요하다. 플랫폼·재검증 도구의 빈 바인딩은 미구현을 뜻하지 않는다.
