# 공개 전 수정 작업의 최종 산출물

2026-10-14 검증, 2026-10-15 저장소 분리 반영. [수정 내역](public-release-remediation-20261014.md)의 후속 결과다. 소스·실행 파일의 수정, 배포 조립과 세 플랫폼 CI 검증을 완료했다. 기존 이력은 비공개 `ARTEX-Desktop-History`에 보존했고 새 `ARTEX-Desktop`도 비공개다.

## 반영 결과

- AGPL 처리 방침, 원작자 고지와 수정 버전 표시, 무보증·전체 라이선스·빌드 소스 안내를 한국어·영어·중국어 문서와 앱 정보 창에 반영했다. 일반 README는 원작자 중심 안내를 유지하고 배포에 필요한 상세 고지는 별도 파일로 보존한다.
- selectors/MPL, 기본 글꼴, Norma, Brevduva 구조 참고, PSL 및 정적 GNU/MinGW/Rust 런타임의 원문·출처를 보완했다. 실제 링크 입력과 공식 고정 archive를 대조한다.
- 개발 PC 경로 치환, 정확한 소스 ZIP, 실행 파일/고지 SHA 검증, ZIP 재추출 검증을 배포 절차에 넣었다. 과거 `preview.1` 실행 파일·태그·릴리스는 수정하거나 삭제하지 않았다.
- macOS의 시스템 임시 경로 별칭 문제와 APFS/HTTP fixture 차이를 수정했다. 검증 창은 두 번째 실행 확인 완료 신호를 받은 뒤 캡처·종료한다. 일반 실행·일반 캡처는 이전 동작을 유지한다.

## 고정한 배포 후보

- 버전: `v0.2.0-preview.2`, 대상: Windows x64 GNU.
- 소스 commit: `34ad9fffffdaee951b9e024155b17b4511196b34`.
- 소스 tree: `0574278b0bbb8680740056555bc394e5b19a74bf`.
- 배포 기준: `D:/Artex/public-candidate`의 root commit 1개, 소스 297개 파일. 후속 CI·시험·문서 보완은 이 root 위에 별도 commit으로 반영하며 과거 개발 이력을 가져오지 않는다.
- 실행 파일: `D:/Artex/builds/v0.2.0-preview.2/ARTEX-Windows-x64/ARTEX.exe`.
- 실행 파일 SHA256: `1a82005fc01dee92ad1dae4474c72ad5072beab44806edd5b20b7e1d78e8291f`.
- 대응 소스 ZIP도 `D:/Artex/builds/v0.2.0-preview.2`에 함께 보관했다. 배포 기준 commit과 `v0.2.0-preview.2` 태그를 새 비공개 GitHub 저장소에 등록했다. 소스 접근에는 로그인과 저장소 권한이 필요하다.

| 파일 | 바이트 | SHA256 |
| --- | ---: | --- |
| `ARTEX-Desktop-v0.2.0-preview.2-windows-x64.zip` | 22,530,394 | `e80e608bc1f6f1dcde27e7af62b57a27abdf464c857b9342f3cc119430fae408` |
| `ARTEX-Desktop-v0.2.0-preview.2-source.zip` | 14,643,274 | `4cb2d9138e81e3eb8832c234a66a0ef61489dfcdde7f1c7f81ec482fbd9c32b4` |
| `SHA256SUMS.txt` | 221 | `6f86ca3afa7d0cfbb1c5a72a799701a9c22dc4952df83ff36099abc790d06f0a` |

배포 원본 3개는 `D:/Artex/public-candidate/desktop/dist/releases/v0.2.0-preview.2-final`에 있다. 소스 ZIP의 Git blob 297개가 정확히 일치하고, 실행 ZIP의 파일 878개는 재추출해 해시를 대조했다. `BUILD-INFO.json`과 `FILE-SHA256SUMS.txt`는 실제 실행 파일 및 개별 배포 파일을 식별한다.

## 검증 결과

