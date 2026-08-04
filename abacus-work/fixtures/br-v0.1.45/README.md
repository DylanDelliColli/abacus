# br v0.1.45 destructive-sync fixtures

These fixtures capture the minimum provider facts needed to test the future
abacus-work process adapter without invoking a live br binary. All paths,
timestamps, and generated IDs are sanitized or deterministic.

Each file separates:

- the exact command/process observation from the disposable spike;
- the verified postcondition; and
- the normalized adapter observation that callers may act on.

The expected_adapter objects are seam-level requirements. Future Rust type
names may differ, but raw br codes/messages must not escape and the semantic
fields must remain equivalent.

Interrupted operations were killed only after a provider-created artifact was
observed. Contract tests must never read numbered JSONL temporary files as the
canonical graph.

Fixture catalog:

- sync-conflict.json — merge markers are rejected before import, including
  with --force.
- sync-malformed-jsonl.json — malformed JSON is rejected before import,
  including with --force.
- database-busy.json — an external writer produces a bounded busy failure.
- database-corrupt.json — unrecoverable and automatically recovered corrupt
  database shapes.
- interrupted-flush.json — canonical JSONL survives a hard interruption;
  retry succeeds while the partial temp file remains.
- interrupted-rebuild.json — hard interruption can leave no active database;
  intact JSONL supports an explicit successful rebuild.
- scope-label-constraints.json — provider label syntax/cardinality facts used
  by the scope-expression design.
