# Shift report — 2026-08-07 (Claude lineage)

For the incoming Claude session. Written to the fresh agent test: you should be
able to act from this document plus the repository without re-deriving today.

---

## 0. READ THIS BEFORE YOU READ ANY SOURCE — bounded authority inversion

Today redirected ABACUS four times. That creates a **temporary, bounded
inversion**: inside named paths, doctrine outranks source. It is **not** a
repository-wide licence to dismiss surprising code. Outside those paths, source
is ordinary evidence and you should trust it normally.

### Authoritative doctrine is a CHAIN, not one commit

No single SHA makes the documents current. Three do, in order, and each proves a
different claim:

- `dafbed3` — *establishes the collapse*: `CONTEXT.md` (invariants I1–I19),
  `docs/adr/0006-stock-br-single-store.md`, `docs/architecture.md`,
  `docs/migration.md` and all five module READMEs describe the **single-store,
  no-Scribe, conventions-not-enforcement** model as accepted product intent.
- `5ad9f4d` — *applies the first subtraction*: the five-item inert authorization
  cluster is gone from source, so doctrine and code already agree there. **It
  also changed `abacus-core/README.md`, `abacus-state/README.md` and
  `docs/migration.md`**, so "the docs are correct as of `dafbed3`" is false —
  those three are only correct at `5ad9f4d` or later.
- `b714451` — *records the remaining replacement work* as `abacus-yx3`.
- `fb5de70` — *parks the attention service*:
  `docs/adr/0004-attention-service.md` is **PARKED, not a contract**. Its
  content is design history, not instruction.

### Bounded stale roots — doctrine wins here

Exactly these, and nothing else:

| Path | Why it remains | Still valid for | No longer proves |
|---|---|---|---|
| `abacus-state/src/**` | replacement not built | build continuity, gate green | product intent |
| `abacus-core/src/usecase.rs` | composes the superseded seams | executable behavioural contract | target design |
| `abacus-core/src/ports.rs` (state port, application/receipt/supersession types) | consumed by the above | same | same |
| `abacus-usecase-journeys/tests/*.rs` | four journeys exercise that composition | regression safety during replacement | that the product works this way |

`abacus-state/README.md` line 1 self-declares `transitional contract` — that
marker is the boundary, not this document.

### Explicitly NOT stale — treat as ordinary evidence

`abacus-runtime/src/**` (Herdr adapter; `ed7fb3e` and `f293eb4` are current
work), `abacus-work/src/**` (the `br` adapter — *more* central now), and the
remaining `abacus-core/src/*.rs` domain vocabulary. If something there surprises
you, it is a real finding. Investigate it.

### The rule, stated with both boundaries

**Within** `abacus-core`, `abacus-state`, and the four journey tests, **on the
removed Ledger / Scribe / authorization semantics tracked by `abacus-yx3`**,
named doctrine wins until replacement lands.

**Outside** that subject-and-path boundary, source remains ordinary evidence,
and a surprising disagreement is a **defect to investigate**, not something to
dismiss. Both halves matter: the first prevents you re-litigating settled
decisions from stale code, the second prevents you waving away a real bug.

### What ends the inversion

`abacus-yx3` — staged deletions **by replacement**. The inversion ends when the
shared-`br` facade replaces those consumers and the stale roots are removed.
Completion is observable: `abacus-state` leaves `Cargo.toml` workspace members,
or `abacus-yx3` closes. Until then, do not "fix" the inconsistency you will
notice; it is deliberate and tracked.

### Probes — run these, do not trust the prose

Every probe below was executed and passed at authoring time. **No HEAD SHA is
written here on purpose** — committing this file changes HEAD, which is exactly
how the first version of §2 falsified itself (§10). Re-run these before acting.

```sh
git merge-base --is-ancestor dafbed3 HEAD && echo "doctrine contained"
git merge-base --is-ancestor 5ad9f4d HEAD && echo "subtraction contained"
git status --porcelain | grep -v '^??'          # expect only .gitignore
head -1 abacus-state/README.md                  # expect: transitional contract
br show abacus-yx3                              # open => inversion still active
```

If containment fails, HEAD moved past this report and §2 is stale — re-derive
from `git log` rather than from this document.

---

## 1. What changed today, and why

Four reversals, each driven by operator instinct or external evidence rather
than by either agent lineage noticing on its own. That pattern matters — see §7.

**1. The attention service is PARKED** (`fb5de70`). Nine revisions and three
reviewers specified a detector for silent stalls in a system that has never run
and therefore has never stalled. Design is sound, review passed, parked anyway.
`docs/adr/0004-attention-service.md` is marked **PARKED — not a contract, do not
implement**, with three known residuals left deliberately unrepaired. It unparks
on evidence: a real stall observed in a running ABACUS.

**2. The threat model was wrong.** Operator: *agent identity fraud was never a
real SABLE problem; accretion was.* Agents are **honest and unreliable** — they
crash, hang, act on stale information, and are confidently wrong. They do not
impersonate each other. This licensed everything after it: an entire
authorization/capability/scope apparatus was defending a threat nobody had.

