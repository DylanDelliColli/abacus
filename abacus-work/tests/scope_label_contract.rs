//! Integration contract for ADR-0002 §1 normalization (ABACUS-omw.4).
//!
//! Exercises the public API the way the `br` adapter will: take a bead's
//! raw provider label set, produce the normalized scope map that enters
//! the Assignment, and produce the covered-label pre-image that the
//! bead-content hash binds.

use abacus_core::ports::WorkError;
use abacus_core::{ScopeKey, ScopeValue};
use abacus_work::scope_labels::{
    covered_scope_labels, normalize_scope_labels, scope_label_preimage,
};

fn declared() -> Vec<ScopeKey> {
    ["area", "epic"]
        .iter()
        .map(|k| ScopeKey::new(k).expect("valid key"))
        .collect()
}

fn key(raw: &str) -> ScopeKey {
    ScopeKey::new(raw).expect("valid key")
}

fn value(raw: &str) -> ScopeValue {
    ScopeValue::new(raw).expect("valid value")
}

/// A realistic provider label set: two scope labels amid ordinary ones.
fn realistic() -> Vec<&'static str> {
    vec![
        "needs-review",
        "area:auth",
        "priority:p2",
        "epic:login",
        "good-first-issue",
    ]
}

#[test]
fn a_realistic_bead_normalizes_to_its_scope_map() {
    let map = normalize_scope_labels(&declared(), realistic()).expect("well-formed bead");

    assert_eq!(map.get(&key("area")), Some(&value("auth")));
    assert_eq!(map.get(&key("epic")), Some(&value("login")));
    assert_eq!(
        map.pairs().count(),
        2,
        "ordinary labels must not enter the scope map"
    );
}

#[test]
fn a_bead_spanning_two_values_of_one_key_is_refused_before_assignment() {
    // ADR-0002 §1: a decomposition defect surfaced for explicit
    // re-labeling, never auto-resolved by picking a winner.
    let labels = vec!["area:auth", "epic:login", "area:billing"];

    assert_eq!(
        normalize_scope_labels(&declared(), labels),
        Err(WorkError::ScopeLabelConflict {
            key: "area".to_owned()
        })
    );
}

#[test]
fn the_covered_set_and_the_map_agree_on_which_labels_are_scope_relevant() {
    let map = normalize_scope_labels(&declared(), realistic()).expect("well-formed bead");
    let covered = covered_scope_labels(&declared(), realistic());

    assert_eq!(
        covered.len(),
        map.pairs().count(),
        "every covered raw label must produce exactly one map entry"
    );
    for (k, v) in map.pairs() {
        let encoded = format!("{}:{}", k.as_str(), v.as_str());
        assert!(
            covered.contains(&encoded),
            "{encoded} is in the map but not covered by the hash"
        );
    }
}

#[test]
fn relabelling_a_scope_label_changes_the_preimage() {
    // The drift path that ADR-0002 §1 relies on: a changed label fails
    // the existing bead-content-hash recheck at Acceptance.
    let before = scope_label_preimage(&declared(), realistic());

    let mut relabelled = realistic();
    relabelled[1] = "area:billing";
    let after = scope_label_preimage(&declared(), relabelled);

    assert_ne!(before, after);
}

#[test]
fn churning_ordinary_labels_does_not_change_the_preimage() {
    // Ordinary labels are not part of the bead contract, so routine
    // triage churn must not invalidate an Assignment.
    let before = scope_label_preimage(&declared(), realistic());

    let mut churned = realistic();
    churned.retain(|label| *label != "good-first-issue");
    churned.push("wontfix-maybe");
    let after = scope_label_preimage(&declared(), churned);

    assert_eq!(before, after);
}

#[test]
fn provider_label_ordering_does_not_change_the_preimage() {
    let forward = scope_label_preimage(&declared(), realistic());

    let mut reversed = realistic();
    reversed.reverse();
    let backward = scope_label_preimage(&declared(), reversed);

    assert_eq!(
        forward, backward,
        "the provider does not promise label ordering"
    );
}

#[test]
fn an_undeclared_key_never_becomes_scope_relevant() {
    // `priority:p2` is a well-formed `key:value` label, but `priority`
    // is not declared, so it is ordinary and invisible to scoping.
    let map = normalize_scope_labels(&declared(), realistic()).expect("well-formed bead");
    assert_eq!(map.get(&key("area")), Some(&value("auth")));

    let covered = covered_scope_labels(&declared(), realistic());
    assert!(!covered.iter().any(|label| label.starts_with("priority:")));
}
