# 출처 및 라이선스 고지

이 문서는 확인한 소스 출처와 고지를 보존한다. 실제 Windows `preview.2`에 포함하는 고지·원문·소스 안내는 패키지의 라이선스 색인과 [최종 산출물·검증 기록](../../docs/public-release-remediation-result-20261014.md)을 따른다. 2026-10-14의 이전 조사 결과는 [라이선스 감사](../../docs/license-audit-20261014.md), 현재 판정은 [공개 전 점검](../../docs/public-release-review.md)에서 구분한다.

## ARTEX와 원본 기반 코드·계약

이 프로젝트는 사용자가 `https://github.com/mhtsec/ARTEX.git`에서 받은 Go/Web ARTEX를 바탕으로 Rust 네이티브 앱을 재구축한 수정 버전이다. 비교 기준 Git commit은 `a9d11cb852eadad00669e4f6a05ee59cbbd14860`이다. 원본 Go 모듈에는 `github.com/Autumn-27/artex`와 `github.com/Autumn-27/norma`가 기록되어 있다. 원본과 그 기여자의 권리를 주장하지 않는다.

2026-10-14 기준 수정 내용에는 Rust 객체·파일 저장, 같은 프로세스의 네이티브 UI·실행 서비스, 실행 승인/중지, 대화·작업 관리와 한국어/영어 표시가 포함된다. 원본 프롬프트·계약·코드 발췌를 보존하며, 완전히 독립적으로 작성한 코드라고 표시하지 않는다. 이전 웹·Tauri 구현은 미사용 코드 정리로 현재 작업 트리에서 제거했고 로컬의 과거 Git 복구 백업에 보존했다.

현재 프로젝트의 LICENSE와 Rust manifest는 AGPL-3.0-only를 사용한다. [LICENSE](../../LICENSE)의 원문을 유지한다. 원본 README의 별도 사용 제한 문구는 [AGPL 7·10조에 따른 처리 방침](../../docs/license-policy.md)으로 다루며 파생 프로젝트의 추가 라이선스 조건으로 채택하지 않는다. 기존 고지를 임의로 제거하거나 전체 프로젝트의 라이선스를 바꾸지 않는다.

## Brevduva 프로토콜 구조 참고

