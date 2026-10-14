# ARTEX Desktop 공개 전 라이선스 감사

2026-10-14 기준 **현재 저장소와 Windows 시험 릴리스를 그대로 공개하는 단계에는 도달하지 않았다.** 원본의 별도 사용 조건, 앱 내 법적 고지, 공개 바이너리의 소스·의존성·런타임 고지 보완이 남아 있다. 확인 가능한 저작권 고지 누락은 이번 점검에서 소스 트리에 복원했다.

이 보고서의 839개 집계와 보존된 웹/Tauri에 관한 판정은 미사용 코드 정리 전의 고정 기준이다. 후속 [코드 정리 기록](unused-code-cleanup.md)에 현재 트리의 변경과 의존성 수를 기록한다. 아래 과거 증거는 현재 트리의 재집계로 덮어쓰지 않는다.

검토 기준은 원본 ARTEX commit `a9d11cb852eadad00669e4f6a05ee59cbbd14860`과 재구축 commit `2a1f5d41c42a9f288052b70d14837e6ad647b853`, 비공개 Windows `v0.2.0-preview.1`이다. 이후 추가한 고지와 이 보고서는 기존 릴리스의 실행 파일·ZIP·태그를 변경하지 않는다. 저장소 공개 전환은 수행하지 않았다.

## 후속 보완: Brevduva 프로토콜 참고 출처

