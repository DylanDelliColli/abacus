//! ADR-0002 §1 label normalization at the work seam (ABACUS-omw.4).
//!
//! The provider stores flat labels; the algebra evaluates a normalized
//! map. This module is the only place that crossing happens, and it is
//! where the two deterministic refusals fire — both *before any
//! Assignment exists*, per ADR-0002 §1:
//!
//! - `scope-label-malformed` → [`WorkError::ScopeLabelMalformed`]
//! - `scope-label-conflict`  → [`WorkError::ScopeLabelConflict`]
//!
//! Single-valuedness is load-bearing, not cosmetic: it is the
//! precondition that makes the disjointness procedure in ADR-0002 §4
//! sound. A bead labeled both `area:a` and `area:b` would otherwise
//! match two provably-disjoint exclusive scopes at once. That is a
//! decomposition defect surfaced for explicit re-labeling, never
//! auto-resolved by picking a winner.

use std::collections::BTreeMap;

use abacus_core::ports::WorkError;
use abacus_core::{ContentHash, ScopeKey, ScopeMap, ScopeValue};

/// Provider whole-label ceiling for pinned `br` v0.1.45.
///
/// Direct evidence (`docs/compatibility/2026-08-04-br-bv.md`, HPG.1
/// fixture set): 50 characters accepted, 51 exits 4 `VALIDATION_FAILED`.
/// Checked here as well as by the key/value bounds, because this is the
/// provider's hard limit and must fail as OUR refusal rather than as an
/// opaque provider exit code.
pub const PROVIDER_MAX_LABEL_LEN: usize = 50;

/// Select the declared-key labels from a bead's raw label set and
/// normalize them into a [`ScopeMap`].
///
/// A label with no colon, or whose prefix up to the first colon is not a
/// declared key, is an ordinary label: invisible to scoping and silently
/// ignored. Only declared-key labels can fail.
/// Resolve a raw label to its declared key, or `None` if it is ordinary.
///
/// Splits at the FIRST colon: a value may not contain a further colon,
/// so a second one makes the value malformed rather than re-splitting.
fn declared_label<'k, 'a>(
    declared: &'k [ScopeKey],
    label: &'a str,
) -> Option<(&'k ScopeKey, &'a str)> {
    let (prefix, value) = label.split_once(':')?;
    declared
        .iter()
        .find(|key| key.as_str() == prefix)
        .map(|key| (key, value))
}

pub fn normalize_scope_labels<'a>(
    declared: &[ScopeKey],
    raw_labels: impl IntoIterator<Item = &'a str>,
) -> Result<ScopeMap, WorkError> {
    let mut bound: BTreeMap<ScopeKey, ScopeValue> = BTreeMap::new();

    for label in raw_labels {
        let Some((key, raw_value)) = declared_label(declared, label) else {
            continue; // ordinary label: invisible to scoping
        };

        // Defense in depth. With config-validated keys (<=15) and the
        // value bound (<=34) this cannot fire, but the provider limit is
        // the real constraint and must surface as OUR refusal if either
        // bound ever widens.
        if label.len() > PROVIDER_MAX_LABEL_LEN {
            return Err(WorkError::ScopeLabelMalformed {
                label: label.to_owned(),
            });
        }

        let value = ScopeValue::new(raw_value).map_err(|_| WorkError::ScopeLabelMalformed {
            label: label.to_owned(),
        })?;

        match bound.get(key) {
            // Benign provider duplication: single-valuedness still holds.
            Some(existing) if *existing == value => continue,
            Some(_) => {
                return Err(WorkError::ScopeLabelConflict {
                    key: key.as_str().to_owned(),
                });
            }
            None => {
                bound.insert(key.clone(), value);
            }
        }
    }

    // A `BTreeMap` cannot yield a duplicate key, so the only failure
    // mode `ScopeMap::new` has is structurally unreachable here.
    Ok(ScopeMap::new(bound.into_iter().collect())
        .expect("deduplicated pairs cannot collide on a key"))
}

