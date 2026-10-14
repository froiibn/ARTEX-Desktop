# 네이티브 창 프레임과 macOS·Arch Linux 점검

2026-10-14 갱신: `desktop/native`와 `desktop/native-core` 및 공통 Rust crate를 점검했다. Windows 실기기 검증, macOS/Arch CI 빌드·시험, 대상별 의존성 해석, 향후 실기기 검증을 구분한다. CI 통과를 모든 데스크톱 환경의 수락시험 완료로 취급하지 않는다.

## 이번 수정

`native/src/window_chrome.rs`의 공통 Rust 제목줄이 기존 메뉴 행을 대체한다. 메뉴, 창 이동 영역, 창 조작 버튼은 겹치지 않는다. 제목줄은 40 논리 px로, 대화와 하단 패널에 별도 제목 행을 추가하지 않는다. 최소화, 최대화·복원, 제목줄 더블 클릭, 이동, 크기 조절을 제공한다. 닫기는 `NativeApp::native_window_close`를 통해 기존 백그라운드·트레이 설정 또는 저장 확인·서비스 종료 절차로 연결한다. 제목줄 코드에서 프로세스 종료나 직접적인 최종 Close를 실행하지 않는다.

| 대상 | 창 구성 | 크기 조절 | 단축키 |
| --- | --- | --- | --- |
| Windows | OS 장식을 끄고 앱 제목줄·우측 조작 버튼 | 네 변·모서리의 초기 누름을 표준 BeginResize로 전달 | Ctrl |
| macOS | 네이티브 제목줄을 투명하게 하고 전체 콘텐츠·앱 제목줄·좌측 조작 버튼을 사용 | 네이티브 Titled/Resizable 동작 유지, BeginResize 미사용 | Command, 대화 전송은 기존 Ctrl도 유지 |
| Linux X11/Wayland | OS 장식을 끄고 앱 제목줄·우측 조작 버튼 | 표준 BeginResize, 최대화·전체 화면에서는 핸들 비활성화 | Ctrl |

