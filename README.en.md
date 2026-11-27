# ARTEX Rust Desktop Rebuild

[한국어](README.md) · **English** · [简体中文](README.zh-CN.md)

Development and verification follow the [technology stack and working rules](AGENTS.md).

LLM connections, the default model, general settings, agents, tools/MCP and Skill configuration are shared across workspaces within the app profile. Models from recently opened workspaces are imported on the first migration; open any other workspace to import its saved settings. Approval bypass and task/conversation history remain workspace-specific. See the [shared settings guide](docs/shared-application-settings.md), available in Korean.

The named UI/tool acceptance matrix is **339/339, zero remaining**. See the [preview.6 changes and verification scope](docs/release-preview6.md).

Open **Tasks → Comprehensive report** to review performed checks, findings, remediation priorities, and unverified items for a site assessment, then export the complete Markdown report. See the [report guide](docs/site-assessment-report.md), available in Korean.

GitHub: [froiibn/ARTEX-Desktop](https://github.com/froiibn/ARTEX-Desktop). The project is currently in **public Windows preview distribution**. The Windows `v0.2.0-preview.6` preview and its corresponding source are available through Releases without requiring login. Under the [handling policy](docs/license-policy.md), the original README's separate use restrictions are not adopted as additional license conditions for this derivative project. The repository was made public on 2026-10-15. The GitHub repository containing earlier development history and `preview.1` has been removed; the cleaned source and verified Windows previews are provided on GitHub. See [repository history](docs/repository-history.md). The original ARTEX author is **Autumn-27**. See [original author, source, and modifications](ARTEX_NOTICE.en.md).

The current development target is a native desktop application running in a single Rust process. The native UI, object models and SQLite storage, and LLM and tool execution services run within the same application. Basic UI operation does not require Go, Node/npm, WebView, or a PostgreSQL server. Individual external tools such as Python and Nmap are required separately when their corresponding features are used.

The project aims to manage tasks, agents, 81 registered tools, assets, evidence, and reports for LLM-assisted security assessment in a local workspace. See [project purpose](docs/project-purpose.en.md).

## Purpose of public release for defensive security

The goal is to help people without a security background assess vulnerabilities in services they develop or operate through penetration testing, understand the findings, and act on them through fixes and security improvements. By lowering the barriers to discovery, remediation, and retesting after fixes, the project aims to help everyday users manage their own services more securely.

The project is published to support security assessment and stronger defenses for services the user owns or has explicit permission to test. A Windows preview is currently provided.

## Why rebuild as a desktop application

Installing Docker, configuring containers, and setting up a web server and PostgreSQL created barriers for non-developers and everyday users. The central purpose of the rebuild is to reduce that preparation so that **people can use ARTEX like a conventional desktop program without development knowledge**.

Users download and extract the executable package, launch the application, and configure a workspace and LLM model to begin. The original agent workflow is retained while conversations, tool execution, approvals, stopping execution, and results are brought together in one application.

- **Simpler setup:** basic application operation requires no separate Docker, web server, or PostgreSQL installation or administration.
- **Local work management:** conversations, tasks, asset relationships and execution records are stored in bundled SQLite, with large evidence kept as files and agent-shared information linked in the same workspace.
- **Integrated execution control:** tool execution on the user's PC, approvals, progress, stopping execution, and results are available in the desktop interface.
- **Unified development and distribution structure:** the UI and application services run in a single Rust process, with a shared codebase targeting Windows, macOS, and Arch Linux. Windows x64 is currently the platform with completed preview distribution and actual GUI verification.

Workspace data is stored locally. LLM calls, remote MCP, and web requests communicate with the configured services or targets. See [project purpose](docs/project-purpose.en.md) for the background and architectural comparison.

## English prompt translation

LLM models differ in how they interpret instructions and invoke tools, so even the same prompt cannot guarantee behavior identical to the original system. Taking these differences into account, the built-in prompts were translated into English while preserving the original roles, context, operating rules, and tool contracts. Actual behavior is checked with the selected model and configuration. Conversations and reports default to Korean, with English available when the user prefers it.

## Folders

- `desktop/native`: current Rust UI and native verification and packaging scripts
- `desktop/native-core`: application services
- `desktop/crates`: shared Rust libraries for storage, domain logic, runtime, host services, tools, and data migration
- `desktop/contracts`: frozen contracts included in the build
- `docs`: design, original compatibility contracts, 81-tool mapping, and platform reviews
- `scripts`: development tools comparing the original Go/Web schemas and behavior
- `desktop/dist`: distribution artifacts
- `.github/workflows/native-rust.yml`: manually triggered OS-specific verification workflow for the current Rust implementation
- `desktop/scripts`, `desktop/tests`, `scripts`: tool, prompt, and original contract comparison checks and contract maintenance tools

The previous Tauri/React application and separate engine client have been removed from the current working tree. The [unused code cleanup record](docs/unused-code-cleanup.md) (Korean) documents the removed scope and recovery baseline. The original Go/Web repository is preserved separately.

## Download a build

Download `ARTEX-Desktop-v0.2.0-preview.6-windows-x64.zip` from the Assets section of the [Windows x64 preview v0.2.0-preview.6](https://github.com/froiibn/ARTEX-Desktop/releases/tag/v0.2.0-preview.6). Extract the entire archive and run `ARTEX-Windows-x64/ARTEX.exe`. The basic application runs without source build tools. The same release also provides its corresponding source ZIP and `SHA256SUMS.txt`.

This is an unsigned Windows preview build without an installer or automatic updates. macOS and Linux executables are not yet provided. See the [first-run guide](docs/download-and-run.md) (Korean).

## Quick start

A workspace is a folder that keeps your conversations and assessment results together. Start with these steps. Button names below match the Korean application UI.

1. **Launch:** extract the entire downloaded ZIP and run `ARTEX.exe`.
2. **Create a workspace:** enter a name on the welcome screen, click **새 작업 공간 만들기** (Create workspace), and choose where to save it. Next time, select a recent workspace or use **폴더에서 작업 공간 열기** (Open workspace folder) to open the same folder.
3. **Connect AI:** open **환경 설정 → LLM 모델** (Settings → LLM models) on the left and enter a name, API format, address, and model using the settings from the service that issued your key. Follow this order: enter API key → **비밀 값 저장** (Save secret) → **모델 설정 저장** (Save model settings) → select the saved model in the list → **기본 모델로 설정** (Set as default). Check it with **저장된 모델 연결 시험** (Test saved model connection). Saving the key alone does not complete setup.
4. **Try a conversation:** open **대화** (Conversations), enter a title, and click **대화 만들기** (Create conversation). First send “Hello. What can you help me with?” and check that you receive a reply.
5. **Check your site:** replace the address in this example with a site you own or have permission to test.

   > https://example.com is my website. Lightly check HTTPS and security headers on its public pages. Exclude bulk requests, login attacks, and data changes. Explain the assessment plan first.

6. **Follow progress and results:** review requested actions in **실행 승인** (Execution approvals). Read answers and tool results in **대화** (Conversations), recorded findings and evidence in **발견 사항** (Findings) and **트래픽·증거** (Traffic and evidence), and progress in **에이전트 실행** (Agent runs). Leave **Bypass** off initially to review tool execution approvals yourself.
7. **Stop and return later:** stop a conversation response from its conversation screen. To stop a whole task, select it under **작업** (Tasks), click **작업 중지·정리** (Stop task and clean up), and check the cleanup result. Reopen the saved workspace to view its history; paused tasks require an explicit resume.

If you see an error about a missing default model, check that you completed **기본 모델로 설정** in step 3. An error about a missing external tool such as Nmap or Python means that tool needs a separate installation. See the [download and first-run guide](docs/download-and-run.md) (Korean) for more information.

## Build and run from source

Install Rust stable, Python 3.11 or newer, and the native build tools for your operating system. On Windows, run the following from the cloned repository directory:

```powershell
python desktop/native/scripts/build.py
& .\desktop\dist\ARTEX-Windows\ARTEX.exe
```

Windows builds always replace `desktop/dist/ARTEX-Windows/ARTEX.exe`. The same build command on macOS/Linux produces `desktop/dist/ARTEX-macOS/artex-native` or `desktop/dist/ARTEX-Linux/artex-native`. All Cargo intermediate files use `desktop/target`. These local builds are excluded from Git and do not replace distributable-release qualification. See [configuration and execution](desktop/native/README.md), [verification status](desktop/native/STATUS.md), and [platform limitations](docs/desktop-platform-audit.md) (Korean). Actual macOS and Arch Linux builds and GUI verification remain separate tasks.

Run the full native verification from the repository directory:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File desktop/native/scripts/verify.ps1 -FullRegression
```

## Original source and Git

This directory is a separate Git repository whose source baselines are uploaded to the public GitHub repository `froiibn/ARTEX-Desktop`. The original Go/Web repository and localization history are preserved in `../original`. Executables, real and test workspaces, API keys, generated logs, and test reports are excluded from Git uploads. The local-only `desktop/dist` executable is not included in a clone; build it with the standard command above.

A normal Cargo build does not read the original repository. Only comparison and generation tools such as `scripts/validate-desktop-contracts.mjs` use `../original` by default. Set `ARTEX_ORIGINAL_ROOT` to use another original source directory. Contract generators rewrite output files, so do not run a generator for a read-only check. Original web development dependencies are used only by the Node-based original comparison tools.

Absolute paths in earlier verification results are retained as historical evidence. Temporary `.resume` scripts may also use the paths from that time; use `desktop/native/scripts` for current work. User data and API keys are managed separately from this source directory.

Local object relationships, conversations, tasks and execution records use bundled SQLite. Large evidence and tool working files remain files. No separate database server is required. See [SQLite storage and legacy migration](docs/sqlite-storage.md) for the transition, preserved originals and verification limits.

The SQLite transition applies to the current development source; existing GitHub release binaries are not updated automatically.
