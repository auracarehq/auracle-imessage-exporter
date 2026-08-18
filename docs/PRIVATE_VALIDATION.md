# Private corpus validation

The private Messages corpus is untracked and must never be copied into this
repository, staged, attached to an issue, or written to an export file. The
expected reconciliation baseline is approximately:

- 94,190 canonical messages
- 999 chats
- attachment metadata only

Run validation as a stream and retain only aggregate counts and categorized
failures. Never retain stdout, because stdout intentionally contains message
text and handles. Diagnostic stderr is category-only by design, but validation
reports must still be reviewed before sharing.

An acceptable report contains only:

```text
schema_valid=<true|false>
messages=<aggregate>
chats=<aggregate>
attachments=<aggregate>
failure_categories=<category:count,...>
cursor_replay_idempotent=<true|false>
```

Private validation is not part of CI and is not evidence of OSS counsel
approval.
