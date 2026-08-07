# ADR-0002: Scope expressions — a label-selector algebra over declared keys

- **Status:** **Partially superseded** (2026-08-07) by ADR-0006. The pinned
  `br` label constraints and normalized `ScopeMap` adapter contract remain
  valid provider/domain facts. Capability grants, check classes, exclusive
  scope enforcement, profile occupancy/activation, grant drift, routing as
  authorization, and Scribe write-time checks are withdrawn from the v1
  target pending the necessity round.
- **Date:** 2026-08-04
- **Decider:** operator (Dylan Delli Colli), on cross-reviewed proposal
- **Companions:** current `CONTEXT.md` I9/I16/I17, ADR-0006,
  `abacus-work/README.md`, and bead `ABACUS-XB0`

> **Supersession boundary.** Sections below are retained as design history and
> as evidence for the provider-compatible `key:value` normalization already
> consumed by `abacus-work`. They no longer authorize building a runtime
> capability/scope system. `ScopeMap` does not fall merely because
> authorization does: it also normalizes work-provider labels. Any broader
> retention must be justified in ADR-0006's necessity round.

## Context

CONTEXT I16/I17 and ADR-0001 §7 fix the *policy*: profiles compose an authority class with explicit capabilities and responsibility scopes; config validation rejects overlapping **exclusive** mutation/decision scopes up front; shared read/observation scopes may overlap; Scribe authorization at write time is the backstop; every decision records the scope it acted under. What remained open (ADR-0001 open question 3) is the *expression*: how a scope is written, and how each of those checks actually evaluates it.

Four constraints from the accepted architecture shape the answer:

1. **Write-time checks must be pure over recorded facts.** Scribe is the authorization backstop, but `abacus-state` never reads the work graph (no lateral module dependencies, ADR-0001 §2), and Scribe never polls providers. Whatever a scope means, Scribe must be able to evaluate "is this subject in this actor's scope" from data already in the request and the Ledger — never from a live graph query.
2. **Exclusive-scope disjointness must be statically decidable.** Config validation rejects overlapping exclusive scopes *before* any runtime exists (I17). The grammar must admit a cheap, sound decision procedure — not a satisfiability search, and not a check that depends on the current contents of the work graph.
3. **Core owns semantics, not file syntax.** `abacus-core` owns "generic capability-ID, grant, and responsibility-scope semantics" and explicitly does not own "profile file syntax or configuration loading". The scope expression is therefore a *domain value* with a canonical textual form that core parses, validates, and evaluates; where that string sits in frontmatter or config files is the composition layer's business.
4. **Topology stays configuration.** Routing a subject to its decision actor must be *derived* from profiles — no stored routing table, no priority or first-match ordering that would make a card set position-sensitive (I16).

And one product constraint: **the smallest useful loop must need zero scoping ceremony.** A single-orchestrator repository must not be forced to invent labels before its first assignment.

## Decision

### 1. Subjects carry reserved `key:value` labels; scopes select over a normalized map

**Provider constraint** (pinned `br` v0.1.45, direct evidence from the HPG.1/br-bv compatibility work): work-graph labels are flat strings restricted to alphanumerics, hyphen, underscore, and colon — `=` is rejected with `VALIDATION_FAILED` — and nothing in the provider prevents one bead from carrying both `area:a` and `area:b`. The design therefore distinguishes the **provider encoding** on beads from the **normalized scope map** the algebra evaluates.

**Encoding.** A scope-relevant label is a provider label of the reserved form `<key>:<value>`, where `<key>` is a declared scope key. A label whose prefix up to the first colon is not a declared key — or that contains no colon — is an ordinary label, invisible to scoping. The repository declares which keys participate:

```toml
# .abacus/config.toml
[scopes]
keys = ["area", "epic"]
```

Only declared keys may appear in scope expressions; an expression using an undeclared key fails validation. When no keys are declared, the only valid scope expression is `*`: ceremony is opt-in, and the single-orchestrator default (Decision §7) needs no labels at all. The expression operators (`=`, `!=`, `=*`, `&`, `|`) exist only in ABACUS-owned cards and config — they never appear in provider labels, so the provider charset constrains *values*, not the algebra.

