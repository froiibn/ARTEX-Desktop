# 2026-10-17 재검증 결함 수정

기준 commit은 `a585764e9a88f5b3e4f39c5a787e4080b6b4c610`이다. 이 commit 이후 작업 트리에 있던 F01~F15 수정, 작업 규칙과 문서 변경을 입력으로 보존하고, 두 번째 원본 대조에서 재현한 R01~R09 및 마무리 실행의 사용자 도구 노출 불일치를 수정했다. Rust 단일 앱, 객체·파일 저장, 영문 프롬프트, 승인·중지 정책 등 합의된 재설계는 되돌리지 않았다. 커밋·푸시·태그·릴리스 변경은 이 작업에 포함하지 않는다.

작업 시작 파일 해시와 원본 manifest 검증 결과는 저장소 밖 `.organization/revalidation-fixes-20261017/start.json`, 수정 전 재현과 정상 대조는 `.organization/revalidation-20261017/report.md` 및 하위 실행 기록으로 추적한다. 원본은 고정 manifest의 287개 파일을 대조하며 수정하지 않는다. 사용자 프로필·작업 공간·키·실제 사이트·실제 LLM 대신 격리된 저장소와 로컬 공급자/MCP fixture를 사용한다.

## 수정 범위와 검증 기준

| 항목 | 원본 의미·수정 원인 | 현재 수정과 회귀 기준 |
| --- | --- | --- |
| R01 MCP 장시간 SSE | `mcphttp/client.go`의 legacy GET 스트림은 도구 호출 사이에도 살아 있다. HTTP client 전체 수명 제한이 30초 후 연결을 끊었다. | `host/mcp.rs`에서 retained GET의 전체 수명 제한을 제거하고 handshake·각 응답 대기는 기존 30초 예산으로 제한한다. 실제 32초 유휴 후 counter 유지, 일반 HTTP 대조, 무응답 timeout·세션 교체와 취소를 확인한다. |
| R02 deadline의 미완결 도구 쌍 | `norma/llm/boundary.go::PairToolBlocks`는 응답 없는 호출을 공급자 요청에서 제외한다. 마무리 실행이 실행 중인 부모 호출을 그대로 재전송했다. | `runtime/request_messages.rs`의 공통 요청 view를 세 공급자 인코딩에 적용한다. 체크포인트 재개방 후 미완결 쌍 제외·확정된 쌍/텍스트 보존·부모 원장 불변·늦은 실제 완료 저장을 확인한다. |
| R03 정상 JSON-RPC 오류 | `norma/mcp`는 정상 오류 응답 후에도 연결을 유지한다. 모든 `Err`를 연결 손상으로 처리했다. | 요청 ID가 일치하고 형식이 유효한 JSON-RPC 오류만 세션을 유지한다. stdio/HTTP/SSE counter 1→오류→2, malformed/단절 시 폐기, 불확실한 호출 자동 재전송 금지를 확인한다. |
| R04 WebFetch 본문 유실 | `norma/tool/webfetch.go::htmlToMarkdown`은 공백 정규화 후 출력 한도를 적용한다. 빈 DOM 컨테이너의 공백이 50,000바이트를 소진했다. | 반복 DOM 순회를 유지하고 정규화한 가시 텍스트에 기존 바이트 예산을 적용한다. 13,000개 빈 div 앞의 본문, 일반 페이지·링크·pre 공백과 UTF-8 잘림 표시를 확인한다. |
| R05 Responses 부분 응답 | `norma/llm/openai_responses.go`는 출력 한도의 부분 응답·사용량·`max_tokens` 이유를 보존한다. JSON 경로가 이를 재시도 가능한 미생성 오류로 처리했다. | JSON/SSE 출력 한도 응답의 실제 텍스트·사용량·종료 이유를 보존한다. 실제 생성이 관측된 JSON 오류도 중복 요청하지 않으며, 잘린 도구 인수는 실행 가능한 호출로 바꾸지 않는다. 요청 수와 정상·오류 대조를 확인한다. |
| R06 독립 질문 부분 답변 | `sidequestion/service.go`는 실패 전에 관측한 답변과 사용량을 반환한다. 현재 실패 저장 분기가 빈 답변으로 바꿨다. | SSE에서 실제 답변 텍스트를 오류에 보존하고 writer가 failed 답변으로 저장한다. 요약 중간 결과는 답변으로 노출하지 않는다. 정상·중단 응답의 실제 writer 저장/재개방과 취소·clear·세대/epoch 경계를 확인한다. |
| R07 Chat 캐시 토큰 | `norma/llm/openai.go`의 `prompt_tokens_details.cached_tokens`가 공통 사용량 변환에서 누락됐다. | 공급자별 기존 키와 함께 Chat 키를 읽는다. JSON/SSE와 독립 질문의 실제 입력 17·출력 3·캐시 7 등 fixture 수치를 확인한다. |
| R08 보관 통계 작업 수 | `db/task_archives.go::taskArchiveAggregates`는 한 작업의 profile/skill 그룹을 논리 작업 하나로 계산한다. opaque registry 키·nullable FK가 다른 작업 또는 0개로 집계됐다. | 원장 키는 바꾸지 않고 보관 그룹의 작업 수를 1로 계산한다. aggregate 표식 version 2를 사용하며 이전 version 1/preview는 검증한 Blob에서 읽기 전용으로 재산출한다. 캐시·조회 저장 불변·복원 후 원장 의미를 확인한다. |
| R09 삭제된 회사 선택 | 원본 작업 생성 UI는 선택한 ID를 현재 회사 목록과 독립적으로 표시·해제한다. 목록에서 사라진 회사가 초안에 남아 생성이 막혔다. | 누락된 회사 ID와 명시적 선택 해제 버튼을 표시한다. 실제 egui 입력으로 초안·다른 선택 보존, 사용자 해제 후 생성 명령을 확인한다. |
| P3 마무리 사용자 도구 | 실행 정책은 외부 도구를 거부하지만 사용자 정의 도구는 마무리 요청에 노출됐다. | 기본 도구와 같은 마무리 허용 규칙을 사용자 도구 제안에도 적용한다. 일반 실행의 노출은 유지하고 마무리의 제안·승인 거부가 일치하는지 확인한다. |

