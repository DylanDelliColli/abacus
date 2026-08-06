//! Target resolution: turning what a caller names into a fenced identity.
//!
//! The provider's snapshot carries two easily-confused fields, and the
//! difference is a contract obligation rather than a detail
//! (`docs/compatibility/2026-08-04-herdr.md`, live dogfood):
//!
//! - `kind` is the **detected client type** (`claude`, `codex`). It is
//!   never a target. "The claude one" is not an address, because there
//!   may be several and the answer changes as sessions come and go.
//!   Resolution never consults this field.
//! - `name` is an **optional live locator** assigned by `agent start`
//!   or `agent rename`. It is legitimate to target, and the recorded
//!   evidence has panes renamed to exactly `claude` and `codex` with
//!   delivery continuing normally.
//!
//! So the refusal is about which field is consulted, not about which
//! strings are allowed. Rejecting the literal text `claude` would break
//! the case the compatibility record documents as working.
//!
//! A name is convenience routing metadata, never ABACUS identity. It
//! disappears when its agent exits and may be reassigned. Resolution
//! therefore always yields the full fenced identity — namespace, pane,
//! and terminal generation — so a caller that arrived by name still
//! holds the fence that detects a restart or live handoff.

use crate::adapter::RawSessionIdentity;

/// One session as the provider's snapshot describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSnapshot {
    pub pane: String,
    /// Terminal/process generation: the fencing component.
    pub generation: String,
    /// Detected client kind. Deliberately **not** addressable.
    pub kind: String,
    /// Assigned live locator, if the session has one.
    pub name: Option<String>,
}

/// Why a target did not resolve to exactly one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetRefusal {
    /// No pane and no assigned name matched. Also the honest answer
    /// when a name has disappeared because its agent exited, and when
    /// a caller tried to address a bare detected kind.
    Unknown,
    /// More than one session answered to the same name. Names are
    /// provider convenience metadata with no uniqueness guarantee we
    /// control, so this refuses rather than picking one.
    Ambiguous { matches: usize },
    /// The target string was empty.
    Empty,
}

