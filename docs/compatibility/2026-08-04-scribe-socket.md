# Scribe socket sandbox compatibility probe

Date: 2026-08-04  
Host: Linux x86_64  
Codex CLI: v0.146.0 (transport probes); v0.146.1 (faithful HPG.5 carriage)\
Claude Code: v2.1.221  
Status: Claude direct socket access passes; Codex v0.146.0 direct socket access is denied on Linux; command-scoped host-relay feasibility passes from direct and Herdr-launched Codex sessions; the production-rule, absent-rule, dynamic-composer, and faithful runtime-carriage HPG.5 controls pass; ADR-0003 revision 5 remains Proposed pending operator sign-off

## Question

Can an agent running under the ordinary workspace-write sandbox connect to Scribe at:

```text
$XDG_RUNTIME_DIR/abacus/<repo-id>.sock
```

The probe used disposable `nc` listeners rather than a Scribe implementation. It tested the transport boundary only; it did not create Ledger state or alter agent configuration.

## Codex observations

The active Codex environment reported `XDG_RUNTIME_DIR=/run/user/1000/`. The ABACUS runtime directory did not exist before the probe and was created with mode `0700` only for the host-side listener.

| Probe | Listener | Client | Result |
|---|---|---|---|
| Unix listener under writable `/tmp` | sandbox | — | refused: `Operation not permitted` |
| Unix socket under `/tmp` | approved host access | ordinary sandbox | refused: `Operation not permitted` |
| Unix socket under `$XDG_RUNTIME_DIR/abacus/` | approved host access | ordinary sandbox | refused: `Operation not permitted` |
| Same XDG socket | approved host access | approved host access | success; payload arrived intact |
| Loopback TCP | approved host access | ordinary sandbox | connection failed |
| Same loopback listener | approved host access | approved host access | success; payload arrived intact |

The positive host controls show that the listeners and paths were valid. The `/tmp` and loopback controls show that changing the socket location or substituting localhost TCP does not solve this sandbox boundary.

Every disposable listener was stopped. The exact socket nodes and the runtime directory created by the probe were removed; no global Claude, Codex, shell, hook, or provider configuration changed.

## Exact Codex permission-profile validation

The current official Codex manual documents allowlist-first Unix-socket access through a permission profile:

```toml
default_permissions = "abacus"

[features.network_proxy]
enabled = true

[permissions.abacus]
extends = ":workspace"

[permissions.abacus.network]
enabled = true

[permissions.abacus.network.unix_sockets]
"/run/user/1000/abacus/<repo-id>.sock" = "allow"
```

The exact path must be computed after repository initialization. ABACUS must not request `dangerously_allow_all_unix_sockets`, broad loopback/private-network access, or danger-full-access.

A disposable host listener was created at the exact XDG path. A fresh Codex v0.146.0 session was then launched in a disposable Git repository with the profile above supplied as command-line configuration overrides, `approval_policy = "never"`, hooks disabled, and no legacy `--sandbox` option. Its `/permissions` screen reported `abacus (current) Configured permission profile`, proving that the permission-profile path won over the legacy sandbox path.

The first fresh agent was discarded after its instructed command contained an accidental extra punctuation argument; the command was not retried in that session. A second fresh agent ran the corrected one-shot client:

```sh
printf '%s\n' HPG5_CODEX_EXACT_SOCKET_OK | nc -q 0 -U /run/user/1000/abacus/abacus-hpg5-codex-r2.sock
```

It failed with `Operation not permitted`, exit status 1, and the host listener observed no connection or payload. A separate host-launched `codex sandbox -P abacus` diagnostic under the identical profile could `stat` the exact socket successfully but the same one-shot `nc -U` connection also failed with `Operation not permitted`. Path visibility and profile selection therefore were not the failure.

The installed release's source tag `rust-v0.146.0` resolved to commit `e363b08c9175ac1cbe5893615dd2cb9ddf95043b`. Its contracts explain the result:

- the Codex v0.146.0 network-proxy contract says `x-unix-socket` proxying is macOS-only and other platforms reject Unix-socket requests;
- the Linux sandbox contract says managed proxy mode applies seccomp after its bridge starts and blocks new `AF_UNIX` and `socketpair` creation for the user command.