/// The raw declared-key label strings a bead-content hash must cover,
/// in a deterministic order.
///
/// ADR-0002 §1 "Snapshot semantics": the bead-content hash covers the
/// RAW declared-key-prefixed label strings. Because the normalized map
/// derives purely from those strings, covering the raw form means a
/// change to either the labels or the derived map fails the existing
/// hash recheck at Acceptance — label drift reuses the contract-drift
/// refusal path instead of adding new machinery.
pub fn covered_scope_labels<'a>(
    declared: &[ScopeKey],
    raw_labels: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut covered: Vec<String> = raw_labels
        .into_iter()
        .filter(|label| declared_label(declared, label).is_some())
        .map(str::to_owned)
        .collect();

    // Sorted so provider label ordering cannot change the hash — the
    // provider promises no ordering, so order is not content.
    //
    // Multiplicity IS content, and is deliberately NOT deduplicated.
    // ADR-0002 §1 states the bead-content hash covers the raw
    // declared-key-prefixed label strings, and that a change to either
    // the raw labels or the derived map fails the recheck. Adding a
    // second `area:auth` leaves the normalized map identical but does
    // change the raw labels, so it must fail recheck and force explicit
    // re-evaluation. Collapsing it here would silently absorb a raw
    // change — the covering is conservative on purpose, and "benign"
    // is the Acceptance step's judgement to make, not this function's.
    covered.sort();
    covered
}

/// The unambiguous byte pre-image over the covered raw labels.
///
/// Length-prefixed framing, not a delimiter join: a separator-joined
/// encoding lets two different label sets collide (`["a:b", "c:d"]` vs
/// `["a:b\nc:d"]`), which would let label drift slip past a hash
/// recheck. This defines WHAT is hashed; [`bead_content_hash`] binds
/// HOW, and nothing here hand-rolls a hash.
pub fn scope_label_preimage<'a>(
    declared: &[ScopeKey],
    raw_labels: impl IntoIterator<Item = &'a str>,
) -> Vec<u8> {
    let covered = covered_scope_labels(declared, raw_labels);

    let mut out = Vec::new();
    out.extend_from_slice(&(covered.len() as u64).to_be_bytes());
    for label in covered {
        out.extend_from_slice(&(label.len() as u64).to_be_bytes());
        out.extend_from_slice(label.as_bytes());
    }
    out
}

/// The canonical bead-content hash: SHA-256 over
/// [`scope_label_preimage`], rendered as the 64-hex [`ContentHash`]
/// core requires for assignment and acceptance rechecks.
///
/// This is the ONE digest path (ABACUS-omw.8, operator-authorized
/// `sha2` dependency): the pre-image defines WHAT is hashed, this
/// binds HOW, and hand-rolled cryptography remains forbidden.
pub fn bead_content_hash<'a>(
    declared: &[ScopeKey],
    raw_labels: impl IntoIterator<Item = &'a str>,
) -> ContentHash {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;

    let digest = Sha256::digest(scope_label_preimage(declared, raw_labels));
    let hex = digest
        .iter()
        .fold(String::with_capacity(digest.len() * 2), |mut hex, byte| {
            write!(hex, "{byte:02x}").expect("writing hex to a String cannot fail");
            hex
        });
    ContentHash::new(&hex).expect("a SHA-256 digest is always 64 lowercase hex characters")
}

#[cfg(test)]
mod tests {
    use super::*;

    use abacus_core::scope::{MAX_KEY_LEN, MAX_VALUE_LEN};

    fn keys(raw: &[&str]) -> Vec<ScopeKey> {
        raw.iter()
            .map(|k| ScopeKey::new(k).expect("valid declared key"))
            .collect()
    }

    fn key(raw: &str) -> ScopeKey {
        ScopeKey::new(raw).expect("valid key")
    }

    fn value(raw: &str) -> ScopeValue {
        ScopeValue::new(raw).expect("valid value")
    }

    #[test]
    fn declared_key_labels_become_the_normalized_map() {
        let map = normalize_scope_labels(&keys(&["area", "epic"]), ["area:auth", "epic:login"])
            .expect("well-formed labels");

        assert_eq!(map.get(&key("area")), Some(&value("auth")));
        assert_eq!(map.get(&key("epic")), Some(&value("login")));
    }

    #[test]
    fn undeclared_and_colonless_labels_are_ordinary_and_ignored() {
        let map = normalize_scope_labels(
            &keys(&["area"]),
            [
                "area:auth",
                "priority:high", // undeclared key
                "needs-review",  // no colon at all
                "team:platform", // undeclared key
            ],
        )
        .expect("ordinary labels never fail");

        assert_eq!(map.get(&key("area")), Some(&value("auth")));
        assert_eq!(map.pairs().count(), 1, "only declared keys enter the map");
    }

