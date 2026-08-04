# Scribe socket sandbox compatibility probe

Date: 2026-08-04  
Host: Linux x86_64  
Codex CLI: v0.146.0  
Status: Codex default sandbox denied; exact-socket permission-profile validation and Claude reciprocal probe remain

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

## Narrow Codex permission candidate

The current official Codex manual documents allowlist-first Unix-socket access through a permission profile:

```toml
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

A disposable nested `codex sandbox` invocation using the installed v0.146.0 CLI, the experimental network proxy, and an exact socket allow entry still returned `Operation not permitted`. That result does not prove the documented profile is ineffective for a normally launched Codex session: the sandbox helper was itself invoked from an already sandboxed session and may not reproduce session-level proxy setup. It does mean ABACUS cannot claim this path works until a fresh Codex session is launched with the profile and repeats the payload check.

Reference: [official Codex manual](https://developers.openai.com/codex/codex-manual.md), sections “Permissions / Unix sockets” and “Agent approvals & security / Network isolation.”

## Proposed transport decision

Keep the versioned Scribe client/server seam and the user-only Unix socket. Do not replace it with loopback TCP, workspace request files, polling, or a second resident bridge:

- loopback was denied by the same sandbox and weakens local peer identity;
- workspace request files would violate fail-loud transactional calls and invite forbidden watcher/retry machinery;
- broadly host-executing the future `abacus` CLI is unsafe because some supported commands intentionally execute project-supplied verification commands.

The intended deployment path is an exact Scribe-socket grant in each agent sandbox. Repository initialization can print the resolved socket path and a configuration/launch snippet, but must never edit global agent configuration. Scribe still authenticates and authorizes every versioned request; the socket allow entry grants reachability, not workflow authority.

If exact socket allowlisting cannot be demonstrated in both supported agent sandboxes, this remains a pre-implementation transport gate and requires a focused transport ADR. No fallback should be guessed into `abacus-state`.

## Remaining checks

- repeat the payload probe from a fresh Codex session launched with the exact permission profile;
- run the ordinary and explicitly granted probes inside the Claude sandbox;
- once one path passes, record the exact launch/configuration surface and add a fail-loud diagnostic contract for a missing grant.
