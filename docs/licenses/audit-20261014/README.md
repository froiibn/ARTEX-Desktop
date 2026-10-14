# 라이선스 감사 자료

2026-10-14의 [공개 전 감사](../../license-audit-20261014.md)에 사용한 목록·해시·출처다. 개발 PC의 cache·작업 공간 절대 경로를 제거하고 패키지 내 상대 경로와 공식 source URL을 보존했다. 이 목록은 배포 조건의 법적 최종 승인이나 원문 전체를 재배포하는 고지 패키지를 대신하지 않는다.

이 자료는 코드 정리 전 commit `7d09ee2a0bd8a24ed50d1ceba3570afd9433f443`에 기록한 당시 증거다. 현재 의존성 수는 [후속 정리 기록](../../unused-code-cleanup.md)을 따른다.

| 파일 | 범위와 확인 수준 |
| --- | --- |
| `rust-native-packages.json` | active native 잠금 전체, 선언·고지 해시·확인 범위·대상별 포함 후보 |
| `rust-native-targets.json` | Windows/macOS/Linux normal/build 그래프. dev 제외, build 도구 포함 |
| `rust-native-notice-validation.json` | 제3자 651개 원본 archive 체크섬과 원문 고지 1,090개 대조 |
| `rust-all-locks.json`, `rust-all-locks-summary.json` | 저장소의 4개 Rust 잠금 파일. 합집합 제3자 839개, Tauri 소스 자료 구분 |
| `rust-additional-official-versions.json` | 미캐시 정확 버전의 공식 registry·archive·고지 출처 |
| `rust-mpl-sources.json` | 5개 MPL crate의 정확한 소스 취득 URL·Cargo.lock 체크섬·수정 없음 대조 |
| `npm-rebuilt-web.json` | 보존된 React 웹 533개: 원문 확보 448개, 공식 선언만 85개 |
| `npm-original-web.json` | 원본 웹 772개 항목: 원문 확보 739개, 공식 선언만 33개 |
| `npm-original-api-recon.json` | 원본 스킬 85개: 정확한 원문 85개. README에 포함된 전문도 구분 |
| `npm-evidence-counts.json` | 위 npm 확인 수준별 최종 집계 |
| `go-original-modules.json` | 원본 Go 요구사항과 체크섬 전용 이력 합집합 75개. 실제 import 도달 그래프와 구분 |
| `norma-content-comparison.json` | proxy v0.4.3·원본 baseline·라이선스만 추가한 후속 commit의 동일 코드 증거 |
| `tracked-assets.json`, `country-flags-provenance.json` | 추적 이미지·글꼴·아이콘 해시와 원본 관계, 국기 SVG 출처 비교 |
| `noto-font-metadata.json` | 원본·static weight-400 이름·저작권·라이선스·RFN 관련 metadata |
| `restored-notices.json` | 이번에 보존한 원문 고지 및 정확한 PSL source·기존 테이블 대조 |

`declared_license`나 `lock_license`는 패키지 작성자의 선언이다. `notice_files`·`license_files`의 해시는 원문 존재·출처 증거이며, 이름만 LICENSE인 파일이 그 패키지의 모든 중첩 구성 요소를 포괄한다는 뜻은 아니다. 원문 미확인 목록과 자동 점검의 한계는 해당 JSON 및 감사 보고서에 남겼다.

외부 npm/Go 자료는 원본 비교 조사에도 포함되며 현재 Rust 네이티브 바이너리에 전부 들어가는 것은 아니다. source-only 아카이브와 실제 바이너리·글꼴·runtime 배포 조건을 구분한다.