**3. Scribe is DELETED.** The relay existed because a sandboxed Codex cannot
open a Unix socket — but seccomp blocks socket *creation*, not file reads and
writes (`docs/compatibility/2026-08-04-scribe-socket.md:73,102`). The transport
problem was one we created by choosing a socket. `ADR-0005` is **Withdrawn**,
`ADR-0003` collapsed to history.

**4. The separate Ledger is GONE.** Stock `br` is the single durable store
(`docs/adr/0006-stock-br-single-store.md`, accepted by the operator). `br` is
SQLite + JSONL, daemonless by design; we were building a second SQLite store
beside it. The **entire two-store consistency subsystem** — application
attempts, receipts, causal supersession, pending projection, the two-phase
Acceptance saga — is obsolete, because it existed only to reconcile two stores.

Rejected with reasons, do not reopen without new evidence: **returning to `bd`**
(it runs a Dolt server — `SABLE/.beads/dolt-server.lock` — reintroducing the
daemon we just deleted, plus measured tax: deadlocks, 15/15 read timeouts under
a 15-worker test, 419-commit drift), and **forking `br`** (the tracker is a
two-way door since JSONL is portable; a fork is one-way).

---

## 2. Where the tree is — LANDED vs PLANNED vs THIS PROSE

Three categories, deliberately not blurred:

- **LANDED** — in `origin/main`, verifiable by the §0 probes. Everything in the
  table below.
- **PLANNED** — decided but not built: the staged deletions (`abacus-yx3`), the
  shared-`br` facade, the acceptance gate (`abacus-w6x`). No source exists.
- **THIS PROSE** — my summary. Where it disagrees with a probe, the probe wins.

Three fixed points, none of which is "current HEAD":

- **Pre-report base:** `b714451` — the last landing before this document existed.
- **Report object:** `0296804` — the commit that introduced this file. A later
  commit revises it; see §10.
- **Current HEAD / origin:** *deliberately not written here.* It is a boot-time
  probe (§0), because the act of committing this report invalidated the first
  version of this very line.

