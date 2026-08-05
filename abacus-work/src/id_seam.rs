//! The lossless ABACUS identifier seam (ABACUS-omw.3).
//!
//! Evidence: `docs/compatibility/2026-08-04-br-bv.md` §"Prefix mapping".
//! `br init --prefix ABACUS` accepts the cased prefix but normalizes
//! every generated ID to lowercase, so the two forms are:
//!
//! ```text
//! ABACUS external ID: ABACUS-<lowercase suffix>
//! br provider ID:     abacus-<lowercase suffix>
//! ```
//!
//! Because core's [`BeadId`] already constrains the suffix to lowercase
//! alphanumeric dot-separated segments, only the PREFIX differs. That is
//! what makes the mapping lossless in both directions rather than a
//! lossy case fold.
//!
//! Three refusals live here, all fail-closed:
//!
//! 1. a provider ID outside the configured namespace is foreign, never
//!    coerced into the ABACUS namespace;
//! 2. a graph containing any foreign ID is mixed, and initialization
//!    refuses it rather than guessing a mapping (record line 45);
//! 3. provider-emitted `claim_command`/`show_command` strings are
//!    untrusted convenience text and are structurally discarded — never
//!    shown to a role and never executed (record line 107).

// `BEAD_ID_PREFIX` is public on `abacus_core::id` but is not in the
// crate-root re-export list alongside its siblings, so it is addressed
// through its module path.
use abacus_core::BeadId;
use abacus_core::id::BEAD_ID_PREFIX;

/// The lowercase namespace `br` actually stores and generates.
pub const PROVIDER_PREFIX: &str = "abacus-";

/// Refusals at the identifier seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdSeamError {
    /// Right shape, wrong namespace: another tool's bead.
    ForeignNamespace { observed: String },
    /// The ABACUS namespace in the wrong case. `br` lookups are
    /// case-insensitive, so this can be echoed back, but a stored
    /// provider ID is always lowercase — normalizing it silently would
    /// hide a provider-behavior change, so it is refused loudly.
    UnnormalizedCase { observed: String },
    /// Not a usable identifier in any namespace.
    Malformed { observed: String },
    /// The graph mixes ABACUS beads with foreign ones. Initialization
    /// refuses rather than operating on a graph it only partly owns.
    MixedGraph { foreign: Vec<String> },
}

/// A validated lowercase provider-form identifier.
///
/// Constructing one is the ONLY way a provider string becomes an ID
/// inside this crate, so an unvalidated `String` cannot reach the facade.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderBeadId(String);

impl ProviderBeadId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for ProviderBeadId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The bare namespace, derived from the prefix so the two cannot drift.
fn namespace() -> &'static str {
    PROVIDER_PREFIX.trim_end_matches('-')
}

/// True when `raw` is in the ABACUS namespace ignoring case. Used only
/// to tell a mis-cased ABACUS id apart from a genuinely foreign one, so
/// the two produce different, actionable refusals.
fn is_abacus_namespace_ignoring_case(raw: &str) -> bool {
    match raw.split_once('-') {
        Some((ns, _)) => ns.eq_ignore_ascii_case(namespace()),
        None => false,
    }
}

/// External → provider. Total: a valid [`BeadId`] always maps.
///
/// Lossless because [`BeadId`] already guarantees a lowercase suffix —
/// only the prefix changes, so no case information is destroyed.
pub fn to_provider(bead: &BeadId) -> ProviderBeadId {
    ProviderBeadId(format!("{PROVIDER_PREFIX}{}", bead.suffix()))
}

/// Provider → external. Refuses foreign, mis-cased, and malformed input.
pub fn from_provider(raw: &str) -> Result<BeadId, IdSeamError> {
    if let Some(suffix) = raw.strip_prefix(PROVIDER_PREFIX) {
        let external = format!("{BEAD_ID_PREFIX}{suffix}");
        return BeadId::new(&external).map_err(|_| IdSeamError::Malformed {
            observed: raw.to_owned(),
        });
    }

    if is_abacus_namespace_ignoring_case(raw) {
        return Err(IdSeamError::UnnormalizedCase {
            observed: raw.to_owned(),
        });
    }

    Err(IdSeamError::ForeignNamespace {
        observed: raw.to_owned(),
    })
}

/// Refuse a graph that is not wholly ours (record line 45).
///
/// Reports EVERY foreign identifier rather than the first, so an
/// operator sees the real scope of the conflict in one pass.
pub fn assert_single_namespace<'a>(
    observed: impl IntoIterator<Item = &'a str>,
) -> Result<(), IdSeamError> {
    let foreign: Vec<String> = observed
        .into_iter()
        .filter(|raw| from_provider(raw).is_err())
        .map(str::to_owned)
        .collect();

    if foreign.is_empty() {
        Ok(())
    } else {
        Err(IdSeamError::MixedGraph { foreign })
    }
}

/// Raw provider bead fields as they arrive from `br --json`, including
/// the untrusted convenience strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawProviderBead {
    pub id: String,
    /// Untrusted: a literal `br` command line. Discarded.
    pub claim_command: Option<String>,
    /// Untrusted: a literal `br` command line. Discarded.
    pub show_command: Option<String>,
}

/// A provider bead after normalization.
///
/// This type has no field for provider command strings BY CONSTRUCTION:
/// discarding them is a property of the type, not a discipline the
/// adapter has to remember on every call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedBead {
    pub id: BeadId,
}

