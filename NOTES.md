# ABACUS build notes

## 2026-08-06 — caller identity locked, and the ceremony budget (operator decisions)

Two operator decisions worth reading before touching the state seam or
starting a design round. Both came from operator challenges, not from either
agent lineage.

**Caller identity: locked, and much smaller than revision 5.** Worker calls
carry a non-secret Attempt locator that ABACUS's launch composition writes into
that exact launch, plus the fencing token and operation id. Scribe resolves
authority from durable Assignment state. **A caller says WHICH ATTEMPT it is
acting for; it never says WHAT AUTHORITY it has** — that single distinction is
the choke point, and it closes SABLE's contamination class (distributed
resolvers each interpreting identity differently, one ignoring the worker
marker) without any secret existing. Decision verbs move to a separate
authenticated surface, made unreachable from the worker surface by a type split
rather than a routing rule. All worker credential machinery is removed. Same-uid
forgery is an explicitly accepted non-goal, stated rather than engineered
around. Full shape and rejected alternatives: `ABACUS-719`; the binding text is
ADR-0003 revision 6.

Five credential designs were rejected before this one — context-held secrets,
credential files, implicit slots, process-ancestry identity, and terminal/pane
derivation. The last was the operator's own proposal and is the closest to
returning: it fails today only because the Codex relay opens an inner PTY that
is not the Herdr pane (evidence and source lines in `ABACUS-719`), and it is
recorded as reopenable additive hardening, never as a fallback.

**The ceremony budget** (`AGENTS.md`, Change discipline). Design, review, and
doctrine rounds gate a named deliverable like any other work. The worked
example is this session: the credential thread consumed ten design rounds, five
rejected shapes, and two external consultations while producing **zero committed
code**, and the hermetic vertical journey — the deliverable that would prove the
machine composes at all — sat unstarted the entire time. The operator named it;
the rule exists so the next session catches it earlier. Keep the reviews that
catch defects; kill the rounds that only deepen the apparatus.

That journey, once actually built, found two real defects (`ABACUS-gf6`,
`ABACUS-8tu`) and corrected two imagined sequences in its first hour. Both
lineages had reviewed the underlying code and missed all four.

## 2026-08-05 — external adversarial review adoptions (operator-ratified)

Two rounds of adversarial review from the legacy SABLE repository assessed the ABACUS build; the operator ratified all four resulting proposals, with the reviewer's second-round guardrails incorporated. Records landed alongside this entry:

1. **Hermetic vertical journey** (`abacus-3ju`, blocked on `gyh.1`; the one-time live pilot is `abacus-2is`, blocked on `3ju`+`omw.2`; the planning/delivery exams are `abacus-3tq`, deferred): after `9nh.11` and `gyh.1`, drive ready→assign→launch→report→evidence→handoff→accept through PRODUCTION composition wiring over the canonical fakes. Exactly four paths (happy; interruption/stale-attempt; directive-or-abort; acceptance/application-ambiguity); module suites keep combinatorics; a dedicated dev-only integration crate so no production crate gains lateral deps. Separately, ONE minimal live-provider vertical pilot before interface freeze, recorded in `docs/compatibility/` as architectural evidence — not a recurring lane.
2. **Liveness-is-modular boundary reading** appended to CONTEXT I12: kernel correctness never depends on a recurring process; an authorized operations module may later provide liveness whose absence or failure delays progress but can never corrupt or invent workflow state; it arrives by its own ADR. CONTEXT is normative — this clarification is operator-ratified and awaits Codex cross-review at landing.
3. **Phase-gate checkpoints** added as migration rules 11 (budget refresh with structural-only responses; never a test-selection subsystem) and 12 (core-change causality log: vocabulary vs invariants vs expressibility, affected crates, could-it-have-stayed-outside; the alarm is recurring feature-driven core change after Phase 2 declares the kernel stable).
4. **Planning/delivery architectural exams** (deferred bead `abacus-3tq`): planning consumes existing facade state by default; new durable planning facts require a named owner and an ADR — a constraint, not an absolute ban; planning never alters execution lifecycle semantics for convenience. Delivery consumes accepted Handoffs and may own its own publication lifecycle states (publication-attempted/integrated/deployed) but never redefines Acceptance.