Working tree when authored: clean except an unrelated modified `.gitignore`
(Codex's, leave it), `.beads` local metadata, and shift reports.

Today's landings, newest first:

| Commit | What |
|---|---|
| `b714451` | backlog: closed `3y7`, staged remainder as `yx3` |
| `5ad9f4d` | **first source deletion** — inert authorization surface, −249 lines |
| `dafbed3` | normative single-store collapse, −668 doc lines |
| `cbb0bc4` | backlog: br-as-Ledger decision recorded, Scribe beads closed |
| `fb5de70` | attention service parked |

Gates at HEAD: **293 tests / 20 targets green**, `clippy --workspace
--all-targets -D warnings` clean, `fmt` clean. Verify with those exact commands;
do not trust this line.

Module sizes: core 14,055 · state 10,396 · work 4,509 · runtime 2,237 ·
journeys 1,041.

---

## 3. The gate: nothing new gets written

**The operator holds all new code** pending a *necessity round* they have asked
for and not yet scoped. Deletions and docs proceed; new source does not. Both
lanes are honouring this. Do not start implementing.

The round decides the minimum record set: which append-only records exist, which
lifecycle states stay distinct, and whether each of lease, numeric fencing,
operation idempotency, Signal taxonomy, audit index, profile activation,
decision-owner metadata and runtime association **earns its cost**. Candidates
are enumerated in `abacus-yx3`.

**Agreed next step**: run `abacus-2is`, the live-provider vertical pilot. Every
conclusion reached today was reasoning about a system that has never executed.
The operator acknowledged this as the next step immediately before requesting
this shift change.

---

## 4. My lane (Claude): what I left

Runtime adapter, `abacus-runtime`. Unaffected by the Ledger decision — Herdr is
still the runtime provider.

- **`gyh.2` landed in two parts.** `ed7fb3e` is the pinned Herdr identity gate:
  fail-closed on version drift, the 17→19 protocol change, schema drift, and
  unparseable output; `NotPermitted` and `Unavailable` deliberately *not*
  collapsed into a version verdict. `f293eb4` is target resolution.
- **Read `abacus-runtime/src/target.rs` before touching targeting.** The
  bare-kind obligation is easy to implement backwards and the backwards version
  looks correct in review. It is not "reject the strings `claude`/`codex`" — the
  compatibility record shows panes renamed to exactly those names resolving
  fine. It is "never resolve via the detected-kind *field*." I nearly shipped
  the wrong one.
- **`gyh.3` is claimed but unwritten** — generation-fenced handle binding. Halted
  by the operator hold, not by a problem.
- **My three reactive items are all closed.** Nothing is queued from Codex.

Honest note: both my runtime commits say red was not observed first. I wrote
tests and implementation together, then mutation-tested to prove the tests bite.
The commits say so. Don't repeat the pattern; do repeat the disclosure.

---

## 5. Codex's lane

Owns `abacus-core`, `abacus-state`, and the ADRs. Currently idle and holding.

They landed the normative collapse and the inert subtraction today, both under
C3 cross-review from me. They are waiting on the necessity-round scope.

**Coordination protocol that works and should continue:** exclusive lanes, real
adversarial review at C1+, and neither lineage conceding to the other — when we
disagreed we took both positions to the operator. Codex refused to smuggle in a
weaker security claim, aborted an ADR landing mid-flight when a premise changed,
and corrected two of my over-claims. Treat their findings as findings.

Message them with `herdr agent prompt codex "..."`.

---

## 6. Decisions settled — do not relitigate

- Stock `br`, one shared **absolute `BEADS_DIR`** injected at launch.
  Per-worktree stores are **rejected** — `br` walks up from cwd, so a linked
  worktree wins discovery and gets its own database converging only through
  JSONL merge, which arbitrates far too late.
- The shared store sits outside the worker sandbox and needs **one narrow
  writable-root grant at launch**. SABLE hit this exact wall (`SABLE-9qqrv`:
  workers could not even *read* the tracker) and fixed it exactly this way, with
  a negative control.
- **Conventions plus visibility, not an access boundary.** Agents write `br`
  directly; no wrapper can enforce anything, and `CONTEXT.md` I3/I10/I17 say so
  explicitly rather than pretending otherwise.
- **Four data-shape protections are the floor**, each from a measured SABLE
  failure, none requiring enforcement: append-only authorization/decision facts;
  accepted completion distinct from Publication; claimed/launched/parked/
  dead/successor distinguishable; Handoff typed and never a bead.
- `bv` stays advice-only and deletable. Its `--robot-alerts` may later be the
  cheapest answer to attention — reach for it before unparking `ikq`.

---

## 7. What the methodology is doing, honestly

**Every consequential finding today came from reading code or probing a sandbox
— not from review of documents.** Two internal lineages passed a design four
times; an external reviewer then found four blockers by asking whether the thing
would actually work. The lesson is in `AGENTS.md`: *verify the seam before you
draft*, and *judge a round by what it caught, not its ratio to shipped code*.

I broke my own rules twice and both are recorded rather than tidied away: I
claimed a whole cut list was "verified inert" having verified two items, and I
nearly deleted a dead method while four normative documents still required the
mechanism — the exact defect class I had raised against Codex twice the same
day. Enforcing a rule on someone else is no protection against walking into it.

**The reduction is committed to, not collected.** 249 source lines removed
against ~24,700 remaining. All of the real deletion arrives as *replacement*.

---

## 8. Traps

- **Tree claim.** `sable-claim` needed the `--force` escape hatch on essentially
  every handoff today, in both directions. `sable-claim release "$(pwd)"`, then
  `--force` if the holder id is stale. Legacy machinery; do not try to fix it.
- **Commit messages: use `git commit -F <file>`.** Backticks in a `-m` string
  get shell-interpreted and silently eat words. It happened to me; I had to
  amend.
- **`br close` needs `--force`** when an issue has open dependencies, and the
  failure message says "already closed or not found," which is misleading.
- Commit with `-c core.hooksPath=/dev/null`; legacy SABLE hooks fire here.
- Never use `bd` or `sable-note` in this repo.

---

## 9. Suggested first moves

1. Read `docs/adr/0006-stock-br-single-store.md` and `CONTEXT.md`. Everything
   else derives from them.
2. Confirm the gates yourself: `cargo test --workspace`, `cargo clippy
   --workspace --all-targets -- -D warnings`.
3. Ask the operator for the **necessity-round scope**. It is the only gate.
4. When it lifts, `abacus-2is` — the live pilot — is the agreed next step, and
   the most valuable open bead.
5. Do not implement anything from a bead written before `dafbed3` without
   checking it against ADR-0006 first. Several still describe the old design;
   the ones I found are annotated, but I will not have found all of them.

---

## 10. Corrections to this report — kept deliberately

An external review read this document **after it landed** at `0296804` and found
three fresh-agent-test defects. They are recorded rather than silently amended,
because a handoff that looks flawless teaches the next author nothing.

**1. The report invalidated its own line by existing.** §2 originally stated
`main == origin/main at b714451`. Committing the report made that false
immediately. *Lesson: a handoff must never write current HEAD as durable prose.*
Fixed by splitting into pre-report base, report object, and a boot-time probe.

**2. Authority was attributed to one commit when it is a chain.** §0 originally
said the documents were correct "as of `dafbed3`". But `5ad9f4d` subsequently
changed `abacus-core/README.md`, `abacus-state/README.md` and
`docs/migration.md` — so for those three, `dafbed3` was already stale when I
wrote it. *Lesson: "these docs are current at SHA X" is almost always wrong when
work continued; state the chain and what each commit proves.* I did not catch
this myself.

**3. The inversion rule was still too global.** "If source and `CONTEXT.md`
disagree, `CONTEXT.md` wins" makes every surprising line dismissible and fails
the fresh-agent test. It now carries **both** a path boundary and a subject
boundary, and explicitly says surprising disagreement *outside* those bounds is
a defect to investigate.

The pattern across all three: **unbounded confidence.** Each was a true
statement over-scoped into a false one — the same failure mode that produced two
of my errors in §7. Bound your claims to the evidence that supports them.
