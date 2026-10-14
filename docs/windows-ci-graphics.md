# Windows CI의 OpenGL 검증 환경

Windows 재실행 `38512194823`의 서비스·저장·host·UI 시험, release 빌드와 통합 자체 시험은 통과했지만 실제 GUI는 종료 코드 1로 실패했다. 새 창 로그는 `egui_glow requires opengl 2.0+`였다. 자동 종료 경쟁 조건과 별개로 CI 그래픽 환경의 OpenGL 지원이 부족했다.

## 적용 방법

`setup-ci-opengl.ps1`이 고정된 Mesa Windows 패키지를 내려받아 archive와 x64 DLL 2개의 SHA256을 확인한다. 새 CI 임시 폴더에만 풀고, `verify.ps1 -SoftwareOpenGlDirectory ...`가 검증할 실행 파일을 새 시험 폴더에 동일한 바이트로 복사한다. 해당 실행 폴더에 두 DLL을 함께 배치해 프로세스별로 사용한다.

Mesa의 [LLVMpipe 문서](https://docs.mesa3d.org/drivers/llvmpipe.html)와 Windows 배포자의 [사용 안내](https://github.com/pal1000/mesa-dist-win#installation-and-usage)에 따른 application-local 배치다. 소프트웨어 렌더링 환경 변수는 검증 프로세스에서 설정하며 시스템 DLL, 레지스트리와 설치된 그래픽 드라이버는 변경하지 않는다.

| 입력 | 고정 값 |
| --- | --- |
| 배포 | `pal1000/mesa-dist-win`, `26.2.4`, MSVC x64 |
| archive SHA256 | `351fc8c8b695878ffb3eaa044b3ead08672a48b1a045e3c3e3975811df0f6695` |
| `opengl32.dll` SHA256 | `75b295fc80df0eaa8f1264c28663f6932c8b72d36513eb092556608858b7f5e0` |
| `libgallium_wgl.dll` SHA256 | `53e49adf87590d5b0cab8553b1e98084812040cc1fca8c487695b17b7af3702e` |

고정 버전의 [배포 archive](https://github.com/pal1000/mesa-dist-win/releases/tag/26.2.4)와 실제 파일을 대조했다. 파일이 바뀌면 설치와 검증을 실패 처리하며 자동으로 최신 버전으로 바꾸지 않는다.

## 통과 조건

Windows의 `verify.ps1 -FullRegression`은 native 서비스·host·UI 및 전체 공유 workspace 시험을 실행한다. 저장 모듈 시험도 전체 공유 suite에 포함된다. workflow의 별도 중복 시험 단계는 macOS에서 실행하며 Windows는 이 통합 검증을 이용한다. release 빌드는 별도로 유지한다.

GUI에서는 기존 23개 영역·캡처·두 번째 실행 전달·단일 profile 소유권·정상 종료·WebView 및 엔진 sidecar 확인을 유지한다. 소프트웨어 OpenGL을 요청하면 두 DLL이 실제로 격리된 실행 폴더의 정확한 경로에서 로드됐는지 관측하고, 관측한 파일의 SHA256을 다시 확인해야 통과한다. 시스템 OpenGL로의 조용한 대체나 GUI 생략은 성공으로 인정하지 않는다.

`software_opengl` 결과의 `llvmpipe`는 요청한 환경 설정과 검증된 라이브러리의 실제 로드에 근거한다. 별도의 `GL_RENDERER` 조회 또는 실제 GPU 드라이버 인증으로 해석하지 않는다. DLL은 CI 시험 fixture이고 현재 Windows 배포 ZIP에는 추가하지 않는다. 제품 동작과 고정 `preview.2` 실행 파일·소스 ZIP도 이 CI 수정으로 변경하지 않는다.

로컬에서는 동일한 고정 DLL을 실제 배포 실행 파일 옆의 새 시험 폴더에 배치해 창 23개 영역·캡처·두 번째 실행·정상 종료와 실제 DLL 경로/해시를 확인했다. 잘못된 archive, drive-relative 경로, 기존 목적 폴더도 거부한다. 원격 MSVC 및 각 OS의 최종 workflow 성공은 해당 실행 결과로 별도 기록한다.

## 비동기 실행과 화면 시험의 시간 의존성

원격 재검증 `38521823069`에서는 native 서비스·host·UI 시험이 통과했고, 전체 공유 회귀의 `approved_skill_resources_match_scoped_read_and_bash_paths`가 실패했다. Bash가 `running`을 반환했는데 시험은 즉시 `completed`를 요구했다. `timeout_ms=10000`은 전경 대기 시간이며, 실제 실행 수명은 별도 `execution_timeout_ms`다. 10초를 넘긴 구체적인 시작·실행 지연 원인은 로그로 확정하지 않는다.

시험은 기존 전경 대기를 유지하고, 동일 실행 ID와 동일 run/epoch 문맥으로 `TaskOutput`을 조회한다. 실행 수명을 60초로 제한하고 추가 조회 대기는 65초로 제한한다. 명시 대기 예산은 합계 최대 약 75초이며 동기 spawn/I/O 비용은 별도다. 최종 상태 `completed`, 종료 코드 0, 캡처 오류 없음, 실제 Skill 파일의 출력 내용을 모두 요구한다. 남은 `running` 상태는 성공으로 인정하지 않고 해당 실행의 중지를 요청한다.

모니터 화면의 배치·클릭 시험도 렌더 프레임이 1초 안에 끝난다는 가정에 의존했다. 배치 시험은 첫 전체 조회 하나를 발생시키고 정확한 pending 토큰·조회 종류를 확인한다. 클릭 시험은 실제 pending 조회를 준비하고 클릭 효과를 별도로 확인한다. 중복 조회는 실제 pending fence로 막으며, 기존 버튼 geometry·초안·snapshot 보존·중지 및 강제 종료 권한 검증은 유지한다. 이 시간 의존성은 소스에서 확인한 독립적인 시험 문제이며, 진단이 누락된 첫 UI 실패 `38519672512`의 원인으로 확인된 것은 아니다.

실패 로그는 stdout·stderr의 마지막 부분을 각각 출력한다. 컴파일 stderr 360줄 뒤에도 stdout의 실패 assertion과 실제 종료 코드가 보이는 통제된 실패 시험을 통과했다. 이 보완은 검증 스크립트 및 `cfg(test)` 코드에만 적용하며, 앱의 전경 대기·실행 수명·권한 정책은 변경하지 않는다.