**Normalization (owned by `abacus-work`, at the seam).** From a bead's raw label set, the work facade selects the declared-key-prefixed labels and parses each as `key:value`. Two distinct, deterministic refusals fire *before any Assignment exists*:

- `scope-label-malformed` — a declared-key label with an empty or invalid value (values are `[a-z0-9_-]+`: provider-representable, no dots, no further colons, no uppercase), or an encoded `key:value` exceeding the provider's 50-character whole-label limit (direct v0.1.45 evidence: 50 accepted, 51 exits 4 `VALIDATION_FAILED`; recorded in the HPG.1 fixture set);
- `scope-label-conflict` — two declared-key labels binding the same key to different values.

The surviving result is the **normalized scope map**: at most one value per declared key. Single-valuedness is load-bearing — it is the precondition that makes the disjointness procedure in Decision §4 sound. With multi-valued keys, a bead labeled both `area:a` and `area:b` would match two provably-disjoint exclusive scopes at once. A bead spanning two exclusive partitions is a decomposition defect surfaced for explicit re-labeling, never auto-resolved by picking a winner.

**Snapshot semantics.** Scope membership is evaluated against *recorded* facts, never live graph state:

- Assignment creation snapshots the bead's **normalized scope map** into the Assignment, alongside the bead-content hash that already binds it (ADR-0001 §9.1). The bead-content hash **covers the raw declared-key-prefixed label strings**; since the normalized map derives purely from those strings, a change to either the raw labels or the derived map fails the existing hash recheck at Acceptance — label drift gets the same refusal and explicit re-evaluation path as any other contract drift, with no new machinery.
- Attempts, Evidence, Handoffs, and Signals about an Assignment inherit its snapshot; they never re-resolve labels.
- Scribe evaluates scope checks against these recorded attributes plus the profile grants in force (profile activation is an audited Ledger event carrying the profile content hash).

This is what makes constraint 1 hold: the work graph is consulted exactly once per subject, by the use-case layer that is allowed to read it, at the moment the subject enters the Ledger.

### 2. Grammar, satisfiability, and canonical form

A scope expression is a disjunction of selectors; a selector is a conjunction of atoms. Canonical textual form, owned and parsed by `abacus-core`:

```text
scope    := "*" | selector ( "|" selector )*
selector := atom ( "&" atom )*
atom     := key "=" value      # key present with exactly this value
          | key "!=" value     # key absent, or present with a different value
          | key "=*"           # key present, any value
key      := [a-z][a-z0-9-]*    # must be a declared scope key
value    := [a-z0-9_-]+        # provider-representable: no dots, no colons
```

Examples:

```text
*
area=frontend
area=frontend | area=design
epic=abacus-9nh & area!=docs
area=*
```

Evaluation over a subject's **normalized scope map** (single value per key, Decision §1) is a pure core function. Atom semantics, including the absent-key row, are fixed:

| Atom | Key absent | Key = `v` | Key = other |
|---|---|---|---|
| `k=v` | false | true | false |
| `k!=v` | **true** | false | true |
| `k=*` | false | true | true |

`k!=v` matching absent keys is deliberate: it makes "everything except the frontend slice" (`area!=frontend`) a valid catch-all-rest that still covers unlabeled beads, so two-orchestrator partitions are expressible without labeling the whole backlog.

There is no nesting, no negation of whole selectors, and no other operator. Two levels (OR of ANDs) is complete for the partitioning this system needs and keeps the disjointness procedure trivial.

**Satisfiability.** Validation rejects any selector that is unsatisfiable as an authored defect, never silently ignores it. Exactly two unsatisfiable-conjunction forms exist and both are rejected: `k=a & k=b` (same key, differing literals) and `k=a & k!=a`. `k=*` contradicts nothing.

**Canonical serialization.** Parsing is whitespace-tolerant; comparison, hashing, and every durable record use one canonical form: within a selector, atoms are deduplicated and sorted lexicographically by (key, operator, value); selectors are then themselves deduplicated and sorted lexicographically; separators are exactly `" & "` and `" | "`; `*` stands alone. Two expressions are equal iff their canonical forms are byte-equal. Authored cards may format freely — the card's own content hash covers its raw bytes as always, while scope values recorded on decisions and grants are canonical.

