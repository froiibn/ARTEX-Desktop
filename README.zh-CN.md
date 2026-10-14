# ARTEX Rust 桌面版重构

[한국어](README.md) · [English](README.en.md) · **简体中文**

GitHub：[froiibn/ARTEX-Desktop](https://github.com/froiibn/ARTEX-Desktop)。项目目前处于**私有开发与审查阶段**。Windows `v0.2.0-preview.2` 预览版及其对应源代码已通过私有 Releases 提供，需要登录并拥有仓库访问权限。根据[处理方针](docs/license-policy.md)，不将原版 README 的单独使用限制采纳为本派生项目的附加许可证条件。仓库尚未转为公开。之前的开发历史及 `preview.1` 保存在私有 [ARTEX-Desktop-History](https://github.com/froiibn/ARTEX-Desktop-History) 中。原版 ARTEX 的作者是 **Autumn-27**。请参阅[原作者、来源与修改说明](ARTEX_NOTICE.zh-CN.md)。

当前开发目标是在单个 Rust 进程中运行的原生桌面应用。原生 UI、对象与文件存储、LLM 服务及工具执行服务都在同一应用内运行。基本界面的运行不需要 Go、Node/npm、WebView 或 PostgreSQL 服务器。Python、Nmap 等外部工具在使用相应功能时仍需单独准备。

本项目旨在通过本地工作空间管理 LLM 辅助安全测试中的任务、智能体、81 个已注册工具、资产、证据和报告。请参阅[项目目的](docs/project-purpose.zh-CN.md)。

## 以防御性安全为目的公开

本项目希望帮助没有安全专业背景的用户，通过模拟渗透测试分析自己开发或运营的服务中的漏洞，理解发现的问题，并通过修复和安全加固采取应对措施。通过降低漏洞发现、修复及修复后复测的门槛，让普通用户也能更安全地管理自己的服务。

项目计划公开，以支持对用户拥有或已获得明确测试授权的服务进行安全检查，提高其防御能力。目前仓库仍处于私有开发与审查阶段。

## 为什么重构为桌面应用

安装 Docker、配置容器、搭建 Web 服务器和 PostgreSQL 等准备工作，给非开发者和普通用户带来了使用门槛。重构的核心目的是减少这些准备工作，让**没有开发知识的用户也能像使用普通桌面程序一样使用 ARTEX**。

用户下载并解压可执行程序包，启动应用，配置工作空间和 LLM 模型后即可开始使用。保留原版智能体工作流程，同时将对话、工具执行、审批、停止执行及结果查看整合到一个应用中。

- **简化启动准备：** 基本应用运行不需要另行安装或管理 Docker、Web 服务器和 PostgreSQL。
- **本地工作管理：** 通过对象关系和文件保存对话、任务、资产和证据，在同一工作空间中关联智能体共享的信息。
- **整合执行控制：** 在桌面界面中管理用户电脑上的工具执行、审批、进度、停止执行和结果查看。
- **统一开发与分发结构：** UI 与应用服务在同一 Rust 进程内运行，以共用代码支持 Windows、macOS 和 Arch Linux。目前完成预览分发和实际 GUI 验证的平台是 Windows x64。

工作空间数据保存在本地。使用 LLM、远程 MCP 或 Web 请求时，会与配置的服务或目标通信。背景与架构对比请参阅[项目目的](docs/project-purpose.zh-CN.md)。

## 提示词的英文转换

不同 LLM 模型对指令的理解和工具调用方式存在差异，因此即使使用相同的提示词，也无法保证与原系统完全相同的行为。考虑到这些差异，内置提示词已转换为英文，同时保留原版的角色、上下文、工作规则和工具契约。实际行为需结合所选模型和配置进行确认。对话和报告默认使用韩语，用户希望时可使用英语。

## 目录

- `desktop/native`：当前 Rust UI，以及原生验证和打包脚本
- `desktop/native-core`：应用服务
- `desktop/crates`：存储、领域逻辑、运行时、宿主服务、工具和数据迁移等共用 Rust 库
- `desktop/contracts`：构建中包含的固定契约
- `docs`：设计、原版兼容契约、81 个工具的映射和平台检查
- `scripts`：用于比较原版 Go/Web 数据结构和行为的开发工具
- `desktop/dist`：分发产物
- `.github/workflows/native-rust.yml`：当前 Rust 实现的各操作系统手动验证工作流
- `desktop/scripts`、`desktop/tests`、`scripts`：工具、提示词及原版契约的比较验证和契约维护工具

此前的 Tauri/React 应用和独立引擎客户端已从当前工作树中移除。[未使用代码清理记录](docs/unused-code-cleanup.md)（韩语）记录了移除范围和恢复基准。原版 Go/Web 仓库单独保留。

## 下载构建版本

在 [Windows x64 预览版 v0.2.0-preview.2](https://github.com/froiibn/ARTEX-Desktop/releases/tag/v0.2.0-preview.2) 的 Assets 中下载 `ARTEX-Desktop-v0.2.0-preview.2-windows-x64.zip`。完整解压后运行 `ARTEX-Windows-x64/ARTEX.exe`。基本应用运行不需要源码构建工具。同一发布版本还提供对应的源码 ZIP 和 `SHA256SUMS.txt`。

需要使用有权访问私有仓库的 GitHub 账号登录。该版本为未签名的 Windows 预览版本，不包含安装程序或自动更新。目前尚未提供 macOS 和 Linux 可执行文件。请参阅[首次运行指南](docs/download-and-run.md)（韩语）。

## 从源码构建并运行

准备 Rust stable 和操作系统对应的原生构建工具。在 Windows 上，从克隆的仓库目录运行：

```powershell
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'desktop/target'
cargo build --locked --manifest-path desktop/native/Cargo.toml
& .\desktop\target\debug\artex-native.exe
```

开发电脑上的本地 Windows 分发文件为 `desktop/dist/ARTEX-Windows/ARTEX.exe`。该文件不纳入 Git；克隆后请使用上述命令构建。请参阅[配置与运行](desktop/native/README.md)、[验证状态](desktop/native/STATUS.md)和[平台限制](docs/desktop-platform-audit.md)（韩语）。macOS 和 Arch Linux 的实机构建及 GUI 验证仍需另行完成。

在仓库目录运行完整原生验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File desktop/native/scripts/verify.ps1 -FullRegression
```

## 原版源码与 Git

本目录是独立的 Git 仓库，源码基准上传至私有 GitHub 仓库 `froiibn/ARTEX-Desktop`。原版 Go/Web 仓库和本地化修改历史保存在 `../original`。可执行文件、真实或测试工作空间、API 密钥、生成的日志和测试报告不上传至 Git。仅在本地保存的 `desktop/dist` 可执行文件不包含在克隆结果中；新克隆请按上述 Cargo 命令构建。

常规 Cargo 构建不读取原版仓库。只有 `scripts/validate-desktop-contracts.mjs` 等比较和生成工具默认使用 `../original`。如需指定其他原版源码目录，可设置 `ARTEX_ORIGINAL_ROOT` 环境变量。契约生成工具会重写输出文件，因此只读验证时不要运行生成器。原版 Web 开发依赖仅用于基于 Node 的原版比较工具。

此前验证结果中的绝对路径作为历史证据保留。`.resume` 临时脚本也可能使用当时的路径；当前工作请使用 `desktop/native/scripts`。用户数据和 API 密钥与本源码目录分开管理。
