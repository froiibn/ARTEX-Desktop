# 시험 빌드 다운로드와 실행

Windows x64의 `v0.2.0-preview.4` 시험 배포를 다운로드할 수 있다. 2026-10-15에 저장소를 공개로 전환했으며 로그인 없이 내려받을 수 있다.

1. [릴리스 페이지](https://github.com/froiibn/ARTEX-Desktop/releases/tag/v0.2.0-preview.4)에 접속한다.
2. Assets에서 `ARTEX-Desktop-v0.2.0-preview.4-windows-x64.zip`을 받는다. `Source code` ZIP은 직접 빌드할 때 사용한다.
3. ZIP 전체를 쓰기 가능한 폴더에 압축 해제한다.
4. `ARTEX-Windows-x64` 폴더의 `ARTEX.exe`를 실행한다. 설치 프로그램은 없다.
5. 앱에서 새 작업 공간을 만들거나 기존 작업 공간 폴더를 연다.
6. LLM을 사용하려면 모델 메뉴에서 API 키를 저장하고, 모델 설정을 저장한 뒤 목록에서 선택해 **기본 모델로 설정**한다.

기본 앱에는 Rust, Go, Python, Node/npm, PostgreSQL, WebView의 별도 설치가 필요하지 않다. Nmap·Python·셸·사용자 MCP 등의 외부 도구는 해당 기능을 실행할 때 필요한 프로그램을 별도로 설치해야 한다. 외부 도구와 API 키는 다운로드 파일에 포함하지 않는다.

이 실행 파일에는 코드 서명이 없다. Windows 보안 정책에 따라 실행 경고 또는 차단이 발생할 수 있다. macOS의 빌드·통합 자체시험과 Arch의 Xvfb 창 검증은 CI에서 통과했다. 해당 OS의 실기기 수락 검증과 다운로드 패키지는 별도 작업이다. 설치·자동 업데이트 기능도 포함하지 않는다.

## 무결성과 소스

같은 릴리스의 `SHA256SUMS.txt`로 다운로드한 ZIP을 확인한다.

```powershell
Get-FileHash .\ARTEX-Desktop-v0.2.0-preview.4-windows-x64.zip -Algorithm SHA256
```

패키지의 `BUILD-INFO.json`에 실행 파일 SHA-256과 이 실행 파일을 빌드한 정확한 소스 commit이 기록된다. 대응 소스 ZIP과 태그는 이 고정 commit을 가리킨다. main의 후속 CI·시험·문서 보완은 기존 배포 파일과 구분한다. 압축에는 라이선스 자료와 빌드 정보가 포함된다. 원작자·출처와 수정 사항은 [ARTEX 안내](../ARTEX_NOTICE.md)를 참고한다.

같은 릴리스의 `ARTEX-Desktop-v0.2.0-preview.4-source.zip` 및 Git 태그에 소스와 잠금 파일·빌드 스크립트가 있다. 빌드 방법은 저장소 README와 `desktop/native/README.md`를 따른다. 사용자 데이터는 작업 공간과 OS 설정·자격 증명 저장소에 별도로 보관되므로 업데이트 시 실행 파일 교체와 작업 공간 삭제를 혼동하지 않는다.