**Bounds.** Declared keys are at most 15 characters and values at most 34, so every encoded `key:value` label fits inside the provider's 50-character whole-label limit including the colon (keys are checked at config load, values at normalization). An expression carries at most 8 selectors and a selector at most 8 atoms — ample for any partition this system needs, while keeping every pairwise disjointness/containment product trivially small and every durable canonical string bounded.

### 3. Grants and the three check classes

A capability descriptor (module-declared, registry-supplied by the composition root, per the `abacus-core` contract) declares its **check class**. A role card's grant is `capability → scope expression`; a card cannot reclassify a capability — the class is a property of what the capability *does*, fixed by its owning module.

| Class | Meaning | Authorization at call time | Overlap across profiles |
|---|---|---|---|
| `exclusive` | Responsibility-*owning* authority mutations and decisions: assignment creation, Directives, acceptance, rejection, retry, reclamation, arbitration | Routed by scope (§7); Scribe backstop checks the actor's current grant covers the subject | **Rejected** at config validation (§4) |
| `fenced` | Attempt-bound worker workflow mutations: Reports, Evidence, Handoff submission, lease renewal | The Assignment/Attempt binding plus the current lease fencing token — never scope routing. These are the core lease/fencing rules that already exist; scope plays no per-call part | Free — ten workers with `*` grants are the normal case |
| `shared` | Reads and observation | Grant must merely exist and cover the subject | Free |

The `fenced` class is what makes worker fan-in coherent: a worker's mutations are authorized by *which Attempt it is bound to*, not by partitioning the subject space among workers. A fenced capability's scope expression participates exactly once — at Assignment creation, where the use case checks the candidate worker profile's **attempt-lifecycle bundle scope** (§8) covers the bead's normalized map (may this worker be bound to this subject). Overlap among worker grants at that site is expected and correct; the default worker card grants `*`.

### 4. Disjointness: overlap unless contradiction

Config validation must reject overlapping exclusive scopes (I17). The decision procedure:

- Two **selectors** are disjoint iff some key appears in both with jointly unsatisfiable atoms. Exactly two contradiction forms exist: `k=a` vs `k=b` (differing literals), and `k=a` vs `k!=a`.
- Two **scope expressions** are disjoint iff every selector pair across them is disjoint. `*` overlaps everything.
- Anything not provably disjoint is treated as overlapping.

This is conservative in the safe direction: it can reject a cleverly-disjoint configuration (the fix is to rewrite it with explicit literals or negations), but it can never accept an overlapping one. Because the procedure is sound over *all possible* normalized maps, post-validation uniqueness is unconditional: no subject, present or future, can match two profiles' exclusive scopes for the same capability — regardless of what labels later appear in the graph. Soundness leans on §1's single-valued normalization; multi-valued subjects never reach evaluation.

Validation reports the exact conflicting pair: both profile names, the capability, and the two selectors that could not be proven disjoint (`scope-conflict`).

### 5. Subject projection and containment

The binding contracts admit exactly four subject shapes — Bead, Assignment, Attempt, and responsibility scope (CONTEXT §2, `abacus-core` contract) — and this ADR adds no fifth. Each projects to something the pure checks can evaluate:

| Subject | Projects to | Check against a grant |
|---|---|---|
| Bead | Its normalized scope map, read through the facade at creation time and **recorded with the Signal/Assignment** | Membership (§2 semantics) |
| Assignment / Attempt | The Assignment's recorded snapshot, inherited | Membership |
| Responsibility scope (e.g. an authority-transfer Request) | The expression itself, canonical | **Containment** (below) |

A Request's recipient is **addressing, not subject**: the resolved target ActorId is recorded on the Request for routing (§7), while its workflow subject remains one of the four shapes above — an arbitration Request about a bead carries subject = that bead, recipient = the routed actor. The sender needs the request capability for the subject; the *resolution* is checked under the responder's own authority at decision time.

