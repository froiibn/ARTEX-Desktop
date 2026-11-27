# ARTEX Desktop v0.2.0-preview.6

2026-11-27 · Windows x64 GNU Preview

## 한국어

기존 21개와 남아 있던 318개를 검증해 명명된 UI 명령 258개·도구 81개의 수락표를 **339/339, 잔여 0**으로 완료한 버전이다. 300개는 유지 기능의 E2E 통과, 39개는 합의된 네이티브 동작의 변경점 통과다. [최종 보고서](acceptance-final-20261127.md)와 [행별 공개 검증 요약](verification/acceptance-20261127.json)을 참고한다. 실제 입력·정책/CAS·실행·SQLite/파일 저장·재열기·결과 표시를 연결한 Windows 격리 사례에 한정한다.

트리거·nullable 폼·사용자 도구 생성, 삭제 ID 계약, 범위/노드/도구 표시, 첨부의 모델 전달과 Read 권한, 발견 사항의 자산 연결·후속 탐색 출처, 일시정지된 워커의 메시지와 이력, 자기 일시정지·스킬 실행 영수증, SDK 실행 소유권, MCP/스킬 삭제 후 권한 정리, 일괄 일시정지의 시간 집계 충돌, 보관 복구의 출처 ID 재매핑을 수정했다. 기존 SQLite 작업공간과 공통 설정·작업공간별 Bypass 정책은 유지한다.

ZIP 전체를 압축 해제하고 `ARTEX-Windows-x64/ARTEX.exe`를 실행한다. 업데이트 전 앱을 저장·종료하고 작업공간 전체와 공통 설정을 백업한다. [실행·업데이트 안내](download-and-run.md)를 참고한다. 외부 도구·API 키·사용자 작업공간·검증 원시 로그는 포함하지 않는다.

이번 배포는 Windows x64 프리뷰다. 실제 외부 계정/OS 키 저장소, 기본 다중 에이전트 전체 fan-out, 장시간/부하/절전·전원 장애, macOS·Arch 실기기·설치·서명과 별도 수동 HTTP API 2개는 출시 검증 범위로 남아 있다. 코드 서명·설치 프로그램·자동 업데이트는 제공하지 않는다. 기능 수락표 완료를 모든 플랫폼의 안정판 완성으로 해석하지 않는다.

소스 commit과 실행 파일·대응 소스 ZIP·고지·검증·파일 해시는 같은 릴리스의 `BUILD-INFO.json`과 `SHA256SUMS.txt`를 따른다. 기존 preview.5 태그와 파일은 보존한다.

## English

This preview completes the previously qualified 21 plus 318 remaining named-operation entries: **339/339, zero remaining**, covering 258 UI operations and 81 tools. There are 300 retained-behavior E2E passes and 39 explicitly changed native-behavior passes. See the [report](acceptance-final-20261127.md) and [public per-row evidence summary](verification/acceptance-20261127.json). Qualification is limited to isolated Windows cases joining actual input, policy/CAS admission, execution, SQLite/file storage, reopen and result display.

Fixes cover trigger ownership, nullable forms, custom tool creation, typed deletion results, scope/node/tool display, attachment delivery and Read grants, finding anchors and follow-up provenance, paused Worker messages/history, self-pause and Skill receipts, SDK execution ownership, MCP/Skill grant cleanup, batch-pause clock conflicts and archive provenance ID remapping. Existing SQLite workspaces, shared settings and workspace-scoped Bypass remain supported.

Extract the entire ZIP and run `ARTEX-Windows-x64/ARTEX.exe`. Save and close the application and back up the complete workspace and common settings before upgrading. See [download and upgrade guidance](download-and-run.md). External tools, API keys, user workspaces and raw audit logs are excluded.

This remains a Windows x64 preview. Real external accounts/OS vaults, default multi-agent fan-out, long-running/load/power tests, physical macOS/Arch, installation/signing and two separately tracked manual HTTP mappings remain release gates. Code signing, an installer and automatic updates are not included. Named-case acceptance does not certify all-platform stability. The release's `BUILD-INFO.json` and `SHA256SUMS.txt` identify the exact commit, executable, corresponding-source archive, notices and checksums. Existing preview.5 assets are preserved.

## 简体中文

本预览版完成原有21项及剩余318项命名操作验收：**339/339，剩余0项**，包括258个UI操作和81个工具。其中300项为保留行为的端到端通过，39项为已明确调整的原生行为通过。请参阅[最终报告](acceptance-final-20261127.md)和[逐项公开验证摘要](verification/acceptance-20261127.json)。范围限定为Windows隔离测试中真实输入、策略/CAS、执行、SQLite/文件保存、重新打开及结果显示的连通路径。

修复包括触发器归属、可空表单、自定义工具创建、删除结果ID契约、范围/节点/工具显示、附件传递与读取授权、发现项资产关联与后续探索来源、暂停Worker的消息与历史、自暂停/技能执行凭据、SDK执行所有权、MCP/技能删除后的权限清理、批量暂停的计时版本冲突，以及归档恢复的来源ID重映射。保留SQLite工作空间、通用设置和按工作空间设置的Bypass。

完整解压ZIP后运行`ARTEX-Windows-x64/ARTEX.exe`。更新前保存并关闭应用，备份整个工作空间及通用设置。参阅[运行与更新说明](download-and-run.md)。不包含外部工具、API密钥、用户工作空间或原始审计日志。

此版本仍是Windows x64预览版。真实外部账户/系统密钥库、默认多代理完整执行、长时间/负载/电源测试、macOS/Arch实机、安装/签名及另行跟踪的两个手动HTTP映射仍是发布验收项目。不提供代码签名、安装程序或自动更新。命名案例验收不代表所有平台均达到稳定版。准确的源码commit、程序、对应源码ZIP、声明和校验和以同一发布版本的`BUILD-INFO.json`和`SHA256SUMS.txt`为准，保留原preview.5文件。
