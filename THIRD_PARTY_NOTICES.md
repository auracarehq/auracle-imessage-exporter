# Third-party notices

This repository preserves the complete upstream `imessage-exporter` source and
its copyright notices. The upstream authors and stable base are identified in
[`UPSTREAM.md`](UPSTREAM.md).

Rust dependencies are resolved exactly by the committed `Cargo.lock`. Their
SPDX license expressions and source repositories are recorded in package
metadata and can be audited without executing the exporter:

```sh
cargo metadata --locked --format-version 1 | jq -r '
  [.packages[] | select(.source != null) |
   [.name, .version, (.license // "UNKNOWN"),
    (.repository // .homepage // "")]] |
  sort_by(.[0], .[1])[] | @tsv'
```

Direct third-party crates used by the Auracle protocol target are:

| Crate | Purpose | License metadata |
|---|---|---|
| `base64` | Opaque cursor encoding | MIT OR Apache-2.0 |
| `chrono` | RFC 3339 timestamps | MIT OR Apache-2.0 |
| `clap` | CLI parsing | MIT OR Apache-2.0 |
| `plist` | Apple edit-summary parsing | MIT |
| `rusqlite` / `libsqlite3-sys` | Read-only SQLite access | MIT; bundled SQLite is public domain |
| `serde` / `serde_json` | JSONL serialization | MIT OR Apache-2.0 |
| `sha2` | Database fingerprinting | MIT OR Apache-2.0 |

Test-only direct crates are `jsonschema` (MIT) and `tempfile` (MIT OR
Apache-2.0). The upstream targets have additional dependencies enumerated by
`Cargo.lock` and the audit command above. Dependency source distributions in
the Cargo registry contain their complete license and notice texts.
