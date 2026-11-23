# 미사용 코드 정리

2026-10-14. 현재 Rust 네이티브 앱에서 사용하지 않는 이전 Tauri/React 구현과 별도 엔진 전송 코드를 작업 트리에서 제거했다. 원본 Go/Web 저장소와 사용자 작업 공간은 별도로 유지한다. 현재 앱의 저장·에이전트·도구·관리 기능을 축소하는 정리는 하지 않았다.

## 제거한 범위

- `desktop/app`, `desktop/web`: 이전 Tauri/React 앱, 원본 웹 사본, 웹 전용 어댑터·테스트·잠금 파일·원본 로고.
- `desktop/crates/engine`: 이전 사이드카 실행 파일.
- `desktop/crates/host`: EngineClient, stdio 전송·WebView 전용 처리, 이전 업데이트 채널과 작업 공간 요청 큐, 전용 예제·시험.
- 호스트의 미사용 PTY 보조 메서드 3개와 RPC의 미사용 stdio 프레임 읽기·쓰기 및 해당 전용 시험. 실제 도구의 프로세스 대기·종료와 내부 JSON RPC의 크기 제한은 유지한다.
- `desktop/scripts`의 이전 UI 생성·수리·사이드카·패키징 스크립트 15개, 이전 Tauri CI 1개.
- 이전 Tauri의 README·기능 비교·구현 상태 문서 4개. 과거 설계와 상세 계약은 기존 기준점·현재 네이티브 구현을 구분해 유지한다.
- 호출 없는 Forms 편집 메서드 3개와 생성되지 않는 ExportBody 이벤트·분기를 제거했다. 실제 토큰별 저장·오류 처리, 다시 불러오기 버튼, 본문/보고서 파일 저장 경로는 유지한다.

현재 HostBridge의 `execute_direct` 호출은 UI와 앱 서비스가 같은 프로세스에서 OS 기능을 실행하는 경계다. 이를 이전 EngineClient와 함께 제거하지 않았다. 스킬 승인 후 Read/Bash의 실제 작업 경로 검증도 direct 호출로 전환해 유지한다.

프롬프트·작업 삭제의 시험 전용 wrapper와 테마의 렌더링 시험용 helper·아이콘은 `#[cfg(test)]`로 격리했다. 실제 프롬프트 조립, 삭제 실패 정리, 공유 테마·아이콘 렌더러와 포커스·대비·도형 검증 시험은 유지한다.

## 유지한 범위

현재 `desktop/native`, `native-core`, 공통 Rust 라이브러리와 고정 계약을 유지한다. 외부 프로그램·PTY·MCP·트래픽·키·알림·절전·파일·중지·실행 감독 기능, 데이터 이전 CLI와 백업/복원에 사용하는 importer를 유지한다.

`desktop/scripts`에는 도구 실행 증거 검사와 현재 계약 overlay를 유지하는 3개 도구가 남는다. 원본 계약 비교용 상위 `scripts`, 프롬프트 대조와 현재 Rust CI도 유지한다. 앱 기본 실행에 Node/npm이 필요하지 않으며 이 도구는 개발 비교용이다.

원본 프롬프트·계약·포팅 코드의 출처, Norma Apache, MIT 및 글꼴·PSL 고지는 계속 보존한다. 코드가 제거된 자산의 과거 감사 기록도 삭제하지 않는다.

## 의존성 변화

| 집계 | 정리 전 | 정리 후 |
| --- | ---: | ---: |
| 저장소 Rust 잠금 파일 | 4개 | 3개 |
| 제3자 package/version/source 합집합 | 839 | 654 |
| active native 잠금 전체 패키지 | 663 | 662 |
| Windows normal/build 제3자 후보 | 400 | 399 |

654개는 전체 운영체제·빌드·시험 의존성을 포함한 저장소 범위다. Windows의 399개에도 빌드 스크립트와 매크로가 있어 전부 실행 파일에 포함된다는 뜻은 아니다. 이전 UI가 이미 native 실행 경로에서 제외되어 있었으므로 이번 정리를 실행 파일의 대규모 성능 개선이라고 표시하지 않는다.

남은 잠금 파일에는 새 package/version을 추가하거나 생존 패키지의 체크섬을 변경하지 않았다. [현재 native 목록](licenses/native-dependency-inventory.json)을 갱신했으며, [정리 전 감사](licenses/audit-20261014/README.md)의 원본 증거는 그대로 보존한다.

## 검증과 배포

원본 계약 비교는 54개 테이블·571개 필드·261개 경로·258개 UI 동작·81개 도구와 1,221개 스키마를 통과했다. 프롬프트 비교는 9개 기본 역할·6개 마무리·3개 판단 구성을 통과했고, 도구 증거 참조는 81개 대응과 49개 Rust 시험 함수 연결을 통과했다. 실제 실행 검증과 단순 참조 검사를 구분한다.

최종 코드의 전체 검증 `desktop/native/test-results/49dc6d7c1e0442f49df6ac662c698a6c/verification-result.json`은 통과했다. 서비스 24개·호스트 56개·UI 96개·공통 회귀 332개를 검사했으며 중복 범위가 있어 합산하지 않는다. 빌드·통합 서비스/LLM fixture·실제 23개 화면·정상 종료·두 번째 실행 요청의 GUI 수신도 통과했다. 확인한 빌드·시험 stderr에는 warning이 없고 검증 source 184개 해시가 현재 파일과 일치한다. macOS·Arch Linux의 실기기 검증은 이 Windows 검사로 대체하지 않는다.

