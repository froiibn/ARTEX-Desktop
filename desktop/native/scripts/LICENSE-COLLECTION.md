# Native distribution license collection

Run with Python3.11 or later after a locked native Windows GNU release build:

```powershell
python desktop/native/scripts/collect-licenses.py `
  --output <new-payload-directory>/licenses `
  --binary <new-payload-directory>/ARTEX.exe `
  --link-map <local-audit-directory>/native-link.map `
  --cache <local-audit-directory>/license-cache `
  --target x86_64-pc-windows-gnu
```

The release build must emit the actual GNU linker map with
`-C link-arg=-Wl,-Map,<absolute-map-path>`. Paths are command arguments, not
hard-coded developer paths. Use `CARGO_ENCODED_RUSTFLAGS` when a path contains
spaces. The collector does not build the application or change Cargo.lock.
Cargo metadata/tree use `--locked --offline`; upstream material may be downloaded
into the separate cache. After an online collection, `--offline` verifies the
same pinned material without network access. Cache receipts preserve original
URL/digest and immutable-revision404 findings. Output must be a new directory;
previous distributions are never overwritten.

`INDEX.json` is the successful integrity manifest. Distribution code must check
its `status == "verified-distribution-inputs"`,
`binary_distribution_ready == true`, every listed file's hash, and absence of
`INCOMPLETE.json`. Match `runtime/provenance.json.binary_sha256` with ARTEX.exe and
`rust/index.json.cargo_lock_sha256` with the corresponding source lock. Match the
collector/input policy hashes with the source archive. An unsuccessful run exits
nonzero and marks partial output undistributable. `--notices-only` is an analysis
mode with `binary_distribution_ready == false`; it cannot qualify a binary.

The Cargo normal/build graph excludes development dependencies and still includes
build scripts/proc macros, so the index is not an embedded-code count. Every
selected registry archive is checked against Cargo.lock. Cached package source
files are compared with the original archive before extracting license, NOTICE,
copyright, original source headers and embedded font metadata. Historical slash
expressions are interpreted as alternatives; AND conditions are retained.
Original nested notices are kept independently of the selected OR branch.

An exact-version source offer is included for MPL libraries. selectors carries
MPL source headers without a separate LICENSE; the collector preserves its
original header, adds the unmodified standard MPL text, and does not substitute
unrelated repository subproject licenses. The pinned PSL preferred source and
BSD/MPL terms are included. Norma's Apache notice, Brevduva LICENSE/NOTICE and
reference scope are retained. Default UI fonts keep OFL/Ubuntu and Hack's
MIT/Bitstream terms and original copyright/name records. The Korean static font
retains its original OFL and modification provenance.

Audited old declaration-only packages are version restricted. They receive
unmodified standard terms plus exact original declaration/source attribution,
clearly labelled as supplements. No fabricated author-specific notice is claimed.
The Google-origin Brotli source's MIT header remains an additive notice even when
the package's BSD alternative is selected. New or unsupported licensing cases
fail and require a policy review.

`license-inputs.json` pins required resource hashes and the reviewed GCC15.1 /
MinGW13.0 Winlibs UCRT-r2 distribution archive and original runtime notices.
Loaded compiler/runtime files and gcc.exe must match that complete verified
archive. Rust runtime archives must match the rustup channel manifest's pinned
rust-std archive. Unknown native runtime inputs, Rust rust-mingw inputs outside
that reviewed scope, changed toolchain versions or altered cached sources fail.
Only Windows GNU x64 native runtime is qualified by this policy.

GNU map LOAD inputs include unused import archives and rustc's temporary LTO
objects. The former are conservatively recorded, not called Microsoft DLL
redistribution. rustc deletes temporary objects/trimmed link archives after
linking: their absence is explicitly recorded with no intermediate-byte matching
claim. Corresponding retained Rust/Cargo archives are identified; external
GNU/MinGW runtime files still require actual byte matching. No map or local
developer path is put in the portable output. SHA/version/source evidence is
recorded instead.

`legal_certification` remains false. This is reproducible technical provenance and
notice validation, not a replacement for resolving original-source restrictions,
trademark questions, or legal review of a particular distribution.