**Non-work-scoped targets.** Some capability targets have no work-scope projection at all — repository-level observation, runtime/actor operations like the watchdog's `runtime:observe`. A capability descriptor declares whether its targets project to scope maps; for those that do not, the only valid grant scope in v1 is `*`, enforced at config load. Partitioning non-work domains is out of scope until a real need names the projection.

**Conservative containment.** Grant expression `G` contains subject expression `S` iff every selector of `S` is provably contained in at least one selector of `G`. Selector `s` is contained in selector `t` iff every atom of `t` is implied by some atom of `s`, using the fixed implication table: `k=v ⇒ k=v`; `k=v ⇒ k=*`; `k=v ⇒ k!=w` for `w≠v`; `k!=v ⇒ k!=v`; `k=* ⇒ k=*`. `*` contains everything; only `*` contains `*`. Like §4 this is conservative in the safe direction: it under-approximates true containment, so a scope-subject action not provably inside the sender's grant is refused (`scope-unauthorized`) even if a cleverer prover would admit it; the remedy is rewriting one expression.

### 6. The evaluation sites

| Site | Evaluator | Checks | On failure |
|---|---|---|---|
| Card/config load | core semantics, invoked by composition | grammar, declared keys, satisfiable selectors, canonicalization, exclusive disjointness (§4), bundle coherence (§8) | `scope-conflict` / `scope-bundle-incoherent` / schema error; configuration refused |
| Assignment/dispatch | core use case, via ports, against the current bead read | label normalization (§1); acting orchestrator's exclusive grant covers the map; candidate worker's attempt-lifecycle bundle scope (§8) covers the map | `scope-label-malformed` / `scope-label-conflict` / `scope-unauthorized`; no Assignment created |
| Signal creation | core use case + Scribe | subject projection (§5); membership or containment per subject shape | `scope-unauthorized` |
| Scribe write time (backstop) | pure core check inside Scribe's transaction, against recorded snapshot + grants in force | every fenced decision/mutation, per its check class (§3) | distinct `scope-unauthorized` (or lease/fencing refusal for `fenced`-class calls), audited (I17) |
| Acceptance | existing bead-content-hash recheck | raw declared-key labels (hence the derived map) unchanged since assignment | existing hash-mismatch refusal; explicit re-evaluation |

### 7. Routing resolves to one active actor; drift is loud

**Activation binding.** Singleton occupancy applies exactly where authority-uniqueness needs it: a profile holding **any exclusive grant** may be occupied by at most one active actor — profile activation is already an audited Ledger event, and Scribe refuses a second activation (`profile-occupied`). Profiles holding only `fenced`/`shared` grants are freely multi-occupied: the default worker card occupied by ten concurrent workers is the normal case, and per-actor identity enters the record where it matters — the Assignment binds the concrete worker ActorId at creation. Deactivation is likewise explicit and audited. Combined with §4's cross-profile disjointness, routing of exclusive capabilities is actor-unique by construction:

`route(capability, subject) → profile` (unique by §4) `→ its single active actor` (unique by activation binding).

No active actor for the routed profile is a first-class loud outcome (`unroutable-subject`) surfaced to the operator — never a silent default, never a fallback actor. Requests store their resolved target actor at creation; the facade may *compute* a route as a convenience, but the record carries the concrete decision actor, as I17 already requires.

**Grant drift.** An in-flight Assignment names its exact decision actor and the profile content hash in force at creation; that recorded hash is the *historical authorization fact*, never a continuing license. Every **new** decision on the Assignment requires both identities to hold at decision time: the acting actor must be the Assignment's recorded decision actor, **and** that actor's currently active grant must still cover the subject. If a card edit or re-activation shrinks the grant, the actor's next decision fails loudly (`scope-unauthorized`); the remedies are the explicit, fenced authority-transfer Request that already exists, or restoring the configuration. Nothing continues silently under revoked authority, and no other orchestrator can seize the Assignment merely by matching scope — transfer is always an explicit fenced decision (I17, core invariant 2).

