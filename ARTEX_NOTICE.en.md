# ARTEX Original Author and Modified Version

[한국어](ARTEX_NOTICE.md) · **English** · [简体中文](ARTEX_NOTICE.zh-CN.md)

The original ARTEX author is **Autumn-27**. The rights of the original author and contributors are respected. The original project address is [Autumn-27/ARTEX](https://github.com/Autumn-27/ARTEX). The source used for this rebuild was obtained from [mhtsec/ARTEX](https://github.com/mhtsec/ARTEX), with comparison baseline commit `a9d11cb852eadad00669e4f6a05ee59cbbd14860`.

[ARTEX Desktop](https://github.com/froiibn/ARTEX-Desktop) is a modified rebuild based on the original workflows, prompts, and tool contracts. It is not presented as an official desktop edition distributed or endorsed by the original author.

The central purpose is to let non-developers and everyday users unfamiliar with Docker, server, and database setup use ARTEX like a conventional desktop program. The rebuild reduces separate installation and administration required for basic operation and brings the original agent workflows, local workspaces, tool execution, approvals, and stopping execution into one application. Public release is also being pursued for defensive use: helping people without a security background assess and address vulnerabilities through penetration testing on services they develop or operate, or have explicit permission to test. See [project purpose](docs/project-purpose.en.md).

Major modifications as of 2026-10-14 include:

- Native desktop UI and execution services in a single Rust process
- Object graph and file-based workspace storage with agent information sharing
- Tool execution approvals and stopping controls, conversation and task management
- English translation of built-in prompts preserving the original context, accounting for model-dependent behavior that cannot be guaranteed to remain identical
- Korean and English interface text and desktop layout

The project as a whole retains **AGPL-3.0-only**. Existing rights and notices of the original author and contributors are not replaced. Redistribution and modification follow the terms of [LICENSE](LICENSE). The program is provided without warranty as specified in that license. Modified source is available through the ARTEX Desktop repository above and corresponding source supplied with releases.

The original README's prohibitions on online and practical use are handled under the [policy based on AGPL sections 7 and 10](docs/license-policy.md) and are not adopted as additional license conditions for this derivative project. Original author, attribution, warranty, modification, and AGPL notices are preserved, as are obligations under applicable laws and separate agreements. This is not a guarantee that all legal disputes or third-party rights have been resolved.

The project remains in private development and review. Windows `v0.2.0-preview.2` and its corresponding source are available through private Releases. See the [final artifact and verification record](docs/public-release-remediation-result-20261014.md) for notices, source and verification scope. The repository has not been made public.
