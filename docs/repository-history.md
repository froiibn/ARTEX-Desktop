# 저장소와 개발 이력

2026-10-15, 사용자가 승인한 범위에 따라 저장소를 분리했다. 두 저장소 모두 비공개다.

- [ARTEX-Desktop](https://github.com/froiibn/ARTEX-Desktop): 정리한 현재 소스와 이후 개발. 기본 브랜치는 main이며 초기 root commit은 `34ad9fffffdaee951b9e024155b17b4511196b34`이다. 과거 개발 commit·브랜치·`preview.1` 태그를 가져오지 않았다.
- [ARTEX-Desktop-History](https://github.com/froiibn/ARTEX-Desktop-History): 기존 개발 이력 보관. 기존 저장소 ID, `codex/rebuilt` 브랜치, `v0.2.0-preview.1` 태그·릴리스·asset ID를 그대로 보존했다. 이 자료는 일반 배포용으로 옮기지 않는다.

Windows `v0.2.0-preview.2` 실행 파일과 대응 소스 ZIP은 새 저장소의 같은 이름 태그에서 제공한다. 이 태그는 고정 배포 소스 `34ad9f…`를 가리킨다. CI 환경·시험 코드·문서의 후속 보완은 이 root commit 위에 별도 commit으로 반영한다.

이전 개발 소스 `6d1239a71e439623ad1291d3191c7daa0f4e7ccd`의 [세 플랫폼 CI](https://github.com/froiibn/ARTEX-Desktop-History/actions/runs/38527450589)는 모두 통과했다. 새 main에는 동일한 CI·시험 보완을 파일 단위로 옮겼다. 새 저장소의 [Windows](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38595311266/job/115960523214)·[Arch](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38595311266/job/115960522781)와 별도 [Intel Mac](https://github.com/froiibn/ARTEX-Desktop/actions/runs/38599721218) 검증은 통과했다. 같은 전체 실행의 Apple Silicon job은 실행 머신 배정 실패로 시험이 시작되지 않았으므로 전체 실행을 success로 기록하지 않는다. 정확한 commit과 검증 범위는 후속 기록에 있다.

저장소 공개 전환은 이번 승인에 포함하지 않았으며 수행하지 않았다. [검증과 배포 기록](public-release-remediation-result-20261014.md)과 [다운로드 안내](download-and-run.md)를 함께 참고한다.
