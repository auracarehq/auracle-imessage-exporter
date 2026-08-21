# Auracle modifications

Auracare added the following to the upstream GPLv3 codebase:

- an independently usable `auracle-imessage-exporter` binary target;
- the `export-jsonl` command and Auracle JSONL v1 protocol;
- deterministic read-only SQLite streaming;
- opaque cursor, edit, reaction, service-message, and attachment-metadata logic;
- `progress` records and `--resume`, so an interrupted pass is picked up from
  its last acknowledged position instead of streamed again from the start;
- Apple association-type classification and recoverable-chat message handling,
  with aggregate-only diagnostics for canonical rows lacking any chat link;
- synthetic fixtures, JSON Schema validation, golden tests, and cursor tests;
- universal macOS build, Developer ID signing, checksum, notarization, and
  release automation; and
- Auracle-specific security, build, release, and validation documentation.
- a semantics-preserving Clippy modernization required to test stable upstream
  4.2.0 with the pinned current Rust toolchain.

The Auracle application repository contains only the protocol and process
integration. This exporter is not an Auracle dependency, submodule, linked
library, or bundled artifact.
