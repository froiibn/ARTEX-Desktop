# Brevduva 구조 참고 범위

2026-10-14. 이번 공개 후보의 구조 설명은 [froiibn/brevduva commit f6f7219143c615db02a1f927f0a5402512327dda](https://github.com/froiibn/brevduva/tree/f6f7219143c615db02a1f927f0a5402512327dda)의 공개 프로토콜 명세를 확인 기준으로 삼는다. 원문은 Apache-2.0이며 `Copyright 2026 SEIZIA (Jaeyoung Ko)` NOTICE를 함께 보존한다. [LICENSE](licenses/brevduva-Apache-2.0.txt), [NOTICE](licenses/brevduva-NOTICE.txt), [원문 해시](licenses/brevduva-provenance.json).

이 기준은 현재 구조 설명의 출처다. 이전 구현 당시 참고한 명세의 commit을 확인했다고 주장하지 않는다. ARTEX의 계약·코드는 자체 프로토콜 `artex-agent/1.0`이며, Brevduva 공개 패키지·수신기·서버를 설치·연결하거나 wire 호환을 구현한 구성은 아니다.

## 내부 계약에 반영한 구조

| 참고한 개념 | ARTEX 내부 대응 |
| --- | --- |
| 에이전트 식별과 실행 세션 구분 | 지속적인 agent_id와 실행별 run_id·engine_epoch 인증 문맥 |
| 채널별 협업·격리 | workspace/task 범위의 채널과 수신자·객체 참조 권한 검사 |
| 단방향 전달과 요청/응답 구분 | send/broadcast와 request/reply, correlation_id를 통한 원 요청 연결 |
| 진행 보고와 업무 완료 구분 | report의 진행 상태와 final/outcome 종결 판정을 구분 |
| 수신 확인과 처리 완료 구분 | receipt 및 observed/completed 전달 상태, request 답변 없이 업무 완료로 처리하지 않음 |
| 전달 소유권과 세대 확인 | 실행·epoch에 연결한 claim 소유권, 만료·중지·재시작에 대한 재검사 |
| 영속 전달과 중복 처리 방지 | 파일 저장의 agent_messages·agent_deliveries·conversation_requests, command/message ID와 종결 답변 검사 |

구체적인 필드, 완료 규칙, 저장 형식과 오류는 [ARTEX 상세 명세](desktop-implementation-spec.md#내부-메시지-프로토콜-artex-agent10) 및 `desktop/contracts/agent-message.v1.schema.json`, `desktop/crates/agent/src/lib.rs`가 정의한다. Brevduva의 transport, 인증 토큰, org 주소 체계, 서버 API, SDK, 원격 MCP와 동일하다고 설명하지 않는다.

## 현재 코드와 배포에서의 범위

- Cargo manifest·잠금 파일에 Brevduva crate나 SDK를 추가하지 않는다.
- Rust 내부 메시지 처리와 스키마는 ARTEX 소스에 포함하며 전체 프로젝트의 AGPL-3.0-only를 유지한다.
- 참고 출처의 Apache 원문·실제 NOTICE를 별도 배포 라이선스 자료와 대응 소스에 포함한다. 일반 UI·README에 의존성 목록을 나열하지 않는다.
- 외부 Brevduva 서버·수신기·계정·네트워크 연결은 앱의 기본 실행에 요구하지 않는다.
