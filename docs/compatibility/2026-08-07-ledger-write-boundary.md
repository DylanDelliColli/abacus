# Shared-store write-boundary compatibility probe

Date: 2026-08-07
Host: Linux 6.6.87.2-microsoft-standard-WSL2, x86_64
Codex CLI: v0.146.1
`br`: v0.1.45
Status: a Codex worker can write its linked worktree but not the repository's
Git common directory. Pinned `br` can explicitly select one shared
worktree-resident store through `BEADS_DIR`; Codex needs that exact directory
added as a writable launch root.

## Questions

1. Can a sandboxed Codex worker write a proposed Ledger at
   `<git-common-dir>/abacus/state.sqlite3` directly?
2. If stock `br` becomes the sole store, can linked worktrees address one
   shared database rather than creating one database each?

## Git-common-directory probe

A disposable main repository and linked `worker` worktree were created below:

```text
/home/ddc/dev-environment/abacus-directstate-probe.SoN4n4
```

The shared Git common directory was the main repository's `.git`, outside the
linked worktree. A fresh ephemeral Codex process ran in the worker worktree
with:

```text
--ephemeral --ignore-user-config --ignore-rules
-s workspace-write
-c approval_policy="never"
```

No operator approval or filesystem override was supplied.

| Probe | Result |
|---|---|
| Create `workspace-write-ok` inside the linked worktree | success, exit 0, empty stderr |
| Create `<main>/.git/abacus-probe` | refused, exit 1: `Read-only file system` |

The positive control proves the ordinary workspace grant was active. The
negative row proves that grant does not follow Git's worktree indirection into
the common directory. A worker cannot directly mutate the proposed
Git-common-directory Ledger.

An earlier `/tmp` topology was discarded: that Codex session had an
independent writable `/tmp` grant, so success there did not measure linked
worktree reachability.

## Stock-`br` follow-up

Local `br where --json` and `br info` confirmed the pinned provider is
daemonless/direct and stores its database and JSONL below `.beads` in the
discovered worktree. The existing `br` compatibility record had already shown
that a linked worktree lazily creates its own database from its own checked-out
JSONL; default discovery therefore does **not** provide shared live state.

Pinned upstream source and local invocation established two explicit selection
mechanisms:

- absolute `BEADS_DIR`; and
- the `--db` option.

From outside the repository, setting `BEADS_DIR` to the control checkout's
absolute `.beads` directory made `br where --json` resolve that exact database
and JSONL. `BEADS_DIR` is the appropriate launch-wide selection because every
provider invocation then agrees without repeating a database flag.

The same source inspection established:

- `br update --claim` performs its unassigned check and compare-and-set inside
  the mutation transaction; the existing two-contender probe produced one
  winner;
- `br comments add` appends one comment transactionally, assigns an ID, marks
  the issue dirty, and exports comments inside `issues.jsonl`; and
- native `br` audit events are explicitly local-database-only and never
  exported to JSONL, so they cannot be ABACUS's portable workflow history.

The ordinary Codex worker still cannot write a sibling control checkout merely
by setting `BEADS_DIR`; the exact control `.beads` directory must be supplied
as an additional writable root at launch. `codex --help` exposes `--add-dir`
for that purpose. Under ADR-0006 this is intentional direct access to the one
work store, not an attempted sandbox boundary around a secret or second
database.

## Conclusion

The first probe invalidated direct writes to a Git-common-directory Ledger. It
did **not** prove a host command was intrinsically necessary. That conclusion
depended on retaining the second Ledger.

With ADR-0006, the writable object is the shared stock-`br` store. One injected
absolute `BEADS_DIR` plus the exact Codex writable-root grant yields a single
daemonless database for all linked worktrees. No Unix socket, relay, host
writer, one-shot state RPC, or per-worktree JSONL merge is required.

This evidence proves reachability and provider primitives only. It does not
claim that stock `br` enforces ABACUS lifecycle conventions. Direct provider
access and that limitation are explicit v1 decisions in ADR-0006.

## Cleanup

The fresh Codex process exited. The disposable main repository, linked
worktree, and probe root were removed, and exact-path absence was confirmed.
No global Codex, Claude, Git, shell, hook, provider, approval, or sandbox
configuration changed.