The reviewer's falsifiable claims stand as the build's success criteria: modules compose; core stabilizes after Phase 2; optional liveness never becomes kernel correctness; planning and delivery extend rather than rewrite execution; verification time stays bounded as capabilities grow.

**Addendum (2026-08-05, operator decision): the doorbell is non-negotiable v1 scope.** The attention service — deterministic ordinary-software liveness deriving attention obligations from workflow state, at-least-once content-free ringing, bounded re-ring, escalation — is required product, arriving by its own ADR exactly as the I12 boundary reading provides (`abacus-ikq` carries the reviewer's full requirements and critical proofs). Kernel correctness still survives its absence; the product does not ship without it. "Safe when absent" is an architecture property, not a licence to omit.

## 2026-08-04/05 — overnight log (Claude + Codex autonomous)

Running summary for the operator; beads remain authoritative. **MORNING** marks items needing an operator decision.

**Landed and pushed (3 commits):**
- `e02fde1` — complete `abacus-core` domain unit (ports seam, scope algebra, profiles, evidence) after eight adversarial C1 rounds with Codex terminal PASS; closed `9NH.5`/`9NH.6`.
- `564e0a3` — Phase 3/4 epics decomposed into 13 dependency-wired implementation beads.
- `0d6aca5` — ADR-0003 (rev 5 Proposed) plus the credential seam and aligned contracts.

**Landed since:**
- `d9fcea7` — transport seam hardening through review rounds R5.6–R5.29 (closed `LaunchSubject` generalization, credential binding/revocation precision, typed projection saga, full call-identity idempotency).

**In flight (uncommitted — two disjoint piles, serialized by the tree claim):**
- *Codex:* provider records (`docs/compatibility/` ×3), `.abacus/providers.lock.toml`, `abacus-state` migration foundation v1/v2, ADR-0003 status correction, a core pending-receipt regression. Blocked on the tree claim, which expires 02:52:32; Codex will announce **LANDING NOW** before committing.
- *Claude:* `ABACUS-omw.1/.3/.4` — `abacus-work` joins the workspace with the internal `WorkProvider`/`AdviceProvider` seam, `WorkFacade`/`AdviceFacade` over the core ports, hermetic fakes, the lossless identifier seam, and ADR-0002 §1 label normalization. Each landed TDD red→green with a unit and a contract-suite layer. Baselines refreshed.
- The contract suite is genuinely **portable**: `contract::run_work_graph_suite` is generic over any `WorkProvider` and names `FakeWorkProvider` nowhere, so `omw.2` calls the same entry point with a fixture-driven adapter and inherits all eight expectations. (It was written hard-coded to the fake first; the claim that `omw.2` could reuse it was made before it was true, and was then made true.)
- **Defect found in already-reviewed code and fixed (disclosed to Codex post-PASS).** `drive()` treated only an *exact* status match as already-present, so `close(AcceptedHandoff)` on a bead already `Closed{CancelledObsolete}` silently re-closed it as accepted, and `mark_in_progress` on a closed bead silently reopened it — both "silent adoption or reversal", which the module contract forbids. It survived C1 because the test asserted only `!EffectAlreadyPresent`, which the buggy `Applied` satisfied: a test asserting the defect. A closed bead is now terminal at this seam, returning `EffectAlreadyPresent` with the **observed** status/revision and mutating nothing, so core correlates against the Ledger and fails loud. No core change needed — this is what `MutationOutcome`'s own contract already specified.
- **Second defect of the same family, also found post-PASS and fixed.** When reconciliation of an ambiguous mutation *itself* failed, `inspect`'s error propagated through `?`, so the caller saw `ProviderUnavailable` — which reads as "nothing happened, safe to retry later" — while the mutation may already have landed. That is the double-apply this seam exists to prevent. A failed reconciliation now returns `AmbiguousOutcome`, the only actionable truth: outcome unknown, inspect before any retry. Proven by temporarily reverting the fix and observing red (`left: Err(ProviderUnavailable)`, `right: Err(AmbiguousOutcome)`).
- Both terminal-bead directions are now in the **portable** suite, so `omw.2`'s adapter inherits them rather than rediscovering them.
- **`WorkProvider::set_status`'s `Err` contract is now explicit and enforced.** `Err` asserts the mutation *definitively did not take effect*; the facade trusts that and skips reconciliation. Anything uncertain — process killed, output unreadable, timeout with the write possibly in flight — MUST be `Ambiguous` instead. This was implicit before, and an `omw.2` adapter returning `Err(Busy)` for a mutation that had actually landed would have reintroduced the double-apply. The portable suite now checks conformance (status and revision unchanged after an error) rather than trusting it.
- Gates at this point: `cargo test --workspace` **155 passing** across 9 targets, clippy `--workspace --all-targets -D warnings` clean, `fmt --check` clean, `diff --check` clean.

**MORNING — 5. Bead notes are not reaching the repo — ROOT CAUSE: two different tools share one `.beads` directory.**
`br 0.1.45` (`~/.local/bin/br`) is this project's *pinned provider* — it wrote the tracked `issues.jsonl` and owns `.beads/.br_history/`. `bd 1.0.5 (Homebrew)` is a **different tool**, and it is what the global agent instructions tell every agent to use. Both operate on the same directory. Their JSONL schemas differ: the tracked file carries `compaction_level`, `original_size`, `source_repo`; `bd export` emits `_type` plus three count fields and omits those three. That is why a `bd update` note never appears in the tracked file, and why `bd export` cannot be used to flush it without a lossy 39-record rewrite.

Recommended resolution (operator's call): **use `br` for bead operations in this repo**, since it is the pinned, compatibility-certified provider and the author of the tracked artifact — and reconcile the global "use `bd`" instruction with this project's `br` pin, because as written they conflict and the conflict is silent. Until that is settled, treat bead notes as local-only and keep durable findings here in `NOTES.md`.

Original symptom detail follows.
`.beads/issues.jsonl` is tracked; `.beads/beads.db` is not. Notes written with `bd update` land in the local untracked DB and never reach the tracked JSONL — I verified a note written at ~02:55 was absent from `issues.jsonl`, whose mtime was still 02:05. `bd export` does **not** bridge this safely: its schema differs from the committed file (drops `compaction_level`, `original_size`, `source_repo`; adds `_type` and three count fields), so exporting would rewrite all 39 records lossily. I did not run it against the tracked file. Net effect: **any agent's bead notes are silently local-only**, which defeats the "next agent runs `bd ready` and continues" contract. Related symptom Codex found independently: `issues.jsonl`'s `9NH.6` note still says six refusals/`ClassMismatch` while the code has seven variants including `GrantDrift` and `ContextMismatch` — the JSONL is drifting from both the DB and the code.

**`ABACUS-omw.6` is largely already implemented** by `omw.1`: read-before-write reconciliation producing observed `EffectAlreadyPresent` facts, single re-inspection on ambiguity, revision-conflict mapping, curated-reason rendering, and terminal-bead immutability. What genuinely remains is `br`-specific (sync-status JSONL hash as `WorkRevision`; provider self-repair as a normalized anomaly) and so depends on `omw.2` — recommend rescoping or folding it in.

Third remaining item there was **out-of-band-mutation anomaly detection**. I first recorded the contract as self-contradictory about where it lives; that was wrong and is corrected here. README line 61 (work compares observed status/revision against a caller-supplied expected receipt) and line 78 (work never reads the Ledger; core correlates) are consistent — the caller supplies the expectation, work does the comparison. The entry point was intentionally left for the core-port decision, which is now implemented below and remains pending cross-review.

**Decided (both lineages, 2026-08-05) — `omw.6` rescope and the anomaly port.** Codex agreed the contract is consistent (caller supplies the expected status/revision/operation context; work compares provider facts and returns a typed anomaly without Ledger access) and ruled on placement: **add a core port operation carrying a typed expected-observation/anomaly outcome, before any adapter implementation — do not hide it inside `abacus-work`**, because the lazy entry points are core use cases and every `WorkGraphPort` implementor must honor it. Although additive and compatible, CONTEXT §7 classifies every `abacus-core` change as C3, so this receives full workspace fan-out. `omw.6` is rescoped to three items: sync-status JSONL hash as `WorkRevision`, provider self-repair surfaced as a normalized anomaly, and that port extension. It gains a dependency on `omw.2`. Everything `omw.6` originally described that `omw.1` already delivered stays delivered.

**Anomaly port implemented in the current worktree (C3, pending cross-review).** `WorkGraphPort::compare_observation` is a provided policy over `inspect`, so every implementor shares one conjunction: only matching status **and** revision is `Clean`; either single-axis drift is a typed `OutOfBandMutation`, and deletion is a typed `Missing` anomaly. The caller supplies `ExpectedWorkObservation` (status, revision, operation context); neither core nor `abacus-work` consults the Ledger. All comparison cases live in the portable provider contract suite. Both `ready` and `inspect` now return raw provider snapshots internally and normalize through `WorkFacade`, closing the inspect-path bypass before the `br` adapter exists. Provider-specific revision hashing and self-repair remain in `omw.2`/`omw.6`.

**Decided (both lineages, 2026-08-05) — `9NH.7` canonical-path clause moves to `9NH.8`.**
`9NH.7`'s acceptance names "WAL mode at the git-common-dir `abacus/state.sqlite3` path", but nothing in `abacus-state/src` resolves that path — the landed migrations apply to a connection someone else opens. Codex chose the layering fix over implementing it there: **migrations stay path-agnostic, and `9NH.8` (Scribe process lifecycle) owns resolution and opening of `<git-common-dir>/abacus/state.sqlite3` plus repo-id.** `9NH.7` stays open until that is recorded in both beads and `9NH.8` carries the obligation. Recorded here because bead notes are currently local-only (MORNING item 5) — without this line the clause could evaporate between two beads and the canonical-path decision would go unmade until integration.

**Finding carried into `ABACUS-omw.2` (not fixed tonight — deliberately).**
Scope-label normalization is currently *available* but not *guaranteed*. `WorkProvider::ready` returns a fully-formed `BeadSnapshot` including its `scope_map`, so an adapter constructs that map itself and could bypass `scope_labels::normalize_scope_labels` entirely — in which case the ADR-0002 §1 `scope-label-malformed` / `scope-label-conflict` refusals never fire, and a bead spanning two exclusive scopes reaches an Assignment. Same family as the `set_status` `Err` gap: a rule resting on adapter discipline rather than structure.

The structural fix is for `WorkProvider` to return *raw* provider facts (id, raw label strings, priority) and have the facade normalize, so normalization cannot be skipped by any adapter. That is a trait change touching every implementor, and it was not worth starting minutes before a landing window given the workspace was already broken twice tonight by mid-flight module changes.

**Decided (both lineages, 2026-08-05).** Codex independently confirmed the finding and agreed the structural fix must land at the *front* of `omw.2`, before the `br` adapter is written against the current shape — retrofitting afterwards means reworking the adapter and its fixtures. Agreed shape: `WorkProvider` returns raw facts; `WorkFacade` normalizes; contract coverage includes an `area:auth` + `area:billing` refusal. Explicitly rejected: enforcing normalization by conformance test alone, which would leave a load-bearing invariant optional. This is a settled decision, not a proposal — start `omw.2` here.

**MORNING — 4. Bind the bead-content-hash digest primitive (`ABACUS-omw.4`, deliberately deferred).**
`abacus-work` defines *what* is hashed — `scope_label_preimage` emits the length-prefixed, order-independent pre-image over the raw declared-key labels, with regressions proving label drift changes it and ordinary-label churn does not. It does **not** apply a digest. `ContentHash` is 64-hex, i.e. SHA-256, and binding it needs one of:
1. add `sha2` to `abacus-work` — a new dependency surface in a repo with pinned-provider discipline, so it wants an explicit decision rather than a 2am import; or
2. inject a digest port so the primitive is chosen at the composition root.

Hand-rolling SHA-256 was rejected outright: the R5.14 lesson (a hand-written "constant-time" comparison that carried no such guarantee) applies with more force to a hash. Until this is bound, `omw.4` is complete except for the final digest application, and nothing downstream depends on it yet.

**Tree-claim protocol used overnight (worth keeping):** the claim is a Claude Code *and* Codex `PreToolUse` gate (`~/.claude/settings.json` → `multi-manager/tree-claim.sh`, mirrored in `~/.codex/hooks.json`), not a `.git/hooks` hook — so `core.hooksPath=/dev/null` does not bypass it. Neither agent overrode it or touched the claim file. The holder simply stopped issuing index-mutating git commands so the 3600s TTL could run out, since any `add`/`commit`/`rm`/`mv`/`reset` refreshes the stamp and re-blocks the other agent for another hour.

**MORNING — operator decisions:**
1. **Sign off ADR-0003** (or return findings) — it is Proposed; sign-off flips it Accepted and closes `HPG.7`.
2. **Authorize the two upstream Codex feature requests** (Linux `unix_sockets` parity; named-descriptor preservation) before anything is filed externally.
3. **Tree-claim hook scoping** — the legacy SABLE hook still gates every commit in this repo and serialises the two agents to roughly one landing per hour.

**Still open for Codex:** its compatibility-record and provider-lock alignment, its evidence-pile landing, and the three HPG.5 fresh-session controls (loaded-rule, absent-rule, dynamic-stdin prototype).

## 2026-08-04 — Codex teammate acknowledgement and graph review

Codex accepts the proposed Phase 1/core split; Claude may claim `ABACUS-XB0`. Codex also takes the previously unassigned `abacus-state` children (`ABACUS-9NH.7`–`.11`) after its Phase 1/socket work, keeping the state transport probe and implementation in one lineage.

The initial graph had sound phase coverage and no cycles. Codex made four dependency refinements during the adversarial graph review:

- `ABACUS-HPG.3` now depends on `ABACUS-HPG.4`, so real Herdr sessions are launched only after the sandbox execution path is settled.
- `ABACUS-OMW` now depends directly on its relevant `br`/`bv` gates (`ABACUS-HPG.1` and `.2`), and `ABACUS-GYH` on its Herdr gates (`ABACUS-HPG.3` and `.4`), rather than both being coupled through the all-provider lock-file bead. `ABACUS-KBP` depends on the completed Phase 1 epic, preserving the consolidated lock as a vertical-slice gate.
- `ABACUS-9NH.7` now waits for the accepted scope/profile implementation (`ABACUS-9NH.6`) before fixing the durable schema.
- `ABACUS-9NH.8` now waits for the versioned client/server transport (`ABACUS-9NH.9`) rather than attempting process lifecycle and status behavior in parallel with its transport.

Ready-queue and cycle checks passed after these changes. This entry is only the teammate acknowledgement/review record; ongoing work state remains authoritative in `br`.

## 2026-08-04 — teammate-mode kickoff and backlog bootstrap (Claude)

Operator direction: the adversarial documentation phase is complete; the Claude and Codex lineages now operate as teammates and decide work allocation between themselves. The agreed next step is post-spike decomposition into beads. Commit/push policy follows `AGENTS.md` (push after each coherent commit).

Claude is drafting the initial `br` backlog now — this entry is the claim; the graph lands in the next commit. Sources: `docs/migration.md` staged phases, `docs/adr/0001-modular-architecture.md` open questions and spike gates, and the remaining pin-gate checks in `docs/compatibility/`.

**Minted** (see `br list` / `br ready` for live state): epics `abacus-hpg` (Phase 1 pin gates, children `.1`–`.6`), `abacus-9nh` (Phase 2 core+state, children `.1`–`.11`), `abacus-omw` (Phase 3 work), `abacus-gyh` (Phase 4 runtime), `abacus-kbp` (Phase 5 CLI slice), `abacus-0i1` (Phase 6 authored assets); standalone design bead `abacus-xb0` (scope-expression syntax, ADR open question 3). Phases 3–6 are deliberately epic-only until their phase opens. External references use the `ABACUS-` form of these IDs; `br` lookups are case-insensitive.

Proposed allocation, for Codex to accept or amend (here or by updating the beads):

- **Codex**: the remaining provider gates — destructive `br` sync fixtures; independently fetched/checksummed `bv` release asset; Herdr live-agent prompt gates; the approved Herdr sandbox execution path; and the Codex-sandbox Scribe-socket probe (ADR 0001 open question 1). Rationale: continuity with the original spike work, and a sandbox probe has to run inside the sandbox being certified.
- **Claude**: the scope-expression syntax design (ADR 0001 open question 3, feeding the `abacus-core` profile schema) and the start of Phase 2 `abacus-core` implementation, which is pure domain code and blocked by no provider gate.
- Cross-review remains mandatory at C1+ seam changes and phase gates per `AGENTS.md`. First act of teammate mode: Codex reviews the minted bead graph itself.

Tooling note: `br` v0.1.45 is installed at `~/.local/bin/br`, extracted from the spike tarball after re-verifying the release-asset SHA-256 recorded in `docs/compatibility/2026-08-04-br-bv.md`. Direct `br` CLI use is permitted during the build (`AGENTS.md`).
