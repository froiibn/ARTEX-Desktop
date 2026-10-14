# ARTEX Rust Desktop Rebuild

[한국어](README.md) · **English** · [简体中文](README.zh-CN.md)

GitHub: [froiibn/ARTEX-Desktop](https://github.com/froiibn/ARTEX-Desktop). The project is currently in **private development and review**. The Windows `v0.2.0-preview.2` preview and its corresponding source are available through private Releases, requiring login and repository access. Under the [handling policy](docs/license-policy.md), the original README's separate use restrictions are not adopted as additional license conditions for this derivative project. The repository has not been made public. Earlier development history and `preview.1` are preserved in private [ARTEX-Desktop-History](https://github.com/froiibn/ARTEX-Desktop-History). The original ARTEX author is **Autumn-27**. See [original author, source, and modifications](ARTEX_NOTICE.en.md).

The current development target is a native desktop application running in a single Rust process. The native UI, object and file storage, and LLM and tool execution services run within the same application. Basic UI operation does not require Go, Node/npm, WebView, or a PostgreSQL server. Individual external tools such as Python and Nmap are required separately when their corresponding features are used.

The project aims to manage tasks, agents, 81 registered tools, assets, evidence, and reports for LLM-assisted security assessment in a local workspace. See [project purpose](docs/project-purpose.en.md).

## Purpose of public release for defensive security

The goal is to help people without a security background assess vulnerabilities in services they develop or operate through penetration testing, understand the findings, and act on them through fixes and security improvements. By lowering the barriers to discovery, remediation, and retesting after fixes, the project aims to help everyday users manage their own services more securely.

Public release is being pursued to support security assessment and stronger defenses for services the user owns or has explicit permission to test. The repository remains in private development and review.

## Why rebuild as a desktop application

Installing Docker, configuring containers, and setting up a web server and PostgreSQL created barriers for non-developers and everyday users. The central purpose of the rebuild is to reduce that preparation so that **people can use ARTEX like a conventional desktop program without development knowledge**.

Users download and extract the executable package, launch the application, and configure a workspace and LLM model to begin. The original agent workflow is retained while conversations, tool execution, approvals, stopping execution, and results are brought together in one application.

- **Simpler setup:** basic application operation requires no separate Docker, web server, or PostgreSQL installation or administration.
- **Local work management:** conversations, tasks, assets, and evidence are stored as objects and files, preserving relationships between information shared by agents in the same workspace.
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

Download `ARTEX-Desktop-v0.2.0-preview.2-windows-x64.zip` from the Assets section of the [Windows x64 preview v0.2.0-preview.2](https://github.com/froiibn/ARTEX-Desktop/releases/tag/v0.2.0-preview.2). Extract the entire archive and run `ARTEX-Windows-x64/ARTEX.exe`. The basic application runs without source build tools. The same release also provides its corresponding source ZIP and `SHA256SUMS.txt`.

Sign in with a GitHub account that has access to the private repository. This is an unsigned Windows preview build without an installer or automatic updates. macOS and Linux executables are not yet provided. See the [first-run guide](docs/download-and-run.md) (Korean).

## Build and run from source

Install Rust stable and the native build tools for your operating system. On Windows, run the following from the cloned repository directory:

```powershell
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'desktop/target'
cargo build --locked --manifest-path desktop/native/Cargo.toml
& .\desktop\target\debug\artex-native.exe
```

The local Windows distribution executable on the development PC is `desktop/dist/ARTEX-Windows/ARTEX.exe`. It is excluded from Git; after cloning, build with the commands above. See [configuration and execution](desktop/native/README.md), [verification status](desktop/native/STATUS.md), and [platform limitations](docs/desktop-platform-audit.md) (Korean). Actual macOS and Arch Linux builds and GUI verification remain separate tasks.

Run the full native verification from the repository directory:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File desktop/native/scripts/verify.ps1 -FullRegression
```

## Original source and Git

This directory is a separate Git repository whose source baselines are uploaded to the private GitHub repository `froiibn/ARTEX-Desktop`. The original Go/Web repository and localization history are preserved in `../original`. Executables, real and test workspaces, API keys, generated logs, and test reports are excluded from Git uploads. The local-only `desktop/dist` executable is not included in a clone; build it with Cargo as described above.

A normal Cargo build does not read the original repository. Only comparison and generation tools such as `scripts/validate-desktop-contracts.mjs` use `../original` by default. Set `ARTEX_ORIGINAL_ROOT` to use another original source directory. Contract generators rewrite output files, so do not run a generator for a read-only check. Original web development dependencies are used only by the Node-based original comparison tools.

Absolute paths in earlier verification results are retained as historical evidence. Temporary `.resume` scripts may also use the paths from that time; use `desktop/native/scripts` for current work. User data and API keys are managed separately from this source directory.
