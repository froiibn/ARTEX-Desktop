# ARTEX Desktop v0.2.0-preview.5

2026-11-24 · Windows x64 GNU 시험 배포 / Preview / 预览版

## 한국어

`preview.4` 이후의 SQLite 전환과 누적 개선을 묶은 시험 버전이다. Rust 통합형 네이티브 앱을 유지하며, 별도의 DB 서버 없이 번들된 SQLite를 사용한다. 정상 v1 작업 공간은 쓰기 모드로 열 때 검증 후 v2로 이전하고 기존 파일과 원래 descriptor 사본을 보존한다. 이전 후 구버전 앱에서 v2를 열 수 없으므로 먼저 작업 공간 전체와 공통 앱 설정을 백업한다.

LLM·기본 모델·에이전트·도구/MCP·스킬의 공통 설정과 기존 모델 복구, 사이트별 종합 보고서의 조회·내보내기, 스트리밍·Windows 명령 안내·실행 종료 오류 표시, 대화·작업 목록과 완료 상태 표시의 누적 수정을 포함한다. 작업 공간별 Bypass 범위는 유지한다. 외부 도구와 API 키·사용자 작업 데이터는 배포물에 포함하지 않는다.

전체 기능 동등성 수락 검증은 339개 경로 중 11개 완료, 328개 미완료다. 따라서 안정판으로 승격하지 않는다. 이번 배포 범위는 Windows x64이며, macOS·Arch Linux 실기기 수락을 의미하지 않는다. 코드 서명·설치 프로그램·자동 업데이트는 포함하지 않는다.

릴리스 준비는 `verify.ps1 -FullRegression` → 표준 `build.py` → 커밋된 소스의 `package.ps1` → `create-release.py` 순서다. 패키지 검사에는 실제 배포 실행 파일의 자체시험, 23개 화면 렌더링과 정상 종료, 실행 파일·실제 창 아이콘, 고지·런타임 입력·대응 소스·ZIP 무결성이 포함된다. 검증 실패 산출물은 게시하지 않는다. 실행 결과와 소스 commit·체크섬은 GitHub 릴리스 및 패키지 `BUILD-INFO.json`에 기록하며, 이 절차 설명 자체가 실행 성공을 뜻하지 않는다.

## English

This Windows x64 GNU preview combines the SQLite migration and accumulated changes since preview.4. The integrated Rust native application uses bundled SQLite without a database server. Healthy v1 workspaces migrate to v2 after validation on a writable open, preserving legacy files and the original descriptor. Back up the entire workspace and common application settings first; older applications cannot open v2 workspaces.

Changes include shared LLM/default-model, agent, tool/MCP and skill settings; recovery of previously saved models; site assessment report viewing/export; streaming, Windows shell guidance and termination-error handling; and conversation/task list and completion-state improvements. Bypass remains workspace scoped. External tools, API keys and user workspaces are excluded from distribution.

Only 11 of 339 full-path acceptance entries are complete; 328 remain. This is not a stable release or macOS/Arch Linux physical qualification. Code signing, an installer and automatic updates are not included. The release pipeline requires full native regression, the standard build, committed-source packaging, packaged executable/window/icon/notice checks, and exact corresponding-source and ZIP integrity checks. Results and source identity are recorded in the release and bundled BUILD-INFO.json; this procedure description alone is not a passing result.

## 简体中文

此 Windows x64 GNU 预览版包含 preview.4 之后的 SQLite 迁移和累计改进。保持 Rust 一体化原生应用结构，使用内置 SQLite，无需数据库服务器。正常的 v1 工作空间在以可写模式打开时经验证后迁移到 v2，并保留旧文件和原始描述文件副本。请先备份整个工作空间及通用应用设置；旧版应用无法打开 v2 工作空间。

改进包括 LLM/默认模型、代理、工具/MCP 和技能的通用设置及旧模型恢复，网站综合分析报告查看与导出，流式处理、Windows 命令说明、执行终止错误显示，以及对话/任务列表和完成状态显示。Bypass 仍按工作空间设置。分发文件不包含外部工具、API 密钥或用户工作空间。

339 项完整路径验收中仅完成 11 项，仍有 328 项未完成，因此不是稳定版，也不代表 macOS/Arch Linux 已通过实机验收。不包含代码签名、安装程序或自动更新。发布流程要求完整原生回归、标准构建、已提交源码打包，以及分发程序/窗口/图标/声明、对应源码和 ZIP 完整性检查。实际结果和源码标识记录于发布页面及 BUILD-INFO.json；流程说明本身不表示验证成功。