    #[test]
    fn an_empty_label_set_normalizes_to_an_empty_map() {
        let map = normalize_scope_labels(&keys(&["area"]), []).expect("no labels is valid");
        assert_eq!(map.pairs().count(), 0);
    }

    #[test]
    fn declaring_no_keys_makes_every_label_ordinary() {
        let map =
            normalize_scope_labels(&[], ["area:auth", "epic:login"]).expect("nothing is declared");
        assert_eq!(map.pairs().count(), 0);
    }

    #[test]
    fn the_same_key_bound_twice_to_the_same_value_is_not_a_conflict() {
        // Benign provider duplication: single-valuedness still holds.
        let map = normalize_scope_labels(&keys(&["area"]), ["area:auth", "area:auth"])
            .expect("identical bindings are not a conflict");

        assert_eq!(map.get(&key("area")), Some(&value("auth")));
        assert_eq!(map.pairs().count(), 1);
    }

    #[test]
    fn the_same_key_bound_to_different_values_is_a_conflict() {
        // The load-bearing case: this bead would otherwise match two
        // provably-disjoint exclusive scopes at once.
        assert_eq!(
            normalize_scope_labels(&keys(&["area"]), ["area:auth", "area:billing"]),
            Err(WorkError::ScopeLabelConflict {
                key: "area".to_owned()
            })
        );
    }

    #[test]
    fn an_empty_value_is_malformed() {
        assert_eq!(
            normalize_scope_labels(&keys(&["area"]), ["area:"]),
            Err(WorkError::ScopeLabelMalformed {
                label: "area:".to_owned()
            })
        );
    }

    #[test]
    fn invalid_value_characters_are_malformed() {
        for label in [
            "area:Auth",      // uppercase
            "area:auth.sub",  // dot
            "area:auth:more", // further colon
            "area:auth space",
        ] {
            assert_eq!(
                normalize_scope_labels(&keys(&["area"]), [label]),
                Err(WorkError::ScopeLabelMalformed {
                    label: label.to_owned()
                }),
                "expected malformed refusal for {label:?}"
            );
        }
    }

    #[test]
    fn a_maximal_label_sits_exactly_on_the_provider_bound() {
        // The bounds interlock: a 15-char key plus ':' plus a 34-char
        // value is exactly 50, the provider's accepted maximum. This is
        // the ONLY way to reach 50, which is why ADR-0002 §2 chose those
        // two numbers — they are not independent budgets.
        let max_key = "k".repeat(MAX_KEY_LEN);
        let max_value = "v".repeat(MAX_VALUE_LEN);
        let at_bound = format!("{max_key}:{max_value}");
        assert_eq!(at_bound.len(), PROVIDER_MAX_LABEL_LEN);

        let map = normalize_scope_labels(&keys(&[max_key.as_str()]), [at_bound.as_str()])
            .expect("a 50-character label is accepted by the provider");
        assert_eq!(map.get(&key(&max_key)), Some(&value(&max_value)));
    }

    #[test]
    fn a_label_past_the_provider_bound_is_our_refusal_not_a_provider_exit() {
        // 51 characters: the provider would exit 4 VALIDATION_FAILED, so
        // it must fail here first, as a normalized ABACUS refusal.
        let max_key = "k".repeat(MAX_KEY_LEN);
        let over_value = "v".repeat(MAX_VALUE_LEN + 1);
        let over_bound = format!("{max_key}:{over_value}");
        assert_eq!(over_bound.len(), PROVIDER_MAX_LABEL_LEN + 1);

        assert_eq!(
            normalize_scope_labels(&keys(&[max_key.as_str()]), [over_bound.as_str()]),
            Err(WorkError::ScopeLabelMalformed {
                label: over_bound.clone()
            })
        );
    }

    #[test]
    fn covered_labels_are_the_raw_declared_key_strings_in_stable_order() {
        let covered = covered_scope_labels(
            &keys(&["area", "epic"]),
            ["epic:login", "needs-review", "area:auth", "team:platform"],
        );

        assert_eq!(
            covered,
            vec!["area:auth".to_owned(), "epic:login".to_owned()],
            "covered set is the raw declared-key labels, deterministically ordered"
        );
    }