- 전체 회귀 통과. 공유 모듈의 실제 전체 검증 `1471b0e8923646999959c4509bf23f16` 이후 공유 소스 157개 해시를 대조해 재사용했다. 최종 검증 `488dc5a0f874440fb932382b538cade4`에서 서비스 24개, host 45개 및 통합 9+2개, UI 102개, 빌드·통합 자체 시험·실제 GUI 23개 영역·두 번째 실행·정상 종료를 다시 실행했다.
- 배포한 실제 실행 파일의 자체 시험, 정보 창의 애니메이션 완료 후 캡처, 23개 영역과 정상 종료, WebView/엔진 sidecar 부재 관찰을 확인했다. 정보 창 이미지는 직접 확인했다.
- 배포 ZIP에서 추출한 실제 실행 파일로 두 번째 실행을 의도적으로 4초 늦췄다. 완료 신호 전에는 캡처·종료하지 않았고, 신호 후 요청 수신·read-only 전달·캡처·정상 종료를 확인했다. 실제 hold 시간: 4104ms.
- normal/build 후보 399개, 고지 파일 865개를 검증했다. 이 후보 수를 실제 바이너리에 모두 포함된 구성 요소 수라고 주장하지 않는다.
- 공식 원문·파일 해시·원문 source prefix 대조로 확인한 공개 예시 4건만 정확한 파일+SHA+탐지 범주로 허용한다. 공개 문서의 예제 RSA 키 1건과 경로 예시/JSON 오탐 3건이며 사용자 자격 증명이 아니다. 원문을 삭제하거나 디렉터리 전체를 제외하지 않았다. 변조 파일·다른 파일·소스 ZIP 예외 거부 등 12개 확인을 통과했다. 미검토 탐지 0건이며 이 검사는 모든 비밀정보 탐지를 보증하지 않는다.

| CI 대상 | 개발 commit | 결과 | run ID |
| --- | --- | --- | --- |
| macOS | `6d1239a71e439623ad1291d3191c7daa0f4e7ccd` | success | 38527450589 |
| Arch Linux (Xvfb) | `6d1239a71e439623ad1291d3191c7daa0f4e7ccd` | success | 38527450589 |
| Windows MSVC / software OpenGL | `6d1239a71e439623ad1291d3191c7daa0f4e7ccd` | success | 38527450589 |

macOS는 서비스·저장·host·UI 시험, release 빌드와 통합 자체 시험을 통과했다. Arch도 같은 범위와 Xvfb의 실제 창·스크린샷·정상 종료를 통과했다. Windows는 전체 공유 회귀를 포함한 시험과 debug 실행 파일의 통합 자체 시험·실제 창 검증 및 release 빌드를 통과했다. 이 결과는 같은 개발 commit의 후속 CI이며 고정 배포 후보 `34ad9f…`에서 원격 CI가 실행됐다고 주장하지 않는다.

### 새 저장소의 후속 CI — 2026-10-15 확인

| 대상 | 새 소스 commit | 결과 | 실행 기록 |
| --- | --- | --- | --- |
| Windows MSVC / software OpenGL | `ce1f75e8a5a170e0371a9f32de8b1243f772afeb` | success | [Windows job](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38595311266/job/115960523214) |
| Arch Linux (Xvfb) | `ce1f75e8a5a170e0371a9f32de8b1243f772afeb` | success | [Arch job](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38595311266/job/115960522781) |
| macOS Intel (`macos-15-intel`) | `84098a0bcaf756b31382f27ce36434ca7c1de476` | success | [Intel Mac run](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38599721218) |
| macOS Apple Silicon (`macos-latest`) | `ce1f75e8a5a170e0371a9f32de8b1243f772afeb` | 실행 머신 배정 실패; 시험 미실행 | [ARM job](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38595311266/job/115960523364) |

Windows는 전체 공유 회귀·서비스·host·UI 시험, 통합 자체 시험, 실제 창·두 번째 실행·정상 종료 및 release 빌드를 통과했다. Arch는 서비스·저장·host·UI 시험, release 빌드·통합 자체 시험과 Xvfb의 실제 창 검증을 통과했다. Intel Mac은 서비스·저장·host·UI 시험, release 빌드·실제 서비스/LLM fixture를 통과했다. Mac의 실기기 GUI 수락 시험을 완료했다는 의미는 아니다.

