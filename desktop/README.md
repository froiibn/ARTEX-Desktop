# ARTEX Rust 작업 공간

현재 앱은 [native](native/README.md)의 Rust 네이티브 구현이다. 상위 [프로젝트 안내](../README.md)를 기준으로 실행한다.

```powershell
# 저장소 루트에서 실행한다.
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'desktop/target'
cargo build --locked --manifest-path desktop/native/Cargo.toml
& .\desktop\target\debug\artex-native.exe
```

`native`는 UI, `native-core`는 같은 프로세스 안의 앱 서비스다. `crates`와 `contracts`는 공통 라이브러리 및 고정된 계약이다. 개발 PC의 로컬 실행 파일은 `dist/ARTEX-Windows/ARTEX.exe`이며 Git에는 포함하지 않는다. clone 후에는 위 명령으로 빌드한다.

이전 Tauri/React의 `app`, `web`, 별도 engine 클라이언트와 해당 빌드·테스트 스크립트는 현재 작업 트리에서 제거했다. 현재 검증은 `native/scripts/verify.ps1`, 패키징은 `native/scripts/package.ps1`을 사용한다. 도구 실행 증거·프롬프트·고정 계약의 비교 도구와 원본 데이터 이전 CLI는 유지한다. [정리 기록](../docs/unused-code-cleanup.md)에 복구 기준을 남긴다.

[네이티브 검증 현황](native/STATUS.md), [원본 대비 경계](native/PARITY.md), [설계안](../docs/desktop-native-architecture.md)을 참고한다. 일반 Rust 빌드는 별도의 `../../original` Go/Web 저장소를 요구하지 않는다. 원본 비교용 Node 도구는 상위 `scripts/source-layout.mjs`의 경로 설정을 사용한다.