API 요청 정규화는 저장된 도구 호출을 삭제하거나 종료·정리 성공 결과를 만들지 않는다. 마무리 시작 시 실행 중인 host invocation은 실제 응답·종료 증명이 올 때까지 그 불확실성을 유지한다. 원본과 동일하게 출력 토큰 한도의 부분 응답은 한 번의 모델 응답으로 보존하며, 새로운 자동 복구 정책이나 무조건 성공 판정은 추가하지 않는다.

## 검증 결과

개별 확인은 domain `parity_statistics` 4/4, RPC archive 8/8, native `task_creation` 5/5, host 통합 13/13, runtime 전체 25/25, RPC `side_` 10/10, 실행 조정 `stability_parity_tests` 6/6을 통과했다. 이 수치는 전체 회귀와 중복되므로 합산하지 않는다. 원래 R05 독립 하네스도 최종 라이브러리로 다시 연결하여 실제 요청 3회→1회·사용량 13/5 보존, 중단 답변 보존, Chat cache 0→7을 확인했다.

교차 검토에서는 출력 없는 실패의 재시도 보존, terminal-only/짧은 delta 뒤의 실제 부분 답변 보존을 보완하고 재검증했다. R02 시험의 늦은 완료 대조는 기존 종료 정책과 일치하게 invocation Blob 저장과 terminal 부모 transcript 불변을 각각 확인한다. 이것을 늦은 결과가 자식 문맥에 자동 삽입된다는 의미로 해석하지 않는다.