실행 `38595311266` 전체의 결론은 success가 아니다. Apple Silicon 머신을 배정받지 못해 해당 job은 단계 실행 없이 취소됐으며, Windows·Arch job은 success다. Intel Mac은 별도 실행 `38599721218`에서 success로 확인했다. 앞서 Apple Silicon은 `c892f180a7a07f143c7e800bf13f64c8c4d8c8c3`의 실행 `38592745002`에서 통과했지만, 이를 `ce1f75e…`의 ARM 재검증으로 바꾸어 기록하지 않는다.

새 저장소의 첫 Windows 실패 `38592745002`는 PowerMonitor 시작 전 초기 작업 공간 Create 시험의 10초 대기에서 발생했다. 두 lifecycle 시험의 초기 Create에만 기존 durable Create 시험과 같은 60초 예산을 적용하고, 일치하는 오류 응답을 즉시 표시하도록 고쳤다. 전원 이벤트·종료·재개 시험의 대기와 제품 동작은 유지했다. `84098a0…`은 `ce1f75e…`와 Rust 소스가 같고, 수동 CI의 Intel Mac 선택 항목 및 플랫폼 문서만 추가한 commit이다.

새 main의 추적 파일 301개에서 미검토 개인정보·자격 증명 탐지 0건을 확인했다. 고정 소스 ZIP은 root commit의 297개 Git blob과 정확히 일치한다. 두 저장소의 비공개 상태, History의 refs·릴리스·asset ID/크기/해시 보존, preview.2의 GitHub asset 해시와 로컬 원본 일치도 다시 확인했다.

이전 Windows 실행 `38512194823`에서는 CI의 OpenGL 환경 부족으로 창이 종료됐다. 고정·검증한 소프트웨어 OpenGL을 격리된 시험 폴더에 제공하고 실제 로드 경로·SHA256까지 확인하도록 보완했다. 후속 실행 `38521823069`에서는 Bash의 정상 비동기 `running` 반환을 시험이 즉시 `completed`로 단정한 별도 실패를 확인했다. 동일 실행의 최종 상태·종료 코드·실제 파일 출력을 확인하도록 시험을 수정했다. 모니터 배치·클릭 시험의 1초 렌더 가정도 실제 pending 조회를 이용해 제거했다. 진단이 없었던 첫 UI 실패의 원인은 확정하지 않는다. 구체적인 범위와 대기 예산은 [CI 보완 기록](windows-ci-graphics.md)에 있다.

후속 변경은 CI 검증 및 `cfg(test)` 코드와 문서에 한정된다. 제품의 폴링·실행 수명·권한 정책, 고정 배포 실행 파일·대응 소스 ZIP은 변경하지 않았다. Windows 소프트웨어 OpenGL 검증을 실제 GPU 드라이버 인증 또는 MSVC release 실행 파일의 창 검증으로 해석하지 않는다. 로컬에서는 이미 고정 GNU release 실행 파일 자체의 창·정상 종료 및 동일 소프트웨어 OpenGL 라이브러리 로드를 별도로 검증했다.

## 저장소 분리와 남은 조치

- 사용자의 승인에 따라 기존 저장소를 비공개 `ARTEX-Desktop-History`로 변경했다. 저장소 ID·브랜치·태그·릴리스 및 asset ID/크기를 대조해 보존을 확인했다. 강제 push나 과거 태그 삭제는 수행하지 않았다.
- 새 비공개 `ARTEX-Desktop`에 정리한 배포 기준 root commit과 `preview.2` 다운로드 릴리스를 등록했다. 기존 `preview.1`은 History에 원본 그대로 보존했다. 분리 방법은 [이력 보관 안내](repository-history.md)에 있다. 공개 전환은 별도 승인 전까지 수행하지 않는다.
- macOS/Arch 실기기의 DPI·입력·절전/복구·키 저장·외부 프로세스 정리와 해당 OS 배포 패키지는 별도 수락 범위다. [플랫폼 점검](desktop-platform-audit.md)의 잔여 사항을 유지한다. 코드 서명·설치 프로그램·자동 업데이트는 포함하지 않는다.

사용자의 실제 작업 공간·LLM 키·최근 작업 공간 설정과 원본 소스는 수정하지 않았다. 이 기록은 모든 저작권 관계의 법률적 보증이 아니다. 정확한 로컬 증거 색인은 `D:/Artex/.organization/public-remediation-final-result.json`에 보관한다. 이 후속 기록은 빌드 후 생성했으며 고정 소스 ZIP을 변경하지 않는다.
