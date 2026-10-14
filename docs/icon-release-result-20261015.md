# 앱 아이콘 적용과 Windows preview.3 배포 기록

2026-10-15 승인한 A·방패 아이콘을 앱과 Windows 실행 파일에 적용하고 [v0.2.0-preview.3](https://github.com/froiibn/ARTEX-Desktop/releases/tag/v0.2.0-preview.3)을 공개 시험 버전으로 배포했다. 릴리스 ID는 `413056775`다.

## 적용 범위

- 통합 제목 표시줄의 24px 아이콘, 앱 창·작업 표시줄용 256px PNG, 시스템 트레이용 32px PNG를 동일한 승인 이미지로 구성했다.
- 실행 파일에 16·24·32·48·64·128·256px RGBA 아이콘과 `0.2.0-preview.3` 버전 정보를 내장했다. 실행 시 별도 이미지 파일을 읽지 않는다.
- 승인 원본 PNG, 생성 프롬프트, Rust 변환 예제와 파생 ICO·PNG를 소스에 보존했다. 새 Rust 의존성을 추가하지 않았다. [아이콘 출처와 변환 방법](../desktop/native/assets/branding/README.md)을 따른다.
- Windows GNU는 기존 MinGW `windres`, Windows MSVC 빌드는 Windows SDK `rc.exe`를 이용한다. 다른 OS에서 Windows 리소스 컴파일러를 요구하지 않는다. 이번 배포·실행 검증 대상은 Windows x64 GNU이며 macOS 번들·Linux 런처 아이콘 패키징은 별도 범위다.

## 실제 검증

최종 배포 소스 commit은 `cc588305c0be99ade06bd5fd97091a74db801cd3`이며 태그·대응 소스 ZIP·실행 파일 빌드가 이 commit을 기준으로 일치한다.

- 서비스 24개, host 56개, UI 104개, 공통 workspace 회귀 332개 시험이 모두 통과했다. 일부 suite가 겹치는 집계이므로 고유 시험 516개라는 의미는 아니다.
- 통합 self-test, 23개 화면 영역 렌더, 실제 두 번째 실행의 전달, 정상 종료, 기존 WebView·엔진 자식 프로세스 미사용 검증을 통과했다.
- 디버그 및 실제 배포용 EXE의 7개 RT_ICON 프레임이 승인한 ICO의 PNG 바이트와 정확히 일치했다. Shell large/small 추출과 실제 앱 창의 WM_GETICON bitmap·투명도·색상 검증을 통과했다.
- Windows/eframe이 입력 256px를 OS/DPI 크기에 맞게 조절하므로 창 HICON 크기는 실제 관찰값으로 검사한다. 0×0 client 영역인 winit 이벤트용 창은 제외 사유를 기록하고, 실제 drawable 창의 아이콘은 필수로 검사한다.
- 최적화 배포 EXE 자체의 self-test, 읽기 전용 작업 공간 실행, About 라이선스 창·버전 표시, 정상 종료를 확인했다.
- 실제 linker 입력의 아이콘 리소스 객체·EXE 리소스·ICO를 대조하고 컴파일러 바이트를 기존 검증 MinGW 배포와 비교했다. 배포 고지와 원작자 표기는 유지한다.
- 소스 ZIP의 Git blob 311개와 배포 ZIP의 파일 888개를 바이트·SHA256 기준으로 검증했다. 미검토 개인정보·자격 증명 패턴 탐지는 0건이며 기존에 검토한 원본 라이브러리 공개 예제 4건의 정확한 해시 예외만 유지했다.
- GitHub 업로드 후 인증 없이 파일 3개를 다시 다운로드해 크기·SHA256 일치를 확인했다. 원래 `preview.2` 태그, 릴리스 ID, 파일 ID·크기·해시는 그대로 보존됐다.

이 기록은 로컬 Windows 검증 결과다. 이전 macOS·Arch CI를 이 아이콘 변경 commit의 재검증으로 간주하지 않는다. 코드 서명·설치 프로그램·자동 업데이트는 이번 배포에 포함하지 않는다.

## 고정 산출물

실행 파일 SHA256: `7f9bded2323dbac40b8314ef8f7237237df909b18f43817fe3033b94c582fd0a`

| 파일 | SHA256 |
| --- | --- |
| `ARTEX-Desktop-v0.2.0-preview.3-windows-x64.zip` | `fb94aa84086ae54c507fdc187eaa1c50a530de488dbb5e7979f8c11b7de7e31f` |
| `ARTEX-Desktop-v0.2.0-preview.3-source.zip` | `f03c950dae3a6f0451582946c6c0c7c5bcf9bd4e5516eecf60976fee15b627ea` |
| `SHA256SUMS.txt` | `eeabdddfcc8b1049e0ba070bd3346b0ce9e9848e8079ca1955859b4df5708b8e` |

이 보고서는 배포 이후 작성한 문서다. 고정 태그·실행 파일·대응 소스 ZIP을 수정하지 않는다. 현재 사용자 작업 공간·LLM 키·원본 Go/Web 소스는 변경하지 않았다.