/// Normalize one raw provider bead, discarding its command strings.
pub fn normalize(raw: &RawProviderBead) -> Result<NormalizedBead, IdSeamError> {
    // `claim_command` and `show_command` are deliberately not read.
    // There is nowhere for them to go: [`NormalizedBead`] has no field
    // that could carry them.
    Ok(NormalizedBead {
        id: from_provider(&raw.id)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bead(raw: &str) -> BeadId {
        BeadId::new(raw).expect("valid external bead id")
    }

    #[test]
    fn external_maps_to_lowercase_provider_form() {
        assert_eq!(to_provider(&bead("ABACUS-hpg")).as_str(), "abacus-hpg");
        assert_eq!(to_provider(&bead("ABACUS-hpg.7")).as_str(), "abacus-hpg.7");
        assert_eq!(
            to_provider(&bead("ABACUS-9nh.11")).as_str(),
            "abacus-9nh.11"
        );
    }

    #[test]
    fn provider_maps_back_to_external_form() {
        assert_eq!(from_provider("abacus-hpg"), Ok(bead("ABACUS-hpg")));
        assert_eq!(from_provider("abacus-omw.1"), Ok(bead("ABACUS-omw.1")));
    }

    #[test]
    fn mapping_round_trips_losslessly_in_both_directions() {
        for raw in [
            "ABACUS-hpg",
            "ABACUS-hpg.7",
            "ABACUS-9nh.11",
            "ABACUS-a.b.c",
            "ABACUS-0",
        ] {
            let external = bead(raw);
            let provider = to_provider(&external);
            assert_eq!(
                from_provider(provider.as_str()),
                Ok(external.clone()),
                "round trip lost information for {raw}"
            );
        }
    }

    #[test]
    fn foreign_namespace_is_refused_not_coerced() {
        assert_eq!(
            from_provider("beads-hpg"),
            Err(IdSeamError::ForeignNamespace {
                observed: "beads-hpg".to_owned()
            })
        );
        // A prefix that merely starts with the same letters is not ours.
        assert_eq!(
            from_provider("abacusx-hpg"),
            Err(IdSeamError::ForeignNamespace {
                observed: "abacusx-hpg".to_owned()
            })
        );
    }

    #[test]
    fn mis_cased_provider_id_is_refused_loudly() {
        // `br` lookups are case-insensitive so this can be echoed, but a
        // STORED provider id is lowercase; silently folding would mask a
        // provider-behavior change.
        assert_eq!(
            from_provider("ABACUS-hpg"),
            Err(IdSeamError::UnnormalizedCase {
                observed: "ABACUS-hpg".to_owned()
            })
        );
        assert_eq!(
            from_provider("Abacus-hpg"),
            Err(IdSeamError::UnnormalizedCase {
                observed: "Abacus-hpg".to_owned()
            })
        );
    }

    #[test]
    fn malformed_provider_ids_are_refused() {
        for raw in ["", "abacus-", "abacus-hpg.", "abacus-.7", "abacus-hpg..7"] {
            assert!(
                matches!(
                    from_provider(raw),
                    Err(IdSeamError::Malformed { .. }) | Err(IdSeamError::ForeignNamespace { .. })
                ),
                "expected refusal for {raw:?}, got {:?}",
                from_provider(raw)
            );
        }
    }

    #[test]
    fn a_uniform_abacus_graph_is_accepted() {
        assert_eq!(
            assert_single_namespace(["abacus-hpg", "abacus-omw.1", "abacus-9nh.11"]),
            Ok(())
        );
        // An empty graph is trivially uniform: a fresh checkout.
        assert_eq!(assert_single_namespace([]), Ok(()));
    }

    #[test]
    fn a_mixed_graph_is_refused_with_every_foreign_id() {
        let result =
            assert_single_namespace(["abacus-hpg", "beads-xyz", "abacus-omw.1", "other-1"]);

        assert_eq!(
            result,
            Err(IdSeamError::MixedGraph {
                foreign: vec!["beads-xyz".to_owned(), "other-1".to_owned()]
            }),
            "every foreign id must be reported, not just the first"
        );
    }

    #[test]
    fn a_mis_cased_id_also_makes_the_graph_mixed() {
        let result = assert_single_namespace(["abacus-hpg", "ABACUS-omw.1"]);

        assert!(
            matches!(result, Err(IdSeamError::MixedGraph { .. })),
            "an unnormalized id is not silently accepted into a uniform graph"
        );
    }

    #[test]
    fn normalization_discards_provider_command_strings() {
        let raw = RawProviderBead {
            id: "abacus-omw.1".to_owned(),
            claim_command: Some("br update abacus-omw.1 --claim".to_owned()),
            show_command: Some("br show abacus-omw.1 --json".to_owned()),
        };

        let normalized = normalize(&raw).expect("valid provider bead");

        assert_eq!(normalized.id, bead("ABACUS-omw.1"));

        // The command strings must not survive anywhere in the value,
        // including its debug rendering, which is what reaches logs.
        let rendered = format!("{normalized:?}");
        assert!(
            !rendered.contains("br "),
            "raw br command leaked: {rendered}"
        );
        assert!(!rendered.contains("--claim"), "leaked: {rendered}");
        assert!(!rendered.contains("--json"), "leaked: {rendered}");
    }

    #[test]
    fn normalization_refuses_a_foreign_bead() {
        let raw = RawProviderBead {
            id: "beads-xyz".to_owned(),
            ..Default::default()
        };

        assert!(matches!(
            normalize(&raw),
            Err(IdSeamError::ForeignNamespace { .. })
        ));
    }
}
