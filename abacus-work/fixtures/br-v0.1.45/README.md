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

Happy-path and observation-surface fixtures captured 2026-08-05 in a
disposable scratchpad workspace against the same pinned binary
(`br 0.1.45`), for the omw.2 adapter:

- read-surface.json — `ready --json` omits labels and defaults to
  `--limit 20`; `show --json` carries labels and dependents. The adapter
  needs `--limit 0` plus one `show` per ready id, revision-bracketed.
- status-mutations.json — update/close/reopen output shapes; free-text
  close reasons round-trip verbatim, so curated reasons need exact
  canonical renderings; `Z` and `+00:00` timestamp forms both occur;
  mutation outputs carry no revision material.
- deletion-tombstone.json — deletion yields `status: "tombstone"` at exit
  0, never an error; a never-existing id yields exit 3 `ISSUE_NOT_FOUND`.
  Both normalize to `NotFound` so `compare_observation` reports the typed
  `Missing` anomaly.
- revision-bracketing.json — `sync --status --json` exposes
  `jsonl_content_hash` (64-hex, direct `ContentHash` fit) as the
  `WorkRevision` source, valid only at `dirty_count` 0 with `db_newer`
  false; br auto-exports after each mutation.
- output-schemas.json — one emission of `br schema` (all output-type JSON
  Schemas) at the pinned version. br marks this surface "not a stable
  API" and stamps a per-emission `generated_at`, so it is pin-change
  drift evidence for the adapter's parse types — never a runtime
  verification surface. Runtime verification stays on the checksummed
  binary identity and `br --version`.