The single-orchestrator default falls out with no special case: one orchestrator profile granted its capabilities at scope `*`, occupied by one active actor, is trivially valid, routes everything, and requires no declared keys and no bead labels.

### 8. Lifecycle bundles are granted coherently

Per-capability scopes checked only within one capability admit dead configurations on both sides of the trust divide. Config validation closes both with the same rule: a profile granting *any* member of a bundle must grant *all* of them **at the identical canonical scope**; violations are rejected at load (`scope-bundle-incoherent`).

- **Assignment-lifecycle bundle** (exclusive class): every exact-decision lifecycle capability — assignment creation, Directive issuance, acceptance, rejection, retry, reclamation, **Attempt revocation, and Assignment cancellation**. Bundle membership is **descriptor-driven**: a capability descriptor declares its bundle, and core validates coherence over whatever the registry declares — the list here is the current canonical membership, not a hard-coded core enum. Without the bundle, profile A could hold `assign` over a subject while only profile B holds `accept` — yet an Assignment names one exact decision actor, so A's assignment could never be decided; omitting cancellation would likewise strand an obsolete Assignment whose creator can no longer end it. With it, the routed assigner is by construction a valid decision actor for everything the Assignment's lifecycle needs, including ending it. Lease-expiry *evaluation* remains non-grantable — expiry is a time-derived fact, and the fenced actor action it enables is reclamation, which is in the bundle. The decision resolving an **authority-transfer** Request is also in-bundle — it is a decision on the Assignment, made by its current exact decision actor, and is valid only if the recipient's currently active bundle grant covers the subject; cross-orchestrator **arbitration** stays a deliberately separate exclusive capability outside the bundle (non-work-scoped, so `*`-only in v1 per §5).
- **Attempt-lifecycle bundle** (fenced class): Report submission, Evidence recording, Handoff submission, lease renewal. Without it, a worker profile granting Reports at `area=a` but Handoffs at `area=b` produces an Attempt that can start but never complete. With it, the bundle has one coherent scope, and Assignment creation checks exactly that scope when binding a candidate worker (§3).

**Work-status application is not a grantable capability.** Closing the bead in `br` after Acceptance (`accepted_handoff`) is the *projection* of the committed Acceptance decision, executed under that decision's recorded operation identity per the acceptance saga (ADR-0001 §3) — it is internal application authority, not a card-grantable verb. It therefore cannot be configured apart from `accept`, and no configuration can produce an acceptance without a usable close path.

Modeling separate assigner and decision authorities is a genuinely larger domain change (Assignment would need to name two actors, and CONTEXT/core invariants would change); it is explicitly out of scope here and requires its own ADR if ever wanted.

### 9. Subtree scoping is a labeling convention, not a mechanism

"Everything under this epic" is expressed by stamping children with an `epic:<id>` label at decomposition time and scoping over it (`epic=abacus-9nh`). It is deliberately **not** a live ancestry query: parentage lives in the work graph, drifts as the graph evolves, and is unreadable from Scribe (constraint 1). Denormalizing ancestry into a label makes the fact snapshot-able, hash-bound, and pure to evaluate. Because scope values exclude dots, hierarchical child IDs (`abacus-hpg.1`) are not valid scope values — stamp the dotless root-epic ID, which is also the only ancestry fact worth partitioning on. Keeping stamped labels consistent with actual graph shape is ordinary backlog hygiene, and a natural job for the deferred planning machinery (ADR-0001 §11) — it is never a correctness input to authorization, which only ever consults the recorded snapshot.

## Alternatives considered