ARTEX의 내부 통신 `artex-agent/1.0`은 [froiibn/brevduva](https://github.com/froiibn/brevduva)의 에이전트 식별·채널·요청/응답·보고·수신 확인 구조를 참고한 자체 Rust 구현이다. Brevduva 서버·클라이언트·SDK를 가져오거나 연결한 구성은 아니다.

2026-10-14 후속 점검에서 commit `f6f7219143c615db02a1f927f0a5402512327dda`의 LICENSE, NOTICE와 Cargo manifest를 확인했다. 명시된 라이선스는 **Apache-2.0**, NOTICE의 저작권자는 `Copyright 2026 SEIZIA (Jaeyoung Ko)`다. [라이선스 원문](brevduva-Apache-2.0.txt), [원본 NOTICE](brevduva-NOTICE.txt), [확인 기준과 원문 해시](brevduva-provenance.json)를 보존한다. 이 commit은 이번 라이선스 확인 기준이며 구현 당시 참고한 버전을 확정한 것은 아니다. ARTEX 전체 프로젝트의 AGPL을 변경하지 않는다.

## Norma SDK

Norma v0.4.3의 tag `320ae32989da97dd91469bb89164cbfe87d83d5d`에는 라이선스 파일이 없었으나 직계 후속 commit `52f17dd52d95b087265e5d5b306fe85c1ea05599`는 Apache-2.0 LICENSE만 추가했다. Go module proxy v0.4.3의 210개 파일, ARTEX 기준점의 `norma/`, 해당 후속 commit의 기존 파일은 모두 Git blob SHA가 일치한다. 같은 코드의 명시적으로 라이선스를 부여한 출처를 확인했다.

`Copyright 2026 Autumn-27`. [Apache-2.0 원문](../../docs/licenses/norma-Apache-2.0.txt)을 보존한다. Norma 기반 SDK 도구 계약·프롬프트·포팅에는 이 출처와 수정 버전이라는 고지가 적용된다. upstream의 원본 NOTICE는 해당 기준점에서 발견되지 않았다. 현재 전체 수정 작업은 AGPL을 유지하며 이 구성 요소의 원문 고지를 대체하지 않는다.

## 원본 웹 템플릿과 UI 자료

원본 `web/LICENSE`는 MIT, `Copyright (c) 2024 Mohammed Arham Khan`이며 `arhamkhnz/next-shadcn-admin-dashboard`의 MIT 고지와 일치한다. 이전 React 코드와 원본 고지는 로컬 과거 이력의 복구 기준 commit `7d09ee2a0bd8a24ed50d1ceba3570afd9433f443`에 보존되며, 현재 트리에는 [원문 사본](../../docs/licenses/original-web-MIT.txt)을 유지한다.

이전 shadcn UI 기반 구성 요소는 MIT, `Copyright (c) 2023 shadcn`이며 [고지 원문](../../docs/licenses/shadcn-ui-MIT.txt)을 보존한다. 제거한 웹 CSS의 국기 SVG는 `country-flag-icons`의 자료였으며 MIT, `Copyright (c) 2020 @catamphetamine` [고지](../../docs/licenses/country-flag-icons-MIT.txt)를 보존한다. 정리 전 최신 CSS와 257개 중 238개 SVG의 XML이 정확히 일치했다. 이 자산은 현재 트리에 포함되지 않으며 과거 자산을 다시 배포하면 정확한 도입 버전의 확인이 필요하다.

원본은 승인 상세 UI가 `RuoJi6/AegisHook`의 CallDetail 표현 구조를 참고했다고 명시한다. 직접 코드 복사를 확정하지 않고 참고 관계를 기록하며, MIT `Copyright (c) 2026 RuoJi6 and AegisHook contributors` [원문](../../docs/licenses/AegisHook-MIT.txt)도 보존한다.

## Public suffix 데이터 및 Go 기반 검증 자료

`desktop/crates/domain/resources/psl`에는 Go의 `golang.org/x/net/publicsuffix v0.58.0`을 이용한 테이블·알고리즘과 비교 자료가 있다. [Go Authors BSD 고지](../../desktop/crates/domain/resources/psl/LICENSE)를 보존한다. 기반 Public Suffix List 데이터에는 별도의 [MPL-2.0 원문](../../desktop/crates/domain/resources/psl/MPL-2.0.txt)이 적용되며, [고정 원본 데이터](../../desktop/crates/domain/resources/psl/public_suffix_list.dat)와 [출처](../../desktop/crates/domain/resources/psl/PROVENANCE.md)를 보존한다. 공개 바이너리에도 코드와 데이터 고지·정확한 소스 안내를 함께 제공한다.

## Noto Sans KR 글꼴

`desktop/native/assets/fonts`의 Noto Sans KR은 SIL Open Font License 1.1을 따른다. [OFL 원문](../../desktop/native/assets/fonts/OFL.txt)과 [출처·정적 weight-400 변형 기록](../../desktop/native/assets/fonts/PROVENANCE.md)을 보존한다. 글꼴은 별도 라이선스를 유지하며 프로젝트의 AGPL로 대체하지 않는다.

## Rust 의존성과 기본 UI 글꼴

[현재 Rust 의존성 메타데이터 목록](../../docs/licenses/native-dependency-inventory.json)은 잠긴 native Cargo graph의 선언을 기록한다. [정리 전 감사 목록](../../docs/licenses/audit-20261014/README.md)은 당시 4개 Rust 잠금 파일과 원본 Go/JS·재구축 JS·자산의 고정 증거다. 정리 후 현재 수와 범위는 [정리 기록](../../docs/unused-code-cleanup.md)을 따른다. 실제 빌드 대상·선택 기능·포함된 자산의 고지는 별도로 검토한다. eframe의 기본 글꼴에는 OFL, Ubuntu Font License 및 Hack의 MIT·Bitstream Vera 고지가 있으므로 단순 MIT/Apache 분류로 생략하지 않는다.

## 과거 웹·Tauri 기록과 외부 도구

이전 `desktop/app`, `desktop/web`와 해당 원본 로고·아이콘은 현재 작업 트리에서 제거했다. 과거 Git 기록까지 지우거나 권리·고지가 불필요해졌다고 주장하지 않는다. 해당 기록을 공개·재배포하는 범위는 정리 전 감사 결과를 함께 고려한다. Nmap·Python·사용자 MCP 등 별도 설치 프로그램의 실행과 해당 프로그램을 함께 재배포하는 경우도 구분해 검토한다.
