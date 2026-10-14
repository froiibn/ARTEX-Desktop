# ARTEX Desktop Project Purpose

[한국어](project-purpose.md) · **English** · [简体中文](project-purpose.zh-CN.md)

ARTEX Desktop is a native Rust desktop application for managing tasks, agents, tools, and evidence for LLM-assisted security assessment on the user's computer. It is a modified rebuild based on the original ARTEX workflows, prompts, and tool contracts. The project as a whole currently uses AGPL-3.0-only.

After the user configures a model, objectives, and scope, agents divide objectives and invoke tools. The user can view conversations, approvals, execution status, stopping controls, and results in one interface. Assets, findings, execution records, files, and evidence are linked within the workspace for reporting and retesting. The 81 registered tools include file, command, MCP, task management, and information-recording functions. This does not mean that the product installs 81 separate security scanners.

## Why move to a desktop application

The central purpose is to let **non-developers and everyday users use ARTEX like a conventional desktop program without development environment or server administration knowledge**. The original Docker Compose deployment required Docker installation and container and PostgreSQL configuration. Building from source required development tools and an understanding of the build process. These setup steps created barriers for everyday users.

The intended workflow is to download the executable, open the application, configure a workspace and LLM model, and begin work through the interface. The original agent workflows and tool contracts are retained while reducing setup and administration and bringing conversations, tasks, and execution results into one program.

**Reduce installation and administration.** Basic application operation excludes the need to install, configure, or administer Docker, containers, a web server, or PostgreSQL. Storage is redesigned as an object graph with file persistence suited to local desktop workspaces. Agents continue to share information through object relationships, permissions, and durable storage. The basis for this choice is simpler operation on a personal computer.

**Bring work and execution control into one interface.** Conversations, task lists, agent activity, tool approvals, stopping execution, and results are visible within the application. Local files and external programs run on the user's computer, so execution status and termination are managed with the application's lifecycle.

**Adapt data and credentials to the local environment.** Conversations, assets, findings, and evidence are linked in file-based workspaces. API keys use the operating system's credential store wherever possible. Model calls and remote tools communicate with the configured services.

**Unify the UI and application services in Rust.** The UI, storage, agents, and execution services run in the same process, avoiding a separate server and connections between service processes for basic operation. Storage, approvals, cancellation, and shutdown are managed within one application structure, with a shared development and verification foundation for Windows, macOS, and Arch Linux. Packaging and actual operation must still be verified on each operating system.

| Component | Original ARTEX | Current ARTEX Desktop |
| --- | --- | --- |
| Basic execution | Go web server and Next.js interface | Native UI and application services in the same Rust process |
| Workspace information | PostgreSQL-based sharing | Object graph and durable file storage; sharing through explicit object relationships and permissions |
| Getting started | Deploy server and database using Docker Compose or another method, then access the web interface | Download the executable, launch the application, and create or open a local workspace |
| Tool execution | External programs in the server environment | Run and supervise programs on the user's computer when needed |
| Credentials | Server configuration and storage | Prefer the OS credential store and retain references in objects |

Basic operation of the native application requires no separate Go server, JavaScript frontend, WebView, or PostgreSQL. The previous Tauri/React code and separate engine client have been removed from the current working tree. External programs such as Python, Nmap, shells, and user-configured MCP servers have not all been reimplemented in Rust.

## Why translate the built-in prompts into English

LLM prompt behavior varies with the model and configuration. Keeping the original wording does not guarantee identical role execution, tool calls, or responses, so the built-in prompts were translated into English while preserving the original intent and context.

The translation was structured to preserve each agent's role and responsibilities, scope, evidence recording, termination conditions, and tool invocation and output contracts. English is used for the default role prompts, wrap-up prompts, and approval judgment prompts. Actual behavior and results are checked with the selected model, settings, and execution environment. Conversations and reports default to Korean, with English available on request.

## Target platforms and purpose of public release

The target operating systems are Windows, macOS, and Arch Linux. Windows x64 currently has completed distribution and actual GUI verification. Actual macOS and Arch Linux verification and packages remain to be provided.

The purpose of pursuing public release is to **help people without a security background assess and address vulnerabilities in services they develop or operate through penetration testing**. Findings and evidence help users understand problems, apply code fixes or security configuration improvements, and retest to strengthen the service's defenses.

The intended scope is security assessment and research on targets the user owns or has explicit permission to test. The project is published to make security assessment more accessible to everyday users and developers and support defensive use through vulnerability discovery, prevention, and remediation.

The original README's separate prohibitions on online and practical use are handled under the [policy based on AGPL sections 7 and 10](license-policy.md) and are not adopted as additional license conditions for this derivative project. The defensive purpose is not a new purpose-specific restriction on AGPL rights. Original author, attribution, warranty, modification, and AGPL notices and obligations under applicable laws and separate agreements remain in place; the software license does not grant access or testing permission for target systems.

The current source and Windows `v0.2.0-preview.2` preview were made public on 2026-10-15. Corresponding source and license materials are provided together; see the [final artifact and verification record](public-release-remediation-result-20261014.md) for the verification scope. The earlier [license review](license-audit-20261014.md) (Korean) is preserved as historical evidence of the state at that time.

Workspace data is local, but using LLMs, remote MCP, web requests, and external security tools communicates with configured services and targets. The application is not described as fully offline or as never sending information externally. The terms of external tools and services apply separately from the project's AGPL source license.
