//! Integration contract for the identifier seam (ABACUS-omw.3).
//!
//! Exercises the crate's public re-exports the way the `br` adapter
//! (ABACUS-omw.2) will: normalize a provider listing at initialization,
//! refuse a graph that is not wholly ours, and prove no provider command
//! string survives normalization.

use abacus_core::BeadId;
use abacus_work::{
    IdSeamError, RawProviderBead, assert_single_namespace, from_provider, normalize, to_provider,
};

fn raw(id: &str) -> RawProviderBead {
    RawProviderBead {
        id: id.to_owned(),
        claim_command: Some(format!("br update {id} --claim")),
        show_command: Some(format!("br show {id} --json")),
    }
}

#[test]
fn a_provider_listing_normalizes_to_external_ids() {
    let listing = [raw("abacus-hpg"), raw("abacus-omw.1"), raw("abacus-9nh.11")];

    let normalized: Vec<BeadId> = listing
        .iter()
        .map(|bead| normalize(bead).expect("uniform abacus listing").id)
        .collect();

    assert_eq!(
        normalized,
        vec![
            BeadId::new("ABACUS-hpg").unwrap(),
            BeadId::new("ABACUS-omw.1").unwrap(),
            BeadId::new("ABACUS-9nh.11").unwrap(),
        ]
    );
}

#[test]
fn initialization_refuses_a_graph_that_is_not_wholly_ours() {
    // A checkout whose `.beads` predates ABACUS and still holds another
    // tool's beads. Guessing a mapping here would silently adopt work
    // ABACUS does not own.
    let observed = ["abacus-hpg", "beads-legacy-1", "abacus-omw.1"];

    let result = assert_single_namespace(observed);

    match result {
        Err(IdSeamError::MixedGraph { foreign }) => {
            assert_eq!(foreign, vec!["beads-legacy-1".to_owned()]);
        }
        other => panic!("expected MixedGraph refusal, got {other:?}"),
    }
}

#[test]
fn a_fresh_uniform_graph_initializes() {
    assert_eq!(
        assert_single_namespace(["abacus-hpg", "abacus-omw.1"]),
        Ok(())
    );
    assert_eq!(assert_single_namespace([]), Ok(()));
}

#[test]
fn no_provider_command_string_survives_normalization() {
    let bead = raw("abacus-omw.1");
    // Guard the fixture itself: the raw value really does carry the
    // untrusted strings, so the assertion below is meaningful.
    assert!(bead.claim_command.as_ref().unwrap().contains("br update"));

    let normalized = normalize(&bead).expect("valid bead");
    let rendered = format!("{normalized:?}");

    for leaked in ["br update", "br show", "--claim", "--json"] {
        assert!(
            !rendered.contains(leaked),
            "provider command text {leaked:?} escaped the seam: {rendered}"
        );
    }
}

#[test]
fn every_external_id_survives_a_provider_round_trip() {
    for raw_id in [
        "ABACUS-hpg",
        "ABACUS-omw.1",
        "ABACUS-9nh.11",
        "ABACUS-a.b.c",
    ] {
        let external = BeadId::new(raw_id).expect("valid external id");
        let provider = to_provider(&external);

        assert!(
            provider.as_str().starts_with("abacus-"),
            "provider form must be lowercase-namespaced"
        );
        assert_eq!(from_provider(provider.as_str()), Ok(external));
    }
}

#[test]
fn a_foreign_bead_never_becomes_an_abacus_bead() {
    assert!(matches!(
        normalize(&raw("beads-xyz")),
        Err(IdSeamError::ForeignNamespace { .. })
    ));
}