An ordinary Scribe client opens `AF_UNIX` directly, so an exact `permissions.<profile>.network.unix_sockets` entry cannot make that client reachable under the installed Codex Linux runtime. This is a provider capability gap, not a missing filesystem grant or malformed ABACUS configuration.

References: [official Codex manual](https://developers.openai.com/codex/codex-manual.md), sections “Permissions / Unix sockets” and “Agent approvals & security / Network isolation”; [Codex v0.146.0 network-proxy contract](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/network-proxy/README.md); [Codex v0.146.0 Linux sandbox contract](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/linux-sandbox/README.md).

Cleanup: both native Codex sessions exited, the host listener stopped without receiving a payload, the named Herdr session was stopped and deleted, and the disposable Git repository, socket node, and probe-created runtime directory were permanently removed after exact-path checks. No global Claude, Codex, shell, hook, or provider configuration changed.

## Claude observations

The reciprocal probe ran from the live Claude Code v2.1.221 session in this repository, using `python3` socket scripts issued through the session's ordinary Bash tool mode. The "approved host access" row used the session's explicit sandbox-disable option for the listener only, matching the deployment shape (Scribe as an ordinary user process, agent as sandboxed client). `XDG_RUNTIME_DIR=/run/user/1000/`; the `abacus/` runtime directory did not exist before the probe.

| Probe | Listener | Client | Result |
|---|---|---|---|
| Unix bind+listen+connect+payload under `/tmp` | ordinary session mode | ordinary session mode | success |
| Directory create `0700` + same probe at `$XDG_RUNTIME_DIR/abacus/` | ordinary session mode | ordinary session mode | success |
| Unix socket at `$XDG_RUNTIME_DIR/abacus/` | approved host access | ordinary session mode | success; payload arrived intact |
| Loopback TCP | ordinary session mode | ordinary session mode | success |

The operative Claude session therefore needs no additional grant to reach the proposed Scribe socket, including the cross-boundary direction that matters for deployment.

Scope caveat, mirroring the Codex fresh-session caveat in the opposite direction: Claude Code sandbox behavior is configuration-dependent, and this session's operative profile is permissive (it also allows user-home writes). This result certifies the current profile on this host, not every Claude configuration. If the operator later tightens the Claude sandbox, the ordinary-mode rows must be re-run before relying on socket reachability, and a denied result would put Claude in the same exact-grant posture as Codex.

Cleanup: every probe socket was unlinked by its script and the probe-created `$XDG_RUNTIME_DIR/abacus/` directory was removed after verifying it was empty. No global Claude, Codex, shell, hook, or provider configuration changed.

## Pre-connected descriptor feasibility

One bounded follow-up tested an ADR candidate without treating it as the transport decision. A host harness connected an `AF_UNIX` stream to a disposable listener, duplicated the client endpoint to inheritable file descriptor 9, and launched `codex sandbox` under the same exact `abacus` profile while explicitly preserving that descriptor. The inner sandboxed shell created no socket; it only wrote `HPG7_INHERITED_FD_OK` to descriptor 9. The sandbox exited 0 and the host peer received the exact marker.

A second disposable probe added the missing Codex-agent layer. A fresh ephemeral Codex v0.146.0 process inherited the same connected descriptor and was instructed by its isolated `AGENTS.md` to run exactly one descriptor write, without retry. The model issued that command once, but its command sandbox returned `zsh:1: 9: bad file descriptor`, exit status 1. The host peer received no payload and the temporary Git root remained exactly `?? AGENTS.md`.

The pair of probes proves that Linux seccomp blocks socket creation rather than reads and writes on an already-connected descriptor, but Codex v0.146.0 does not preserve that descriptor from the agent process into model-issued commands. The pre-connected-descriptor candidate is therefore unavailable without an explicit upstream Codex descriptor-passthrough feature. Such a feature would still need analysis of descriptor exposure to every child process (including project-supplied commands), concurrent framing and reply ownership, and Scribe crash/restart behavior: a stale connected descriptor cannot reconnect itself, while a reconnecting helper risks becoming the forbidden second resident bridge.

Cleanup: both probes closed their accepted/client/listener descriptors and removed their socket/private temporary directory. Absence of leftover socket directories was verified. The ephemeral agent retained no native session history; its temporary Git repository still contained only Git metadata and the original `AGENTS.md` before the repository and both disposable harness files were permanently removed.

## Command-scoped host relay feasibility

A second bounded follow-up tested ADR-0003's primary candidate without treating the disposable program as the Scribe client design. A one-shot host listener bound the exact `0600` socket `/run/user/1000/abacus/abacus-hpg7-relay-r1.sock`. A fixed-function client at `/run/user/1000/abacus-hpg7-relay-r1/abacus-scribe-probe` accepted only the literal `scribe-rpc` subcommand, connected to that hard-coded socket, sent one four-byte big-endian length plus JSON request frame, required the correlated framed response, and used three-second send/receive timeouts. It accepted no socket, command, project path, or payload argument and used `SOCK_CLOEXEC`.

The host client directory and binary were both mode `0555`, outside the fresh agents' writable roots. GCC `13.3.0` produced the 16,704-byte binary. Artifact identities were:

- client source: `bc4ac504b3eb7a6369e39b0d8c2c18392192bcb042a03d0db457f631903e999f`;
- listener source: `becab9f662454a1e4999f16a008d9af57f6ef488462e2b88e77804c0c344de66`;
- disposable execpolicy rule: `9e1025e94b7737fdb1d1df4ed562fd54a48d43115bb1123c590f6d904a0a6ab6`; and
- compiled client: `223669419f23ed419418dcf7c89b6ce646432e89cf64a0e5822dcdfe11f7e69f`.

The uninstalled rule allowed the absolute client path plus `scribe-rpc`; the missing or wrong subcommand and a different path did not match. Codex prefix rules intentionally match prefixes, so the same rule also matched trailing arguments. The client therefore remains the narrower boundary: an extra argument returned usage error 64 without opening a socket. A production relay must likewise reject every unsupported flag/argument; arbitrary request material cannot be smuggled into an allowlisted argv tail.

### Fresh Codex path

A fresh ephemeral Codex v0.146.0 `exec` session used an isolated Git root, ignored user configuration and command rules, and reported `workspace-write`, `on-request`, automatic approval review, and model `gpt-5.6-sol`. It was instructed to issue exactly one host-escalated invocation and never retry.

- With no listener, the client failed loudly once with `connect failed: No such file or directory`, exit 70. There was no alternate transport or retry.
- With the listener present, automatic review allowed the exact command, the client printed `HPG7_RELAY_OK`, and it exited 0. The listener decoded exactly `{"version":1,"request_id":"hpg7-r1","actor_id":"probe-codex","operation":"transport-probe"}` (91 bytes) and returned exactly `{"version":1,"request_id":"hpg7-r1","outcome":"probe-ok"}` (57 bytes). The listener unlinked its own socket.

This is the full agent/exec path that rejected the inherited descriptor: unlike descriptor inheritance, a narrowly approved host command survives that layer and can reach the user-only Scribe socket.

### Herdr-launched Codex path

The clean certification run used pinned Herdr v0.8.0/protocol 19 in disposable namespace `abacus-spike-hpg7-relay-herdr-r2`. It launched Codex v0.146.0 in the isolated repository through `agent start`, with `workspace-write`, `on-request`, automatic review, and lifecycle hooks disabled. Herdr reported pane `w1:p1` and terminal generation `term_658445dbba13e1`. Its high-level `agent prompt --wait` submitted the same one-shot instruction; automatic review approved the exact host command, the worker printed `HPG7_RELAY_OK`, and the bounded wait returned normally at `idle`. The independent listener observed the same exact 91-byte request and 57-byte response frames and removed the socket.

An earlier exploratory namespace (`...-r1`) proved the same payload but inherited an inert global `SessionStart` hook and used a wait set that omitted the one-shot `done` state; it was discarded rather than counted. The prompt was not retried. Both named namespaces were stopped and deleted. The post-test Herdr manifest again contained only the pre-existing stopped `default` session.

### Scope of the result and cleanup

This proves feasibility of the command-approval carriage only. The disposable client hard-coded its request, so the probe did not exercise the production `exec`-session plus runtime-supplied `write_stdin` path, actor authentication/authorization, credential lifecycle, or the separate runtime launch carriage. It also does not turn automatic review into deployment policy. ADR-0003 revision 5 proposes an operator-installed exact absolute executable/subcommand rule, a closed client surface that remains safe for every prefix-matching argv, one bounded typed-JSON request line on stdin with private host-side framing, and fail-closed behavior when that environment policy is absent. Those unproven portions remain explicit HPG.5 controls below.

After recording the evidence, both Codex sessions had ended, both one-shot listeners had exited, both sockets were absent, both Herdr namespaces were deleted, and the disposable Git root still contained only its original harness files plus the two listener result records. The client binary/directory, probe-created empty `$XDG_RUNTIME_DIR/abacus/` directory, and the entire temporary Git root were then removed by exact path. No global Claude, Codex, shell, hook, rule, permission-profile, or provider configuration changed.

## Proposed ADR-0003 revision-5 transport decision

ADR-0003 revision 5 proposes one versioned Scribe protocol on two explicitly configured carriages: direct UDS where the sandbox permits it (the operative Claude profile), and a per-call operator-owned `scribe-rpc` composer for Codex on Linux. Carriage selection is injected; a client never probes one and falls back to the other. The revision remains Proposed until its blocking validation gates and operator sign-off are complete.

The `scribe-rpc` composer accepts exactly its literal two-token invocation and no trailing argument or flag. The agent obtains one live exec session, sends one bounded newline-terminated public typed-JSON command envelope through one `write_stdin`, receives one bounded typed stdout response, and the composer exits under a fixed deadline. It is not a byte relay: for credential-creating openings it generates or accepts explicitly replayed provisioning, composes the distinct internal Scribe request, and privately adds/removes Scribe's four-byte framing. There is one request per process and one connection per request; batching is forbidden in v1. A bounded validated `repo-id` is request data, while the runtime base and carriage are operator-injected configuration; arbitrary socket paths are unrepresentable.

Launch is a separate approved two-token `abacus runtime-rpc` use-case carriage. Its bounded public request contains the authenticated requester, the closed launch-subject selector (worker Attempt or actor activation), that subject's bound transient credential/replay identity, operation identity, and deadline—but no executable, argv, cwd, environment, Envelope, socket path, or caller-asserted authority. Host-side composition authorizes `runtime:launch` before project discovery, loads the persisted Envelope/subject facts, verifies the subject credential through Scribe, resolves execution material only from operator-owned allowlisted configuration, and then constructs the internal `LaunchSpec`. The host runtime uses Herdr's pinned socket `agent.prompt` JSON request only to carry initial Envelope plus secret without argv/environment exposure; this is not an agent-facing Herdr channel and does not make `runtime-rpc` a generic Herdr proxy.

Transport possession is not authority. Scribe authenticates the actor credential and its binding before scope authorization on both carriages. Initial trust is established only through a one-shot pre-listen operator channel; there is no standalone/general agent enrolment verb. `AssignmentOpening` and `AttemptOpening` atomically bind worker provisioning, while `ActivationOpening` carries activation provisioning for the closed operator/rotation cases. Plaintext credentials are transient launch secrets and are excluded from the persisted Envelope, Ledger facts other than their digest/metadata, ABACUS logs, and audit. They do ride model prompts/tool stdin and may persist in provider-owned native transcripts; that accepted residual is bounded by revocation at Attempt end, deactivation, or rotation.

The decision rejects loopback TCP, workspace request files, polling, a broad Codex grant, broad host-side `abacus` CLI execution, and a second resident bridge:

- loopback was denied by the same sandbox and weakens local peer identity;
- workspace request files would violate fail-loud transactional calls and invite forbidden watcher/retry machinery;
- broadly host-executing the future `abacus` CLI is unsafe because some supported commands intentionally execute project-supplied verification commands.

Repository initialization may print the exact operator-applied installation/rule surface, but must never edit global agent configuration. The two upstream Codex improvements remain useful future avenues, not v1 dependencies: Linux Unix-socket allowlist parity and named-descriptor preservation through model-issued commands. Either would require a provider repin and this compatibility lane before changing carriage selection.

## HPG.5 production-rule and faithful-carriage validation (2026-08-05)

The remaining technical gates ran as disposable prototypes. No probe code is
production ABACUS. The full runtime leg used a second Herdr v0.8.0/protocol-19
server with isolated XDG roots and sockets, an initially empty session, and a
disposable Git repository. The dispatcher hard-coded only those isolated
Scribe and Herdr sockets. It never addressed the shared production Herdr
socket, whose log and session contained none of the probe target or launch
identities.

Artifact identities for the faithful r5 run were:

| Artifact | SHA-256 / identity |
|---|---|
| installed fixed-function `abacus` and source | `3031b467fd76c6cc0ce66bd2bcffb579b953de7bb92622e01dfa85cd68f0dba1` |
| disposable Scribe/auth peer | `b2fec4d2dcfb2f4d78fdc0b6d7f86f9b36703e4f6e0ca2470a0c65a0d3e88e2d` |
| same-uid process-metadata scanner | `22f877eacaeb9b189c6460f73e000f2ed0383230e86471bef78a657e95b176e6` |
| selected-artifact plaintext scanner | `83463f08503c066c8f491dd67855b4dcd26fadcd27b7b75da4fad74f7b7cb7f5` |
| isolated Herdr config | `63a3490edea9742dd0692be9f7092bd3b62067a1a5fc7acfc50bd733752bf79b` |
| Herdr binary | v0.8.0, protocol 19, 21,776,680 bytes, `b872ea7e40fa2cb17e857ac9b62b1bf26db7b403c622f5d2f3f5b35f6e9acd28` |
| agent/runtime | Codex v0.146.1; Python 3.13.9 |

The installed dispatcher was byte-identical to its recorded source, mode
`0555`, outside both disposable agents' writable roots. Its parent was `0755`;
the isolated Scribe and Herdr sockets were each `0600`. Both public verbs still
accepted exactly two argv tokens and one bounded JSON line on stdin.

### Fresh loaded-rule and absent-rule controls

A fresh ephemeral Codex session loaded temporary operator-installed allow rules
for the exact absolute r3 dispatcher path plus the literal `scribe-rpc` and
`runtime-rpc` verbs. The injected approved-prefix snapshot contained both exact
surfaces. The agent opened two `scribe-rpc` processes and performed exactly one
`write_stdin` on each: the first credential-creating opening committed at the
peer but lost its response and returned typed `ambiguous` (exit 75); the explicit
reissue carried the returned provisioning, committed nothing new, and returned
`already-applied` with the identical credential id and 64-hex secret. The peer
recorded one opening only. This replaces the feasibility probe's hard-coded
request with the required live exec-session/dynamic-stdin composer behavior.

The first attempted absent-rule control was discarded because its fresh session
still inherited the temporary rules. The valid r4 control used an isolated Git
root and `codex exec --ephemeral --ignore-user-config --ignore-rules` with
`approval_policy="never"`. It requested the same exact host action once, received
the agent-boundary refusal `Approval policy is currently never. Do not provide
sandbox_permissions when approval policy is never.`, and stopped. No dispatcher
process, socket connection, `write_stdin`, alternate transport, or retry
occurred. The temporary rules were then removed; the global rules file returned
to its pre-probe SHA-256
`3a0682587b80e79a4e3a28c57f635c537f1314a0ec68c1b98aacf47037964a93`.

### Faithful `runtime-rpc` chain

The r5 public runtime schema contained exactly `version`, `request_id`,
`repo_id`, authenticated actor, closed worker-Attempt subject plus credential
id, operation, bounded deadline, and transient secret. It exposed no executable,
argv, cwd, environment, Envelope, provider target, pane, or socket coordinate.
The host side sent the digest and binding to the isolated Scribe peer first;
only an atomic authorized-and-burned response supplied the already-persisted
sanitized Envelope and operator-resolved target. The dispatcher then issued a
newline-delimited `agent.prompt` request directly on the pinned isolated Herdr
socket. No high-level Herdr argv carried startup material.

Each of three r5 credential openings independently exercised the same
commit-response-loss and explicit-reissue path as r3: one `write_stdin` per
process, typed `ambiguous` followed by `already-applied`, identical provisioning,
and exactly one durable opening. The first runtime setup used Codex's nondefault
`--no-alt-screen` mode. Authorization, burn, direct socket submission, and the
process scan passed, but the target did not receive the prompt even though Herdr
returned `agent_prompted`. This row was discarded: the existing pinned-provider
record already classifies that launch mode as unsupported. The target was
cleanly restarted under Codex's supported default TUI before the certification
rows.

The supported success row then passed the complete chain:

1. one approved `abacus runtime-rpc` process received one dynamic stdin request;
2. Scribe authenticated the actor and subject binding and atomically burned the
   one-shot secret before any Herdr request;
3. the runtime sent exactly one direct-socket `agent.prompt` request;
4. the command returned typed `submitted`; and
5. the disposable Codex target returned exactly `HPG5_STARTUP_RECEIVED` and
   settled back to `idle`.

During the live pre-authorization hold, a same-uid `/proc` scanner inspected 78
readable `cmdline`/`environ` fields across the composer, Scribe peer, Herdr,
target agent, and other same-uid processes; four racing/protected fields were
unreadable and zero contained the secret. The supported row repeated the scan
with the same 78/4/zero result. A separate exact-plaintext scan of six selected
ABACUS-owned prototype/state/log artifacts found zero matches for all three
secrets. As ADR-0003 states, Herdr and Codex necessarily receive startup
plaintext and the provider-owned terminal/native transcript retains it; that is
the accepted revocation-bounded residual, not an ABACUS-owned state/log claim.

### Fail-closed and ambiguous-delivery controls

| Control | State/Scribe observation | Herdr observation | Result |
|---|---|---|---|
| Public request adds hostile `executable`, `args`, `cwd`, `env`, and `envelope` fields | no request/event | prompt count unchanged | exit 64, typed `unsupported-field` |
| Actor credential is unauthorized | one typed `refused-credential-invalid` event | prompt count unchanged | exit 1; authorization stopped the chain |
| Simulated loss after a real Herdr submission | one `authorized-and-burned` event | prompt count increased by exactly one | exit 75, typed `ambiguous` |
| Explicit reissue of that exact request/secret | one `refused-credential-burned` event | prompt count unchanged | exit 1; no redelivery |

The target's non-secret response marker appeared exactly twice across the
supported success and the single ambiguous submission. The isolated Herdr log
recorded three total runtime prompt submissions only because it also retained
the explicitly discarded no-alt-screen setup row. The shared production Herdr
log/session recorded zero probe launch or target identities.

The Scribe peer's final eleven-event summary was three opening commits with
dropped responses, three explicit `already-applied` reissues, three authorized
one-shot burns (discarded setup, supported success, ambiguous submission), one
unauthorized-actor refusal, and one burned-secret refusal. The hostile-schema
request never reached it. Runtime stage records contained only request ids,
process ids, and closed stage names—never credential plaintext.

Cleanup was exact and post-evidence: the target exited natively; the isolated
Herdr server and Scribe peer stopped; five named `/tmp/abacus-hpg5-*` roots and
three exact `/run/user/1000/abacus/abacus-hpg5-*` runtime roots were removed.
Post-cleanup enumeration returned no HPG.5 directory in either parent, and the
global rule file still matched its baseline hash with no HPG.5 rule. No probe
action created, removed, or renamed a shared production Herdr workspace, pane,
socket, or session; ordinary Claude/Codex coordination continued there
independently. No agent configuration, hook, or repository source file changed.
The ordinary provider-owned Codex native transcript remains the explicitly
accepted residual described above.

## Remaining decision step

All executable HPG.5 preconditions and the faithful disposable runtime-carriage
gate now pass. ADR-0003 remains Proposed, and the transport-design bead remains
open, until the operator explicitly signs off; evidence alone does not make that
governance decision.
