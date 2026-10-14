# ARTEX desktop application icon

The user approved this original design on 2026-10-15. It was generated with the built-in imagegen tool using ARTEX's existing slate and blue UI palette. The A-shaped shield is new desktop artwork; its original pixels are retained in `artex-app-icon-v1.png`. The exact generation prompt is in `artex-app-icon-v1-prompt.txt`.

The artwork is included with this AGPL-3.0-only project under the repository's LICENSE. It does not replace the original ARTEX author or third-party notices.

Derived assets preserve the approved design and transparency:

- `artex-app-icon-256.png`: native window icon.
- `artex-app-icon-32.png`: native tray icon.
- `artex-app-icon.ico`: Windows executable resource with 16, 24, 32, 48, 64, 128 and 256 pixel PNG frames.

Regenerate the files deterministically with the existing Rust image dependency:

```powershell
cargo run --locked --manifest-path desktop/native/Cargo.toml --example export-app-icons
```

`desktop/native/build.rs` embeds the ICO and Windows version metadata using the existing MinGW `windres` or Windows SDK `rc.exe`. Other operating systems do not require a Windows resource compiler. A macOS application bundle icon and Linux desktop launcher packaging remain part of their respective distribution work.