macOS에서 winit 0.30.13의 `drag_resize_window`는 지원되지 않는다. 따라서 모든 OS에서 같은 장식 제거와 크기 조절 호출을 쓰지 않는다. egui-winit 0.33.3은 `titlebar_shown(false)`를 제목줄 투명화로 매핑하며, macOS에서 decorations를 유지하는 이유다. 창 배경 전체의 이동은 끄고 명시적인 제목줄 영역만 이동한다. 구현과 API 근거는 [egui 0.33.3 공식 예제](https://github.com/emilk/egui/blob/0.33.3/examples/custom_window_frame/src/main.rs)와 잠긴 winit/egui-winit 소스다. macOS 네이티브 테두리·확대 동작은 실제 호스트에서 확인해야 한다.

대화의 Ctrl 전용 전송을 Ctrl 또는 egui Command로 수정했다. macOS의 버튼·힌트·메뉴는 ⌘로 표시한다. 실제 egui 입력 회귀는 Command-only 전송, 중복 요청 차단, 첨부·CAS·대화별 초안 및 늦은 응답 소유권을 확인한다. 이는 macOS OS 창 시험을 대체하지 않는다.

Unix의 비 UTF-8 PathBuf를 `json!`에 넣으면 직렬화 실패가 panic으로 이어질 수 있었다. `native-core/src/service.rs`의 StageFile과 SaveFile은 scope 검사 후 JSON 변환 전에 UTF-8 여부를 확인하고 INVALID_PARAMS를 반환한다. 지원하지 않는 경로를 대체 문자열로 바꾸거나 파일을 수정하지 않는다. 일반 한국어·공백 UTF-8 경로는 유지한다. 최근 작업 공간 목록의 직렬화 실패도 기존 파일을 빈 내용으로 덮어쓰지 않도록 변경했다. Unix 전용 서비스 회귀는 거부 뒤 같은 writer에서 정상 파일 작업이 가능한지 확인하며 Windows에서는 실행되지 않는다.

## 확인한 범위

현재 호스트의 Rust는 1.98.0이고 설치 target은 x86_64-pc-windows-gnu다. WSL Ubuntu는 접근 가능하지만 Rust/Cargo/pkg-config가 없으며 Apple SDK·링커도 없다. 도구 체인이나 시스템 패키지를 설치하지 않았다.

`cargo tree --locked --manifest-path desktop/native/Cargo.toml --target TARGET -e features`가 아래 세 대상에서 성공했다. 결과는 `desktop/.resume/window-frame-audit-20261013`에 기록했다. 이는 target별 의존성 해석이며 컴파일·링크·GUI 실행 성공을 뜻하지 않는다.

- aarch64-apple-darwin
- x86_64-apple-darwin
- x86_64-unknown-linux-gnu

Windows 전체 회귀·창 캡처·릴리스 실행 근거는 `desktop/native/STATUS.md`의 최신 항목 및 해당 verification/package 결과를 따른다. 공유 모듈을 수정하면 전체 공유 회귀를 다시 실행한다. 이후 UI/검증 절차만 수정한 경우에는 공유 소스 해시 157개를 대조해 재사용하고 native 서비스·host·UI·실제 실행은 다시 확인한다.

## 배포 전 남은 코드·운영 과제

1. **Unix 외부 프로세스 종료 보장:** `host/src/execution.rs`, `pty.rs`, `mcp.rs`는 원래 프로세스 그룹을 종료하지만 setsid/setpgid로 이탈한 자손을 완전하게 추적하지 않는다. 10월 14일 변경은 leader 회수·출력 완료·알려진 자원 정리와 모든 자손 종료 확인을 구분한다. 강제 종료와 I/O 대기에 기한을 두고 종료하지 않은 drain을 무기한 join하지 않으며, 회수된 leader의 PGID에는 다시 신호를 보내지 않는다. Unix의 완전한 자손 정리 증거는 미확인으로 표시해 파일·기록 삭제를 차단한다. 이탈 자손 Linux 전용 회귀를 추가했지만 Windows에서 실행한 것으로 주장하지 않는다. Linux cgroup/scope 및 macOS에 맞는 자손 추적과 실제 호스트 수락시험이 남아 있다. Windows Job Object와 동일한 보장을 Unix에서 주장하지 않는다.
2. **Linux 절전 시간 예산:** `host/src/power.rs`는 logind 지연 inhibitor를 취득하나 알림 예산을 4초로 고정한다. 실제 `InhibitDelayMaxUSec`가 더 짧게 설정된 환경에서는 보장이 부족할 수 있다. 해당 속성에 안전 여유를 적용하고 실제 절전·복구를 검증해야 한다.
3. **패키징:** 현재 verify.ps1/package.ps1은 Windows 실행 파일·프로세스·모듈 검사용이다. macOS 앱 번들/Info.plist/아이콘/서명·공증, Arch 패키지 또는 배포 디렉터리/.desktop/아이콘/배포용 런타임 고지 검증은 아직 없다. Arch CI에서는 Xvfb의 실제 창·스크린샷·정상 종료를 확인했지만 실기기 X11/Wayland 수락시험을 대신하지 않는다. 기존 Go/web release workflow는 네이티브 Rust 앱의 빌드 근거로 사용하지 않는다.
4. **OS 키 저장:** SecretRef는 각 OS의 자격 증명 저장소를 참조한다. 작업 공간 파일을 다른 OS로 옮겨도 API 키는 자동 이전되지 않는다. Linux Secret Service가 없거나 잠겨 있으면 현재 세션 메모리 fallback 및 재시작 후 재입력 동작을 확인해야 한다. 평문 파일 저장으로 대체하지 않는다.

macOS의 Mach clock, IOKit/CoreFoundation 절전 분기, UI 스레드의 AppKit 트레이, Unix 파일 잠금·rename·디렉터리 sync 구현은 존재한다. macOS CI(개발 commit `6db976b690dceaf07c0f2ecaf00a3bd5e5b9d1ea`)에서 서비스·저장·host·UI 시험, release 빌드와 통합 자체 시험을 확인했다. Arch CI(개발 commit `c0fb14431de813f0ef3c8e9e60cfbf21e744732f`)도 같은 시험·release 빌드·자체 시험 및 Xvfb 창 실행을 통과했다. 이후 추가한 Windows 검증용 종료 신호와 진단 절차의 최종 상태는 배포 결과 기록으로 구분한다. APFS의 대소문자·Unicode 차이, ext4/Btrfs, 충돌·절전·재실행은 실기기 수락시험 대상이다.

## 실제 빌드 준비

우선 검증에 사용한 Rust 1.98 계열과 lockfile을 유지한다. 각 OS에서 저장소 루트를 기준으로 다음 명령을 실행한다. GNU/Linux 명령을 Windows에서 실행한 것으로 대체하지 않는다.

```sh
cargo test --locked --manifest-path desktop/native-core/Cargo.toml
cargo test --locked --manifest-path desktop/native/Cargo.toml
cargo test --locked --manifest-path desktop/Cargo.toml --workspace
cargo build --release --locked --manifest-path desktop/native/Cargo.toml
```

macOS는 Xcode Command Line Tools와 Apple SDK·링커가 필요하다. Intel/Apple Silicon을 각각 검증한 후 필요하면 universal artifact를 묶는다. 일반 다운로드 배포는 앱 번들, Developer ID 서명·hardened runtime·entitlements, notarytool·stapler 및 실제 Gatekeeper 실행을 확인한다. [Apple 서명 안내](https://developer.apple.com/documentation/xcode/creating-distribution-signed-code-for-the-mac/), [Apple 공증 안내](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution).

Arch Linux는 base-devel, pkgconf와 dbus 개발 파일을 확인한다. 실제 잠긴 의존성은 GTK/libappindicator 트레이를 사용하지 않으며 KSNI, XDG Portal, Secret Service와 libdbus-sys를 사용한다. Arch의 `dbus` 패키지가 헤더와 `dbus-1.pc`를 제공한다. [base-devel](https://archlinux.org/packages/core/any/base-devel/), [pkgconf](https://archlinux.org/packages/core/x86_64/pkgconf/), [dbus 파일 목록](https://archlinux.org/packages/core/x86_64/dbus/files/).

GUI 실행은 X11/Wayland·libxkbcommon(X11에서는 libxkbcommon-x11 포함)·OpenGL/EGL 드라이버, 사용자 D-Bus 세션과 데스크톱에 맞는 XDG Portal backend가 필요하다. API 키의 지속 저장에는 org.freedesktop.secrets 서비스가 필요하다. KSNI host가 없으면 창을 숨기지 않고 기존 종료 절차를 사용한다. [Arch XDG Desktop Portal](https://wiki.archlinux.org/title/XDG_Desktop_Portal), [Arch GNOME Keyring](https://wiki.archlinux.org/title/GNOME/Keyring).

최종 수락시험은 실제 Intel/Apple Silicon과 Arch X11/Wayland에서 창 이동·크기 변경·확대/복원·DPI·한국어 입력·키보드·파일 선택·키 저장·트레이 없음/소실·절전/복구·두 번째 실행·종료·외부 도구/PTY 정리를 포함한다. 현재 Windows 검증과 의존성 해석만으로 이 항목을 완료 처리하지 않는다.

2026-10-14 CI 갱신: Arch X11의 실제 실행에서 `libxkbcommon-x11.so`가 필요함을 확인했다. 빌드 성공만으로 이 동적 실행 의존성이 설치되었다고 볼 수 없다. Arch에서는 [공식 libxkbcommon-x11 패키지](https://archlinux.org/packages/extra/x86_64/libxkbcommon-x11/)를 설치한다.

Windows hosted CI의 실제 창 실행에는 OpenGL 2.0+ 지원이 필요하다. 기본 환경이 이를 충족하지 못한 실패는 [고정 소프트웨어 렌더러 절차](windows-ci-graphics.md)로 검증한다. CI 소프트웨어 렌더러 결과와 사용자 PC의 실제 드라이버 검증은 구분한다.