    #[test]
    fn covered_label_order_is_independent_of_provider_ordering() {
        let a = covered_scope_labels(&keys(&["area", "epic"]), ["area:auth", "epic:login"]);
        let b = covered_scope_labels(&keys(&["area", "epic"]), ["epic:login", "area:auth"]);

        assert_eq!(a, b, "provider label ordering must not change the hash");
    }

    #[test]
    fn a_duplicated_raw_label_changes_the_preimage() {
        // Multiplicity is content. The normalized map is identical
        // either way (same key, same value — a benign duplicate), but
        // the RAW labels differ, and ADR-0002 §1 binds the hash to the
        // raw strings. So this must fail the Acceptance recheck and
        // force explicit re-evaluation rather than being absorbed here.
        let declared = keys(&["area"]);

        let once = scope_label_preimage(&declared, ["area:auth"]);
        let twice = scope_label_preimage(&declared, ["area:auth", "area:auth"]);

        assert_ne!(
            once, twice,
            "a duplicated raw label is a change to the covered raw labels"
        );

        // ...while normalization still treats it as the same single
        // binding, so the two rules coexist without contradicting.
        let map = normalize_scope_labels(&declared, ["area:auth", "area:auth"])
            .expect("identical bindings remain benign for normalization");
        assert_eq!(map.get(&key("area")), Some(&value("auth")));
        assert_eq!(map.pairs().count(), 1);
    }

    #[test]
    fn covered_labels_retain_multiplicity_in_sorted_order() {
        let covered = covered_scope_labels(
            &keys(&["area", "epic"]),
            ["epic:login", "area:auth", "area:auth"],
        );

        assert_eq!(
            covered,
            vec![
                "area:auth".to_owned(),
                "area:auth".to_owned(),
                "epic:login".to_owned()
            ]
        );
    }

    #[test]
    fn the_preimage_changes_when_a_covered_label_changes() {
        let declared = keys(&["area"]);
        let before = scope_label_preimage(&declared, ["area:auth"]);
        let after = scope_label_preimage(&declared, ["area:billing"]);

        assert_ne!(
            before, after,
            "label drift must change the pre-image so the hash recheck fails"
        );
    }

    #[test]
    fn the_preimage_ignores_ordinary_label_churn() {
        let declared = keys(&["area"]);
        let before = scope_label_preimage(&declared, ["area:auth", "needs-review"]);
        let after = scope_label_preimage(&declared, ["area:auth", "wip"]);

        assert_eq!(
            before, after,
            "only declared-key labels are covered by the bead-content hash"
        );
    }

    #[test]
    fn the_preimage_framing_is_unambiguous() {
        let declared = keys(&["a", "c"]);

        // Two genuinely different covered sets that a delimiter-joined
        // encoding could render identically.
        let split = scope_label_preimage(&declared, ["a:b", "c:d"]);
        let joined = scope_label_preimage(&declared, ["a:b-c-d"]);

        assert_ne!(
            split, joined,
            "length-prefixed framing must prevent a separator collision"
        );
    }

    #[test]
    fn the_digest_is_canonical_sha256_of_the_preimage() {
        // Known-answer vector: an empty covered set's pre-image is
        // exactly eight zero bytes (the u64 count), independently
        // computed: sha256(00 00 00 00 00 00 00 00).
        assert_eq!(
            bead_content_hash(&keys(&["area"]), []).as_str(),
            "af5570f5a1810b7af78caf4bc70a660f0df51e42baf91d4de5b2328de0e83dfc"
        );
    }

    #[test]
    fn covered_label_drift_changes_the_content_hash() {
        let declared = keys(&["area"]);
        assert_ne!(
            bead_content_hash(&declared, ["area:auth"]),
            bead_content_hash(&declared, ["area:billing"]),
            "declared-key label drift must fail the recheck"
        );
    }

    #[test]
    fn provider_ordering_never_changes_the_content_hash() {
        let declared = keys(&["area", "epic"]);
        assert_eq!(
            bead_content_hash(&declared, ["area:auth", "epic:login"]),
            bead_content_hash(&declared, ["epic:login", "area:auth"]),
            "the provider promises no ordering, so order is not content"
        );
    }

    #[test]
    fn ordinary_label_churn_never_changes_the_content_hash() {
        let declared = keys(&["area"]);
        assert_eq!(
            bead_content_hash(&declared, ["area:auth", "note:one"]),
            bead_content_hash(&declared, ["area:auth", "note:two", "misc"]),
            "labels outside the declared keys are not content"
        );
    }
}