수정 중 실패도 구분한다. 기존 host 라이브러리 시험의 첫 병렬 실행은 46/47이었고, Windows 자손 종료 시험이 생성 직후 빈 PID 파일을 읽어 실패했다. 해당 제품/시험 코드는 바꾸지 않았으며 정확한 단일 시험 재실행은 1/1이었다. 새 HTTP fixture의 첫 실행은 Windows 소켓의 nonblocking 상속 때문에 실패하여 fixture 소켓 모드만 명시한 뒤 재실행했다. archive 복원 회귀의 최초 실패는 복원 시 ledger ID가 재매핑되는 의미를 놓친 시험 가정이었고, 원래 확인 대상인 opaque 값·nullable FK 보존을 검증하도록 수정했다. 이 실패들은 최종 통과 횟수에 넣지 않는다.

최종 Windows 전체 검증은 `RUST_TEST_THREADS=1`에서 `desktop/native/scripts/verify.ps1 -FullRegression`을 실행한 결과다. 모든 Cargo 명령은 `--locked`를 사용했다.

| 최종 검사 | 결과 |
| --- | --- |
| 네이티브 서비스 | 24 passed / 0 failed |
| 호스트, no-default-features | 71 passed / 0 failed |
| 네이티브 UI | 118 passed / 0 failed |
| 공통 workspace 회귀 | 383 passed / 0 failed |
| 빌드·feature 검사·격리 통합 서비스/LLM 시험 | 통과 |
| 실제 Windows 창 | 23개 화면·아이콘·정상 종료 통과 |
| 두 번째 실행 | 같은 primary의 GUI가 경로·요청 ID·read-only 플래그를 수신한 것까지 통과 |

최종 회귀에는 실패가 없고 Cargo warning도 없다. 두 번째 실행의 실제 모드 전환 확인 대화상자에 사용자가 동의하는 단계까지 자동 검증한 것은 아니다. 화면 검사는 격리된 앱 창이며 운영 작업 공간이나 실제 외부 공급자 수락 시험이 아니다.

- 전체 기록: `desktop/native/test-results/41cd2a51b4b149f699159d7bf2a56637/verification-result.json`
- 기록 SHA-256: `e28eb8c42912338efa2ecace0398e098cf1a021433c09477628ebb065f2766e4`
- 검증 소스: 209개 파일 해시를 최종 파일과 대조했다. Python MCP fixture와 새 문서 등 전체 작업 파일 해시는 저장소 밖 최종 `result.json`에도 별도로 기록했다.
- 검증 실행 파일: `desktop/target/debug/artex-native.exe`, SHA-256 `c607181c842be0bdba1b84ffe044723dd333084f1a8420a67570df07052eedd3`
- 원본 287개 파일 해시와 고정 계약 검증을 통과했다. 54 tables / 571 fields, 261 routes, 258 UI operations, 81 tools 및 1,221개 스키마가 유지된다. 계약·의존성·Cargo 잠금 파일은 변경하지 않았다.
- 기존 작업 규칙·세 언어 README·F01~F15 고정 보고서의 바이트를 보존했다. 이번 수정 범위 외의 시작 작업 파일도 해시로 대조했다. 최종 범위·해시 대조 기록은 `.organization/revalidation-fixes-20261017/result.json`이다.

호스트의 최초 부분 검사 기록은 저장소 밖 `.organization/host-revalidation-fixes-20261017-5d67ddd9bd904597b2ffa228a1ddc1c0/verification-result.json`에 남겼다. LLM 독립 재현의 새 결과는 `.organization/revalidation-20261017/llm/fixed-result.json`이며 SHA-256은 `ef4032bf63b0ceed2656d82b5eb9c446c8af804aa94c221b07cbf9308c61a55f`다. 이전 read-only 검증 산출물을 덮어쓰지 않았다.

## 범위와 남은 제약

이 기록은 재현한 결함의 수정·회귀 검증이다. 원본 전체 기능의 완전한 동등성 또는 안정판 자격을 뜻하지 않는다. Windows에서 실행한 결과로 macOS/Arch Linux의 컴파일·실기기 수락 시험을 대체하지 않는다. 실제 외부 LLM·MCP 서비스와 사용자 운영 환경은 이번 격리 시험 대상이 아니다. 이전 [F01~F15 수정 기록](stability-fixes-20261017.md)은 당시 고정 근거로 보존한다.
