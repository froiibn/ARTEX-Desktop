# 저장소와 개발 이력

2026-10-15, 저장소 분리 후 사용자가 기존 이력을 GitHub에서 제거하도록 요청했다. 기존 `ARTEX-Desktop-History`를 삭제하고 GitHub에는 현재 `ARTEX-Desktop`만 보존했다. 현재 저장소는 2026-10-15에 공개로 전환했다.

- [ARTEX-Desktop](https://github.com/froiibn/ARTEX-Desktop): 정리한 현재 소스와 이후 개발. 기본 브랜치는 main이며 초기 root commit은 `34ad9fffffdaee951b9e024155b17b4511196b34`이다. 과거 개발 commit·브랜치·`preview.1` 태그를 가져오지 않았다.
- 기존 `ARTEX-Desktop-History`(ID `1417875247`): GitHub에서 삭제했으며 404 응답으로 확인했다. 삭제 전 브랜치·`v0.2.0-preview.1` 태그를 포함한 전체 Git bundle과 릴리스 파일 3개의 로컬 백업을 검증했다. 로컬 `../rebuilt`는 복구용으로 보존하고 삭제된 저장소의 remote는 제거했다. 이 백업은 현재 GitHub 저장소에 업로드하지 않는다.

Windows `v0.2.0-preview.2` 실행 파일과 대응 소스 ZIP은 새 저장소의 같은 이름 태그에서 제공한다. 이 태그는 고정 배포 소스 `34ad9f…`를 가리킨다. CI 환경·시험 코드·문서의 후속 보완은 이 root commit 위에 별도 commit으로 반영한다.

이전 개발 소스 `6d1239a71e439623ad1291d3191c7daa0f4e7ccd`의 세 플랫폼 CI `38527450589`는 모두 통과했다. 삭제된 저장소의 CI 링크는 더 이상 제공하지 않으며 당시 로그는 로컬 검증 기록에 보관한다. 새 main에는 동일한 CI·시험 보완을 파일 단위로 옮겼다. 새 저장소의 [Windows](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38595311266/job/115960523214)·[Arch](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38595311266/job/115960522781)와 별도 [Intel Mac](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38599721218) 검증은 통과했다. 같은 전체 실행의 Apple Silicon job은 실행 머신 배정 실패로 시험이 시작되지 않았으므로 전체 실행을 success로 기록하지 않는다. 정확한 commit과 검증 범위는 후속 기록에 있다.

현재 저장소의 ID `1419301831`, main, 고정 배포 태그·릴리스와 파일 해시는 기존 이력 삭제 직후 그대로 유지되는 것을 확인했다. 이후 main 변경은 이 삭제 사실을 반영한 문서 수정이다.

기존 이력 삭제 이후 사용자의 별도 공개 요청에 따라 2026-10-15에 현재 저장소(ID `1419301831`)를 공개했다. 공개 범위는 현재 소스와 고정된 `preview.2`이며 로컬 과거 이력·`preview.1` 백업은 포함하지 않는다. [검증과 배포 기록](public-release-remediation-result-20261014.md)과 [다운로드 안내](download-and-run.md)를 함께 참고한다.