Brevduva는 앞선 감사의 별도 출처·라이선스 목록에 누락되어 있었다. 2026-10-14 후속 점검에서 [froiibn/brevduva의 고정 commit](https://github.com/froiibn/brevduva/tree/f6f7219143c615db02a1f927f0a5402512327dda)을 확인했다. LICENSE와 workspace manifest는 Apache-2.0을 명시하고, NOTICE는 `Copyright 2026 SEIZIA (Jaeyoung Ko)` 및 프로토콜 명세의 출처를 표시한다.

ARTEX에서는 프로토콜 구조를 참고한 자체 내부 통신을 구현했다. Brevduva 라이브러리·SDK·서버를 직접 포함한 의존성으로 집계하지 않으며, 과거 Cargo 감사 수치를 바꾸지 않는다. [Apache 원문](licenses/brevduva-Apache-2.0.txt)과 [실제 NOTICE](licenses/brevduva-NOTICE.txt)를 별도 자료로 보존하고 [출처·원문 해시](licenses/brevduva-provenance.json)를 기록했다. 이번에 확인한 commit을 과거 구현의 실제 입력 버전으로 단정하지 않으며, 명세·코드·문구 사용 범위의 상세 대조와 새 배포 패키지 반영은 별도로 확인한다.

## 전체 목록과 확인 범위

| 대상 | 목록 점검 | 원문과 출처 확인 |
| --- | --- | --- |
| 원본과 파생 코드 | ARTEX, Norma, 원본 웹 템플릿, UI 참고 출처·고지 | 원본 Git 이력·LICENSE·README·로그인 UI, Norma 동일 코드와 후속 라이선스 commit 대조 |
| 재구축 Rust | 4개 잠금 파일의 제3자 합집합 839개, 선언 누락 0 | active native의 제3자 651개 archive SHA-256과 Cargo.lock 일치, 원문 고지 1,090개 대조 불일치 0; 추가 미캐시 87개 버전은 공식 registry와 archive 확인 |
| native 대상별 후보 | Windows 400개, macOS x64/arm64 각각 418개, Arch/Linux GNU x64 483개 | normal/build 그래프, dev 제외. 빌드 스크립트·매크로 포함이므로 실제 실행 파일에 전부 포함된 수가 아님 |
| 원본 Go | 두 go.mod의 54개 요구 module/version 쌍과 체크섬 전용 이력을 합쳐 75개 | Norma는 별도 동일 코드 대조, 나머지 74개 공식 module archive와 go.sum 일치·고지 수집. Go 실행 도구가 PATH에 없어 import별 최종 선택 그래프를 주장하지 않음 |
| 보존된 재구축 웹 | npm 533개 항목, 선언 누락 0 | 정확한 원문 448개 확인; 나머지 85개는 공식 정확 버전의 선언·metadata 확인, 원문 증거 미확인 |
| 원본 웹 | npm 772개 항목, 766개 고유 name/version | 정확한 원문 739개 항목 확인; 33개는 공식 선언만 확인 |
| 원본 api-recon 스킬 | npm 85개 항목 | 85개 원문 확인. degenerator 5.0.1의 MIT 전문은 별도 LICENSE가 아닌 정확한 source README에 있음 |
| 글꼴·테이블·이미지 | Noto KR, egui 기본 글꼴, PSL, 추적 이미지·국기 SVG·앱 아이콘 | 글꼴 메타데이터·원문 고지, 고정 PSL source 및 테이블 bytes, 아이콘 재생성·이미지 해시·SVG 출처 대조 |

패키지별 선언, 고지 원문 해시, 정확한 소스 경로, 대상별 범위와 미확인 목록은 [기계 판독 감사 자료](licenses/audit-20261014/README.md)에 있다. 원문 존재·체크섬의 자동 검증과 특수 조건의 원문 검토를 구분하며, 모든 소스 파일의 권리 귀속과 예외를 수동으로 확정한 것은 아니다.

## 공개 전에 해결할 문제

| 문제 | 영향 범위 | 완료 조건 |
| --- | --- | --- |
| 원본 README의 온라인·실무 사용 금지와 AGPL 간 충돌 | 프로젝트 목적과 공개 안내 | 원저작권자의 명확한 조건 확인 또는 AGPL 7·10조에 따른 처리 근거 정리. 별도 약정의 효력을 임의로 무효라고 단정하지 않음 |
| native 정보 화면에 법적 고지 없음 | 공개 앱 | 원저작자·기여자, 수정 버전·날짜, 무보증, AGPL 재배포권, 라이선스 원문 및 해당 릴리스 소스에 접근하는 정보/라이선스 UI 제공 |
| 새로 복원한 고지가 기존 릴리스에 없음 | 공개 바이너리와 소스 ZIP | 새 소스 commit·태그에서 모든 고지를 포함해 다시 패키징. 기존 태그와 ZIP을 조용히 바꿔 같은 빌드라고 표시하지 않음 |
| selectors 고지 매핑과 MPL 라이브러리 소스 안내 부족 | 공개 바이너리 | selectors의 실제 MPL 헤더·본문·정확한 crate source 연결. MPL 5개 라이브러리의 버전·체크섬·소스 제공 경로를 패키지에 포함 |
| GNU/MinGW 정적 런타임 출처·고지 불완전 | Windows GNU 바이너리 | 실제 링크된 runtime/startup 객체의 출처와 버전·고지 확인, 해당 GCC runtime exception 및 MinGW 고지 포함 |
| 원본 로고의 상표·외부 권리 근거 및 일부 SVG 버전 추적 | 공개 소스의 원본 자산과 홍보 자료 | 정확한 자산 적용 범위·출처 확인, 원저자 공식 제품으로 오인되지 않는 표시. 확인이 어려운 자산은 출처가 명확한 자체 자산으로 교체하는 선택 검토 |
| 정확한 대응 소스와 재빌드 확인 | 공개 바이너리 | 공개할 최종 태그의 빌드 스크립트·잠금 파일·리소스·의존성 소스 안내를 포함하고 깨끗한 clone에서 해당 구성의 빌드 확인 |

소스만 공개하려는 경우에도 현재 저장소를 공개로 바꾸면 기존 Releases가 함께 접근 가능해진다는 점을 고려해야 한다. 바이너리를 제외할지, 보완한 새 릴리스만 공개할지 범위를 확정한다. 이 점검에서는 기존 릴리스를 삭제·숨김·교체하지 않았다.

## ARTEX의 AGPL과 사용 제한

루트와 `desktop/LICENSE`는 원본 AGPL v3 원문과 동일하다. 원본 Git 이력의 `9b82b02fe2afca15d25f2efb941aa867b0d8c22c`은 2026-09-18 AGPL 도입 commit이며 author name은 Autumn-27이다. 프로그램에 별도의 or-later 적용 고지는 찾지 못했으므로 현재 `AGPL-3.0-only`를 유지한다. LICENSE 말미 적용 예문의 or-later 문구는 프로그램 전체에 대한 개별 허락과 구분한다.

원본 README는 AGPL을 부여하면서 별도 약정으로 로컬 학습·연구만 허용하고 자기 소유·승인을 받은 온라인 시스템도 실제 테스트를 금지한다. 현재 앱이 지향하는 허가된 보안 검증과 직접 충돌한다. [AGPL 7·10조](https://spdx.org/licenses/AGPL-3.0-only.html)는 추가 제한을 규율하므로 이를 새 프로젝트의 라이선스 제한으로 무조건 옮기거나 아무 문제 없다고 제거하는 판단을 이 감사에서 확정하지 않는다.

AGPL 자체는 판매나 상업 사용을 일률적으로 금지하는 비상업 라이선스가 아니다. [4·5·6조](https://spdx.org/licenses/AGPL-3.0-only.html)에 따라 저작권·수정·무보증 고지와 해당 소스를 제공하는 조건을 준수해야 한다. Rust로 포팅하고 Git 이력을 분리한 것만으로 원본 프롬프트·계약·코드 발췌의 권리가 사라지지 않는다. 현재 전체 앱을 MIT나 폐쇄 소스 라이선스로 바꾸는 근거는 없다.

원본 로그인 UI에는 법적 고지가 있었지만 현재 native 정보 창에는 앱 설명·단축키만 있다. 따라서 [AGPL 5(d)](https://spdx.org/licenses/AGPL-3.0-only.html)의 원본 인터페이스가 고지를 표시하지 않는 경우에 대한 예외를 적용한다고 가정하지 않는다. 고지 UI는 원본의 별도 사용 금지를 강제하는 동의 UI와 구분한다.

## Norma와 원본 웹의 확인 및 보완

Norma v0.4.3 tag `320ae32989da97dd91469bb89164cbfe87d83d5d`에는 LICENSE가 없었다. 직계 후속 [52f17dd52d95b087265e5d5b306fe85c1ea05599](https://github.com/Autumn-27/Norma/commit/52f17dd52d95b087265e5d5b306fe85c1ea05599)는 LICENSE 한 개만 추가하며 `Copyright 2026 Autumn-27`, Apache-2.0을 명시한다. Go proxy archive, ARTEX baseline의 norma와 라이선스 추가본의 기존 파일 210개가 모두 Git blob SHA 일치하므로 같은 코드의 명시적 라이선스 근거를 확인했다.

[Apache-2.0 원문](licenses/norma-Apache-2.0.txt)을 복원하고 [별도 라이선스 자료](licenses/third-party-notices.md)에 포팅·SDK 계약·프롬프트의 출처와 수정 버전임을 표시했다. 해당 licensed commit에는 원본 NOTICE가 없었다. [Apache 2.0 4조](https://www.apache.org/licenses/LICENSE-2.0)에 따라 없던 원저자 NOTICE를 만들어 주장하지 않고 실제 고지와 관련 변경 표시를 보존한다.

원본 웹의 MIT `Copyright (c) 2024 Mohammed Arham Khan`은 상위 `arhamkhnz/next-shadcn-admin-dashboard` 고지와 일치한다. 복사된 React 코드에 빠졌던 원문을 `desktop/web/LICENSE`와 `docs/licenses/original-web-MIT.txt`에 복원했다. shadcn UI, country-flag-icons, AegisHook 참고 관계와 MIT 고지도 보존했다. 국기 CSS의 257개 SVG 중 238개 XML이 공식 country-flag-icons와 일치하며 19개는 도입 버전의 추가 추적이 필요하다.

## 실제 Rust 배포 조건

Rust 의존성의 MIT/Apache/BSD/ISC/Zlib/Boost 등의 선언에서 현재 AGPL과 즉시 양립 불가능하다고 확정한 항목은 발견하지 않았다. AND 조건의 복합 라이선스와 OR 선택권을 구분한다. ring, aws-lc-sys에는 여러 원문·중첩 고지가 있고 webpki-roots의 데이터는 CDLA-Permissive-2.0, ICU/Unicode 자료는 Unicode-3.0이다. 단순히 모든 패키지가 MIT라고 분류하지 않는다.

MPL 대상은 cssparser 0.35.0, cssparser-macros 0.6.1, dtoa-short 0.3.5, option-ext 0.2.0, selectors 0.31.0이다. 정확한 source archive의 63개 소스 파일과 cache는 변경·누락이 없었다. [MPL 3.1–3.4](https://www.mozilla.org/en-US/MPL/2.0/)에 맞춰 정확한 소스 취득과 원문·고지를 연결한다. 기존 ZIP에는 MPL 본문이 다른 패키지 폴더에 이미 있지만 selectors의 매핑은 무관한 malloc_size_of/servo_arc의 MIT·Apache 고지여서 수정이 필요하다.

Windows 빌드는 x86_64-pc-windows-gnu와 crt-static을 사용한다. Cargo 밖의 libgcc, MinGW, startup 객체는 Cargo 목록에 잡히지 않는다. 관측 DLL import에 GNU runtime DLL이 없다고 정적 코드가 없다는 뜻은 아니다. [GCC runtime exception](https://github.com/gcc-mirror/gcc/blob/master/COPYING.RUNTIME)은 적격 컴파일에서 결합 프로그램의 배포를 허용할 수 있으므로 GCC 사용 자체가 ARTEX를 다른 GPL로 바꿔야 한다는 주장은 하지 않는다. 정확한 링크 출처와 적용 고지 확인은 남아 있다.

## 글꼴과 Public Suffix List

Noto Sans KR의 원본 variable 및 static weight-400은 OFL 원문·출처를 보존한다. Reserved Font Name은 Source이며 font family/PostScript에 이를 사용하지 않아 이번 검사에서 RFN 강제 개명 문제는 발견하지 않았다. static font name 메타데이터가 Thin을 유지하는 기술적 표시는 개선 가능하지만 라이선스 차단 문제로 분류하지 않는다.

egui 기본 글꼴은 Ubuntu Light(Ubuntu Font License), Noto Emoji(OFL), Hack(MIT와 Bitstream Vera), emoji-icon-font(MIT)다. 기존 ZIP은 해당 본문을 보존한다. Google·Canonical 등의 font 내부 저작권을 읽기 쉬운 색인으로 함께 표시하는 보완을 남긴다. 글꼴 자체를 프로젝트 AGPL로 대체하지 않는다.

Go publicsuffix 코드·테이블에는 BSD 고지가 있고 기반 PSL 데이터에는 MPL-2.0이 있다. 고정 revision `d6c92f1bbb7433e5db7b8405c25d4035fb8ff376`의 데이터 원문·MPL 본문·출처를 `desktop/crates/domain/resources/psl`에 보존했다. 기존 text/nodes/children 세 파일은 Go v0.58.0 upstream과 byte 일치하며 변경하지 않았다. 공개 패키지에는 두 출처의 고지와 정확한 데이터 source 접근을 포함한다.

## 보존된 웹과 외부 도구

Tauri/React는 현재 Rust native 바이너리에 포함되지 않지만 공개 Git 소스에는 남아 있다. Tauri의 잠금 파일은 현 manifest와 --locked로 해석되지 않아 현행 빌드 그래프를 확정하지 않는다. 아카이브 자료임을 명시하고 향후 실제 배포한다면 잠금 파일과 GTK/WebKit 등 시스템 라이브러리를 별도 점검한다.

웹 그래프에는 sharp/libvips의 LGPL-3.0-or-later, lightningcss의 MPL-2.0, caniuse의 CC-BY-4.0, Geist의 OFL, Simple Icons의 CC0와 개별 상표 조건이 있다. libvips 묶음의 중첩 라이브러리 고지·소스·재링크 조건은 실제 웹 바이너리를 배포할 때 추가로 충족해야 한다. 원문 미확인 npm 항목을 목록에 남기며 선언 확인만으로 모든 중첩 배포 조건이 완료됐다고 표시하지 않는다. 이러한 웹 조건을 현재 native 실행 파일의 LGPL 의무로 자동 적용하지 않는다.

현재 ZIP에는 Nmap·Python·외부 MCP 프로그램이 없으며 승인된 요청 때 사용자의 설치 프로그램을 실행한다. [Nmap NPSL](https://nmap.org/npsl/) 등 개별 도구를 향후 함께 배포하면 별도 재배포 조건을 점검한다. ScopeSentry는 독립 원격 서비스와 API/MCP로 상호운용하며 제품 코드를 번들한 증거는 없다. 그 제품의 별도 AGPL·상업 조건을 ARTEX 전체에 자동 적용하지 않는다.

## 공개 전 실행 순서

1. 원본의 별도 사용 조건을 처리할 근거와 공개 목적을 확정한다.
2. 앱 정보·라이선스 UI와 의존성별 정확한 소스·고지 색인을 보완한다.
3. GNU 런타임과 공개 자산·보존 코드의 범위를 정리한다.
4. 최종 소스로 깨끗한 빌드를 검증하고 새 태그에서 바이너리·완전한 대응 소스·고지를 함께 패키징한다.
5. 기존 비공개 릴리스의 공개 포함 여부와 최종 공개 범위를 확인한 뒤 전환한다.

프로젝트의 목적과 구현 경계는 [프로젝트 목적](project-purpose.md)에 정리했다.