/// Resolve a caller-supplied target within one namespace.
///
/// Matches a pane locator or an assigned name, never a detected kind.
pub fn resolve_target(
    snapshots: &[AgentSnapshot],
    namespace: &str,
    target: &str,
) -> Result<RawSessionIdentity, TargetRefusal> {
    if target.is_empty() {
        return Err(TargetRefusal::Empty);
    }
    let matched: Vec<&AgentSnapshot> = snapshots
        .iter()
        .filter(|snapshot| snapshot.pane == target || snapshot.name.as_deref() == Some(target))
        .collect();
    match matched.as_slice() {
        [] => Err(TargetRefusal::Unknown),
        [one] => Ok(RawSessionIdentity {
            namespace: namespace.to_owned(),
            pane: one.pane.clone(),
            generation: one.generation.clone(),
        }),
        many => Err(TargetRefusal::Ambiguous {
            matches: many.len(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(pane: &str, generation: &str, kind: &str, name: Option<&str>) -> AgentSnapshot {
        AgentSnapshot {
            pane: pane.to_owned(),
            generation: generation.to_owned(),
            kind: kind.to_owned(),
            name: name.map(str::to_owned),
        }
    }

    fn fleet() -> Vec<AgentSnapshot> {
        vec![
            snapshot("w1:p1", "gen-a", "claude", Some("claude")),
            snapshot("w1:p2", "gen-b", "codex", Some("codex")),
            snapshot("w1:p3", "gen-c", "claude", None),
        ]
    }

    #[test]
    fn a_pane_locator_resolves_to_its_fenced_identity() {
        assert_eq!(
            resolve_target(&fleet(), "ns", "w1:p3"),
            Ok(RawSessionIdentity {
                namespace: "ns".to_owned(),
                pane: "w1:p3".to_owned(),
                generation: "gen-c".to_owned(),
            }),
            "an unnamed pane is addressable by handle"
        );
    }

    #[test]
    fn an_assigned_name_resolves_even_when_it_equals_a_client_kind() {
        // The recorded dogfood renamed w1:p1 to `claude` and w1:p2 to
        // `codex`, and both kept working. Refusing these strings would
        // break the exact case the compatibility record documents.
        assert_eq!(
            resolve_target(&fleet(), "ns", "codex"),
            Ok(RawSessionIdentity {
                namespace: "ns".to_owned(),
                pane: "w1:p2".to_owned(),
                generation: "gen-b".to_owned(),
            })
        );
    }

    #[test]
    fn a_bare_detected_kind_is_not_a_target() {
        // w1:p3 is a `claude` session with NO assigned name. Nothing
        // may resolve it by its kind - that is the bare-kind refusal.
        let unnamed_only = vec![snapshot("w1:p3", "gen-c", "claude", None)];
        assert_eq!(
            resolve_target(&unnamed_only, "ns", "claude"),
            Err(TargetRefusal::Unknown),
            "'the claude one' is not an address; only panes and assigned names are"
        );
    }

    #[test]
    fn resolution_never_consults_the_kind_field_even_with_no_competing_name() {
        let one = vec![snapshot("w1:p9", "gen-z", "codex", None)];
        assert_eq!(
            resolve_target(&one, "ns", "codex"),
            Err(TargetRefusal::Unknown)
        );
    }

    #[test]
    fn a_name_that_disappeared_with_its_agent_is_unknown_not_stale() {
        // Names are live locators, not durable identity. When the agent
        // exits the name simply stops resolving.
        let after_exit = vec![snapshot("w1:p1", "gen-a", "claude", Some("claude"))];
        assert_eq!(
            resolve_target(&after_exit, "ns", "codex"),
            Err(TargetRefusal::Unknown)
        );
    }

    #[test]
    fn a_duplicated_name_refuses_rather_than_picking_one() {
        let duplicated = vec![
            snapshot("w1:p1", "gen-a", "claude", Some("worker")),
            snapshot("w1:p2", "gen-b", "codex", Some("worker")),
        ];
        assert_eq!(
            resolve_target(&duplicated, "ns", "worker"),
            Err(TargetRefusal::Ambiguous { matches: 2 }),
            "names carry no uniqueness guarantee we control; guessing would misroute"
        );
    }

    #[test]
    fn a_resolved_name_still_carries_the_generation_fence() {
        // Obligation 2: after resolving a convenience name the caller
        // must still hold the fenced identity, so a restart or live
        // handoff is detectable rather than silently followed.
        let resolved = resolve_target(&fleet(), "ns", "claude").expect("resolves");
        assert_eq!(resolved.generation, "gen-a");
        assert_eq!(resolved.pane, "w1:p1");
        assert_eq!(resolved.namespace, "ns");
    }

    #[test]
    fn the_same_name_after_a_generation_change_resolves_to_the_new_generation() {
        // Obligation 4's input half: a name survives a live handoff but
        // the generation rotates. Resolution reports the CURRENT
        // generation so the caller's stale handle can be fenced.
        let after_handoff = vec![snapshot("w1:p1", "gen-rotated", "claude", Some("claude"))];
        let resolved = resolve_target(&after_handoff, "ns", "claude").expect("resolves");
        assert_eq!(
            resolved.generation, "gen-rotated",
            "the name is the same; the fence must not be"
        );
    }

    #[test]
    fn an_empty_target_is_refused_distinctly() {
        assert_eq!(
            resolve_target(&fleet(), "ns", ""),
            Err(TargetRefusal::Empty)
        );
    }

    #[test]
    fn a_pane_match_wins_without_consulting_names() {
        let odd = vec![
            snapshot("w1:p1", "gen-a", "claude", Some("w1:p2")),
            snapshot("w1:p2", "gen-b", "codex", None),
        ];
        // Exactly two candidates claim `w1:p2`: one by pane, one by a
        // perverse name. Refusing is correct - silently preferring
        // either would make addressing depend on an invisible rule.
        assert_eq!(
            resolve_target(&odd, "ns", "w1:p2"),
            Err(TargetRefusal::Ambiguous { matches: 2 })
        );
    }
}
