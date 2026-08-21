# auracle-imessage-exporter

`auracle-imessage-exporter` is a standalone GPLv3 command-line program that
streams a local macOS Messages archive as deterministic JSONL. It is useful
from a terminal without Auracle. Auracle invokes a separately installed copy;
the executable is not linked, vendored, downloaded by, or bundled with the
Auracle application.

> **Legal release gate:** the GPL process-separation boundary still requires
> OSS counsel review before general Auracle distribution. A release from this
> repository must not be described as legally cleared merely because its
> source, signature, checksum, or CI checks are complete.

## Command

```sh
auracle-imessage-exporter export-jsonl \
  --db-path ~/Library/Messages/chat.db \
  --attachments metadata \
  [--cursor <opaque-cursor>] \
  [--resume <progress-cursor>]
```

The first stdout line is a `manifest`, the last is a `checkpoint`, and every
line conforms to [`docs/auracle-imessage-jsonl-v1.schema.json`](docs/auracle-imessage-jsonl-v1.schema.json).
All diagnostics are data-free categories written to stderr.

Every 200 messages the stream carries a `progress` record holding an opaque
cursor for the position just streamed. A pass that was interrupted can be
picked up from the last such cursor its consumer acknowledged by passing it as
`--resume`: the manifest, every handle and every chat are streamed again, and
then only the messages the earlier pass had not yet carried — plus any message
edited, or reacted to, since that pass read the database. `--cursor` keeps its
meaning alongside it, so a resumed incremental pass stays incremental. A
`--resume` cursor from a different database is ignored with a
`resume_ignored` diagnostic and the pass streams in full.

The exporter:

- opens SQLite read-only and streams message and attachment records;
- never emits a filesystem path or reads attachment contents;
- emits attachment association, MIME type, basename, byte count, and
  availability only;
- retains direct/group participant and sender attribution;
- folds tapback reactions into their canonical message, retains normal app
  payload variants, and suppresses service/association event rows;
- retains canonical messages linked through Apple's recoverable-chat table and
  reports truly unassociated rows only as an aggregate, data-free diagnostic;
- reads Apple's modern edit summary and emits the latest canonical edit;
- produces stable source identifiers and deterministic ordering;
- returns an opaque resumable cursor;
- marks resumable positions with `progress` records and picks a pass up from
  one with `--resume`; and
- switches to a full export when the cursor belongs to a different database
  fingerprint.

`checkpoint.totals.records` includes the manifest, every `progress` record and
the checkpoint. Message and chat totals count records in the current pass, not
the lifetime database total.

## Install a release

Download both adjacent assets from
<https://github.com/auracarehq/auracle-imessage-exporter/releases>:

```text
auracle-imessage-exporter
auracle-imessage-exporter.sha256
```

Then verify before execution:

```sh
shasum -a 256 -c auracle-imessage-exporter.sha256
codesign --verify --deep --strict --verbose=2 auracle-imessage-exporter
spctl --assess --type execute --verbose=2 auracle-imessage-exporter
chmod 755 auracle-imessage-exporter
```

Auracle performs its own signature and adjacent-checksum verification when the
member selects this binary in “Locate exporter…”.

## Build and test

See [`docs/BUILDING.md`](docs/BUILDING.md) for pinned, reproducible commands.
The required local gates are:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

Synthetic SQLite fixtures cover direct/group chats, incoming/outgoing
messages, modern edits, reactions, service records, attributed bodies, missing
attachments, cursors, idempotency, schema validation, deterministic golden
output, and older database schemas. No private archive is included.

## Upstream and license

This repository is a fork of Christopher Sardegna's
[`ReagentX/imessage-exporter`](https://github.com/ReagentX/imessage-exporter),
based on the stable `4.2.0` tag (`f4021a1113bd47400ceaedfb79907ef2c63624a9`).
The full upstream source and history are retained. See [`UPSTREAM.md`](UPSTREAM.md),
[`MODIFICATIONS.md`](MODIFICATIONS.md), and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

The complete work is licensed under GNU GPL version 3 or later. See
[`LICENSE`](LICENSE). Upstream and third-party copyright notices remain in the
source and notice files.