기존 Windows GitHub 릴리스와 배포 파일은 이 정리에서 교체하지 않는다. 공개 전에 앱 고지·MPL 소스·GNU 런타임 고지를 보완한 새 릴리스로 패키징해야 한다. [공개 전 점검](public-release-review.md)을 따른다.

## 복구와 과거 기록

정리 전 소스와 이력은 commit `7d09ee2a0bd8a24ed50d1ceba3570afd9433f443`, 로컬 Git bundle·소스 ZIP에 보존했다. 이전 앱/웹의 로컬 산출물도 저장소 밖 `.organization`의 복구 백업으로 이동했다. 기존 사용자 작업 공간·키·배포 산출물을 재귀 삭제하지 않았다.

필요한 파일만 과거 commit에서 꺼내 별도로 검토할 수 있다. 현재 변경을 되돌리는 전체 reset이나 과거 저장 파일을 사용자 작업 공간에 덮어쓰는 방식은 사용하지 않는다. Git 이력을 다시 쓰지 않았으므로 작업 트리에서 제거한 과거 코드와 관련 라이선스의 공개 범위는 여전히 별도 검토 대상이다.

## 2026-10-20 빌드 산출물 정리

이번 사용자 요청은 위의 과거 코드 정리와 구분되는 **로컬 실행 파일·패키지·중복 캐시 제거 및 출력 위치 통일**이다. 현재 소스의 미커밋 변경은 유지했다. 최신 실행 파일은 `desktop/dist/ARTEX-Windows/ARTEX.exe`에 보존했으며 SHA-256은 `6dbf74ce19351de584e0f70654385f295f2a21ec63edee5015f3dfceeb00b395`다. 기존 사용자 작업 공간·앱 프로필·키·원본 소스·Git/소스 백업·원격 릴리스는 변경하지 않았다.

일반 빌드는 `python desktop/native/scripts/build.py` 하나로 통일했다. 실제 Cargo가 보고한 실행 파일만 표준 경로에 완성된 바이트로 교체하고, 출처 고지·LICENSE와 실제 실행 파일 해시 및 로컬 개발 빌드 구분을 함께 둔다. 실패한 컴파일이나 누락된 산출물을 오래된 캐시 복사로 성공 처리하지 않는다. 잠긴 실행 파일은 삭제하지 않고 실패한다. `.cargo/config.toml` 및 명시적 `--target-dir`로 중간 파일을 `desktop/target`에 모으므로 중첩 workspace에서도 별도의 native/target을 만들지 않는다. macOS/Linux의 같은 명령 출력은 각각 `desktop/dist/ARTEX-macOS/artex-native`, `desktop/dist/ARTEX-Linux/artex-native`다.

정식 패키지는 `desktop/dist/packages/<version>`, 릴리스 조립은 `desktop/dist/releases/v<version>`으로 고정하고 기존 버전을 덮어쓰지 않는다. 패키징의 전체 검증·커밋·고지·링커·실행·아이콘 조건은 유지했다. 일반 로컬 빌드가 정식 배포 검증을 통과했다는 뜻은 아니다. CI의 정상 빌드/실행 경로도 표준 스크립트와 dist 경로로 연결했으나 원격 CI나 macOS/Arch 빌드를 이번에 실행하지는 않았다.

삭제 전 모든 절대 경로가 `D:\Artex` 내부의 명시된 생성 폴더인지 확인하고, reparse point·작업 공간 marker·실행 중 프로그램과의 충돌을 검사했다. 옛 checkout의 생성 폴더가 Git 추적 소스가 아님도 확인했다. `builds`, `rebuilt/desktop/dist`·`target`, `public-candidate/desktop/dist`·`target`, 중복 `desktop/native/target`, 이전 앱 복구 보관본의 target 등 7개 생성 폴더와 개별 산출물 44개를 삭제했다. 총 98,457개 파일, 논리 파일 크기 합계 72,902,837,138바이트이며 실제 디스크 회수량 측정값은 아니다.

구버전 배포의 package/BUILD-INFO/감사 로그·이미지·링커 증거 등 123개 기록은 삭제 전에 바이트를 대조하여 저장소 밖 근거 폴더의 `retained-records`에 보존했다. 과거 보고서의 삭제된 실행 파일 경로는 현재 실행 위치로 안내하지 않는다. 실행 파일이 아닌 소스 ZIP·Git bundle·라이선스 원본 자료와 기존 시험 workspace는 유지했다. 현재 공통 Cargo 캐시는 빌드 중간 자료이며 별도 사용자 실행 위치가 아니다.

검증은 빌드 실패/오래된 캐시 거부/같은 경로 재빌드/잠긴 출력 보존의 실제 파일 시험 4개, 저장소 루트와 하위 폴더에서의 표준 빌드, 세 Cargo manifest의 동일 cache 경로, Python/PowerShell 구문, Git ignore·공백 검사를 포함한다. 표준 위치의 실행 파일 자체에서 비대화형 통합 self-test가 성공했고 기존 81개 도구·로컬 공급자·CAS·저장/재열기·정리·정상 종료 근거를 확인했다. 앱 Rust 소스나 의존성은 변경하지 않아 UI/도구 전체 회귀를 새로 실행하지 않았다. 사용자가 중단한 데스크톱 화면 자동 조작은 재개하지 않았다.

정리·검증 근거는 `.organization/current-build-output.txt`가 가리키는 저장소 밖 폴더의 `artifact-inventory-before.json`, `cleanup-plan-execute.json`, `cleanup-result.json`, `standard-self-test-result.json`, `result.json`에 기록한다. 실행 파일·생성 기록을 Git에 추가하거나 커밋·푸시·릴리스 업로드를 하지는 않았다.
