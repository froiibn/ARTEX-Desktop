# Public suffix resources and licenses

The Rust algorithm in `../../src/psl.rs` is a port of
`golang.org/x/net/publicsuffix v0.58.0`. The `text`, `nodes` and `children`
files are unchanged generated Go table bytes. The Go Authors' BSD-3-Clause
notice is preserved in `LICENSE`. On 2026-10-14 all three files were compared
byte for byte with the pinned Go module's upstream resources:
https://github.com/golang/net/tree/v0.58.0/publicsuffix/data

The underlying Public Suffix List has its own MPL-2.0 terms; the Go code's
BSD notice does not replace the dataset's notice. `MPL-2.0.txt` and the
unmodified preferred source `public_suffix_list.dat` are preserved here.
The source header retains the upstream MPL notice.

Pinned dataset revision: `d6c92f1bbb7433e5db7b8405c25d4035fb8ff376`
(2026-02-06), as identified by the Go package's generated `table.go`.

- Source: https://github.com/publicsuffix/list/blob/d6c92f1bbb7433e5db7b8405c25d4035fb8ff376/public_suffix_list.dat
- License: https://github.com/publicsuffix/list/blob/d6c92f1bbb7433e5db7b8405c25d4035fb8ff376/LICENSE
- Source retrieval in this repository: 2026-10-14, unchanged pinned content.

`go-cases.json` is a differential verification fixture derived through Go's
API. `export.go.txt` is the verification-only exporter. The table revision
and source are also relevant to these data derivatives. Future binary
packages must include both the code and dataset notices and clear access
to the pinned dataset source. This audit did not change the existing table
bytes or Rust runtime.