- **Live bead-subtree scopes** ("this orchestrator owns the subtree under ABACUS-X"). Rejected: membership depends on current graph ancestry, which Scribe cannot read (module boundaries) and which mutates under the Assignment; disjointness would be a property of graph state, not configuration, so I17's up-front validation would be impossible. The labeling convention (Decision §9) captures the use case without the coupling.
- **Multi-valued scope keys** (a bead in several areas at once). Rejected: exclusive partitioning is the point, and a subject matching two owners' exclusive scopes defeats config-time disjointness. Refusing duplicates at normalization (§1) forces the ownership question back to decomposition, where it belongs.
- **Priority / specificity / first-match resolution** between overlapping scopes. Rejected: order-dependence turns a declarative card set into a position-sensitive program, hides routing policy inside evaluation order, and contradicts I17's decision to *reject* exclusive overlap rather than resolve it.
- **Glob/regex over bead IDs.** Rejected: IDs are opaque identifiers; encoding routing meaning into them recreates name-keyed authority (the legacy failure ADR-0001 §7 exists to kill) one level down.
- **Full boolean algebra with nesting.** Rejected: expressive power nobody has asked for, at the cost of disjointness and containment checks that become real SAT. Two-level selector algebra keeps validation explainable in one sentence.
- **A stored routing table** mapping subjects to actors. Rejected: a second mutable authority for "who decides", drifting from the cards; derivation from profiles keeps topology config-only (I16).
- **Separate assigner and decision authorities per subject.** Deferred, not adopted (§8): it changes the Assignment's shape and core invariants; the lifecycle bundle keeps v1 coherent without it.
- **Scope-partitioned worker authorization.** Rejected (§3): workers are authorized by Attempt binding and lease fencing, which already exist and already scale to any fan-in; partitioning the subject space among workers would make ordinary overlapping worker pools invalid configuration.

## Consequences

**Positive**

- Every check I17 promises is now concretely evaluable: static disjointness and bundle coherence at config time, pure membership/containment at write time, drift detection at acceptance — all from one small grammar with fixed semantics.
- Worker fan-in scales structurally: ten or a hundred workers hold overlapping `fenced` grants legally, and their call authorization rides the lease/fencing rules core already owns.
- Topology changes remain card/config-only: repartitioning two orchestrators is editing two scope strings and re-running validation; no code, no schema change.
- The smallest loop pays nothing: no declared keys, one `*` grant, one active actor.
- No new machinery classes: snapshots ride the Assignment record that exists, drift detection rides the bead-content hash that exists, activation rides the audited Ledger event that exists, audit rides the decision records that exist.

**Negative / accepted**

- The conservative disjointness and containment checks can reject configurations and scope-subject asks that are valid in practice. Accepted: the rewrite burden is small and explicit, and the alternative is admitting overlap or over-broad authority by accident.
- Scope values are constrained to the provider-representable charset (no dots), so hierarchical child IDs cannot be scope values. Accepted: root-epic stamping covers the real partitioning need (§9).
- A bead genuinely spanning two exclusive areas must be re-labeled or split before assignment (`scope-label-conflict`). Accepted: that is a decomposition question forced to the surface, not a defect of the mechanism.
- Label vocabulary requires discipline; declared keys bound the blast radius of typos but cannot make labels *meaningful*. Accepted: meaning-assignment is the orchestrator's decomposition job, assisted later by planning tooling.
- Stamped `epic:` labels can drift from actual graph ancestry between hygiene passes. Accepted: authorization never depends on ancestry, only on the recorded snapshot; drift affects routing convenience, not correctness, and surfaces as `unroutable-subject` or a hash-mismatch refusal — both loud.
- The activation-binding rule means an exclusive-grant profile cannot be occupied by two live sessions for throughput. Accepted: that is exactly the authority-uniqueness the design wants; parallel capacity comes from more orchestrator profiles partitioning scope, or from more workers multi-occupying fenced-only profiles — never from shared exclusive authority.

**Cost class.** This ADR is design only. Implementation lands in three owning modules: the expression value type, matching, containment, disjointness, canonicalization, bounds, and grant/bundle validation in `abacus-core` (`ABACUS-9NH.6`, which this ADR unblocks); the write-time backstop, activation binding, and snapshot columns in `abacus-state` (`ABACUS-9NH.7`/`.10`); and the reserved-label normalization, charset/length refusals, and raw-declared-label inclusion in the bead-content hash in `abacus-work` — new seam behavior this ADR assigns to that module, to be carried as an explicit child bead when the Phase 3 epic (`ABACUS-OMW`) decomposes. The companion `docs/architecture.md` profile example and exclusivity paragraph are amended by this ADR's revision. No new module, port, or record class.
