//! Profile schema: grants, check classes, lifecycle bundles, hardened
//! config validation, and validated-artifact routing (ADR-0002 §3–§8).
//!
//! Validation is the only gate into routing and occupancy: a successful
//! [`validate_profiles`] returns a [`ValidatedProfileSet`], and
//! exclusive routing plus derived singleton occupancy consume only that
//! artifact. Caller-asserted occupancy and first-match routing are
//! structurally impossible.

use std::collections::BTreeSet;

use crate::authority::AuthorityClass;
use crate::content::ContentHash;
use crate::id::{ActorId, CapabilityId, OperationId, ProfileName};
use crate::scope::{ScopeExpr, ScopeMap};

/// How calls exercising a capability are authorized (ADR-0002 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckClass {
    /// Responsibility-owning authority: scope-routed, pairwise-disjoint
    /// across profiles, singleton-occupied. Orchestrator-class only.
    Exclusive,
    /// Attempt-bound worker mutations: authorized by binding plus lease
    /// fencing; grants overlap freely. Worker-class only.
    Fenced,
    /// Reads and observation: overlap freely, either class.
    Shared,
}

/// Descriptor-driven lifecycle bundles (ADR-0002 §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bundle {
    AssignmentLifecycle,
    AttemptLifecycle,
}

impl Bundle {
    /// The check class every member descriptor must declare.
    fn required_class(self) -> CheckClass {
        match self {
            Bundle::AssignmentLifecycle => CheckClass::Exclusive,
            Bundle::AttemptLifecycle => CheckClass::Fenced,
        }
    }
}

/// Occupancy class derived from validated grants (ADR-0002 §7):
/// singleton exactly when any exclusive grant exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OccupancyClass {
    Singleton,
    Shared,
}

/// A module-declared capability descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    pub id: CapabilityId,
    pub class: CheckClass,
    pub bundle: Option<Bundle>,
    /// Whether targets project to scope maps; non-work-scoped targets
    /// accept only `*` (ADR-0002 §5).
    pub work_scoped: bool,
}

/// One grant in a role card: capability → scope expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub capability: CapabilityId,
    pub scope: ScopeExpr,
}

/// A profile specification parsed from card frontmatter by the
/// composition layer; core owns the semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSpec {
    pub name: ProfileName,
    pub class: AuthorityClass,
    pub grants: Vec<Grant>,
}

/// Distinct configuration failures, each naming its exact location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileConfigError {
    UnknownCapability {
        profile: ProfileName,
        capability: CapabilityId,
    },
    NonWorkScopedRequiresUniversal {
        profile: ProfileName,
        capability: CapabilityId,
    },
    BundleIncoherent {
        profile: ProfileName,
        bundle: Bundle,
    },
    ExclusiveOverlap {
        capability: CapabilityId,
        first: ProfileName,
        first_scope: String,
        second: ProfileName,
        second_scope: String,
    },
    /// Registry defect: the same capability declared twice.
    DuplicateDescriptor { capability: CapabilityId },
    /// Two profiles share one name.
    DuplicateProfileName { profile: ProfileName },
    /// One profile grants the same capability twice (the bundle-length
    /// bypass).
    DuplicateGrant {
        profile: ProfileName,
        capability: CapabilityId,
    },
    /// A bundle member descriptor declares the wrong check class.
    DescriptorBundleClassMismatch { capability: CapabilityId },
    /// Grant check class incompatible with the profile's authority
    /// class (exclusive ⇒ orchestrator, fenced ⇒ worker).
    AuthorityIncompatible {
        profile: ProfileName,
        capability: CapabilityId,
    },
}

/// Core-validated activation snapshot: constructible only from a
/// [`ValidatedProfileSet`], so occupancy and authority class are
/// derived facts, never caller assertions. The full grant snapshot remains
/// part of the transitional activation record until its live state consumers
/// are replaced under ADR-0006.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileActivation {
    pub operation: OperationId,
    pub actor: ActorId,
    pub profile: ProfileName,
    pub profile_hash: ContentHash,
    class: AuthorityClass,
    occupancy: OccupancyClass,
    grants: Vec<Grant>,
}

/// The requested profile does not exist in the validated set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnknownProfile;

impl ProfileActivation {
    pub fn from_validated(
        set: &ValidatedProfileSet,
        operation: OperationId,
        actor: ActorId,
        profile: ProfileName,
        profile_hash: ContentHash,
    ) -> Result<Self, UnknownProfile> {
        let occupancy = set.occupancy_of(&profile).ok_or(UnknownProfile)?;
        let class = set.class_of(&profile).ok_or(UnknownProfile)?;
        let grants = set.grants_of(&profile).ok_or(UnknownProfile)?.to_vec();
        Ok(Self {
            operation,
            actor,
            profile,
            profile_hash,
            class,
            occupancy,
            grants,
        })
    }

    pub fn occupancy(&self) -> OccupancyClass {
        self.occupancy
    }

    pub fn class(&self) -> AuthorityClass {
        self.class
    }

    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }
}

/// The only artifact routing and occupancy accept: produced exclusively
/// by successful validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedProfileSet {
    profiles: Vec<ProfileSpec>,
    registry: Vec<CapabilityDescriptor>,
}

/// Routing outcome for an exclusive capability over a subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteOutcome {
    Profile(ProfileName),
    /// No exclusive holder matches: loud, never a silent default.
    Unroutable,
    /// The capability is not exclusive-class: routing does not apply.
    NotExclusive,
    /// Defense in depth: two matches would mean validation was
    /// bypassed; surfaced loudly, never first-match resolved.
    Ambiguous {
        first: ProfileName,
        second: ProfileName,
    },
}

impl ValidatedProfileSet {
    #[cfg(test)]
    fn new_unchecked(profiles: Vec<ProfileSpec>, registry: Vec<CapabilityDescriptor>) -> Self {
        Self { profiles, registry }
    }

    fn descriptor(&self, id: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.registry.iter().find(|d| &d.id == id)
    }

    pub fn profiles(&self) -> &[ProfileSpec] {
        &self.profiles
    }

    /// Derived routing (ADR-0002 §7): scans every profile; exactly one
    /// match routes, zero is loud `Unroutable`, two is loud
    /// `Ambiguous`. First-match is impossible by construction.
    pub fn route(&self, capability: &CapabilityId, subject: &ScopeMap) -> RouteOutcome {
        match self.descriptor(capability) {
            Some(d) if d.class == CheckClass::Exclusive => {}
            _ => return RouteOutcome::NotExclusive,
        }
        let mut matched: Option<&ProfileSpec> = None;
        for profile in &self.profiles {
            for grant in &profile.grants {
                if &grant.capability == capability && grant.scope.matches(subject) {
                    if let Some(first) = matched {
                        return RouteOutcome::Ambiguous {
                            first: first.name.clone(),
                            second: profile.name.clone(),
                        };
                    }
                    matched = Some(profile);
                }
            }
        }
        match matched {
            Some(profile) => RouteOutcome::Profile(profile.name.clone()),
            None => RouteOutcome::Unroutable,
        }
    }

    /// Derived occupancy (ADR-0002 §7): singleton exactly when the
    /// profile holds any exclusive grant. Unknown profiles yield None.
    pub fn occupancy_of(&self, profile: &ProfileName) -> Option<OccupancyClass> {
        let spec = self.profiles.iter().find(|p| &p.name == profile)?;
        let singleton = spec.grants.iter().any(|g| {
            self.descriptor(&g.capability)
                .is_some_and(|d| d.class == CheckClass::Exclusive)
        });
        Some(if singleton {
            OccupancyClass::Singleton
        } else {
            OccupancyClass::Shared
        })
    }

    /// The profile's authority class, for activation snapshots.
    pub fn class_of(&self, profile: &ProfileName) -> Option<AuthorityClass> {
        self.profiles
            .iter()
            .find(|p| &p.name == profile)
            .map(|p| p.class)
    }

    /// The profile's grants, for activation snapshots.
    pub fn grants_of(&self, profile: &ProfileName) -> Option<&[Grant]> {
        self.profiles
            .iter()
            .find(|p| &p.name == profile)
            .map(|p| p.grants.as_slice())
    }
}

/// Validate a full profile configuration against the capability
/// registry, reporting every failure. Success returns the
/// [`ValidatedProfileSet`] that routing and occupancy require.
pub fn validate_profiles(
    profiles: &[ProfileSpec],
    registry: &[CapabilityDescriptor],
) -> Result<ValidatedProfileSet, Vec<ProfileConfigError>> {
    let mut errors = Vec::new();

    // Registry integrity: unique descriptor ids, bundle-class
    // consistency.
    for (i, d) in registry.iter().enumerate() {
        if registry[i + 1..].iter().any(|other| other.id == d.id) {
            errors.push(ProfileConfigError::DuplicateDescriptor {
                capability: d.id.clone(),
            });
        }
        if let Some(bundle) = d.bundle
            && d.class != bundle.required_class()
        {
            errors.push(ProfileConfigError::DescriptorBundleClassMismatch {
                capability: d.id.clone(),
            });
        }
    }

    // Profile-name uniqueness.
    for (i, p) in profiles.iter().enumerate() {
        if profiles[i + 1..].iter().any(|other| other.name == p.name) {
            errors.push(ProfileConfigError::DuplicateProfileName {
                profile: p.name.clone(),
            });
        }
    }

    let descriptor = |id: &CapabilityId| registry.iter().find(|d| &d.id == id);

    for profile in profiles {
        // Duplicate grants (the bundle-length bypass).
        for (i, g) in profile.grants.iter().enumerate() {
            if profile.grants[i + 1..]
                .iter()
                .any(|other| other.capability == g.capability)
            {
                errors.push(ProfileConfigError::DuplicateGrant {
                    profile: profile.name.clone(),
                    capability: g.capability.clone(),
                });
            }
        }

        for grant in &profile.grants {
            match descriptor(&grant.capability) {
                None => errors.push(ProfileConfigError::UnknownCapability {
                    profile: profile.name.clone(),
                    capability: grant.capability.clone(),
                }),
                Some(desc) => {
                    if !desc.work_scoped && grant.scope != ScopeExpr::Universal {
                        errors.push(ProfileConfigError::NonWorkScopedRequiresUniversal {
                            profile: profile.name.clone(),
                            capability: grant.capability.clone(),
                        });
                    }
                    // Authority/check-class compatibility.
                    let compatible = match desc.class {
                        CheckClass::Exclusive => profile.class == AuthorityClass::Orchestrator,
                        CheckClass::Fenced => profile.class == AuthorityClass::Worker,
                        CheckClass::Shared => true,
                    };
                    if !compatible {
                        errors.push(ProfileConfigError::AuthorityIncompatible {
                            profile: profile.name.clone(),
                            capability: grant.capability.clone(),
                        });
                    }
                }
            }
        }

        // Bundle coherence: exact unique set membership at one
        // identical canonical scope.
        for bundle in [Bundle::AssignmentLifecycle, Bundle::AttemptLifecycle] {
            let members: BTreeSet<&CapabilityId> = registry
                .iter()
                .filter(|d| d.bundle == Some(bundle))
                .map(|d| &d.id)
                .collect();
            if members.is_empty() {
                continue;
            }
            let granted: Vec<&Grant> = profile
                .grants
                .iter()
                .filter(|g| members.contains(&g.capability))
                .collect();
            if granted.is_empty() {
                continue;
            }
            let granted_set: BTreeSet<&CapabilityId> =
                granted.iter().map(|g| &g.capability).collect();
            let first_scope = granted[0].scope.canonical();
            let coherent = granted.iter().all(|g| g.scope.canonical() == first_scope);
            if granted_set != members || !coherent {
                errors.push(ProfileConfigError::BundleIncoherent {
                    profile: profile.name.clone(),
                    bundle,
                });
            }
        }
    }

    // Exclusive disjointness across profiles, per capability.
    for desc in registry.iter().filter(|d| d.class == CheckClass::Exclusive) {
        let holders: Vec<(&ProfileSpec, &Grant)> = profiles
            .iter()
            .flat_map(|p| {
                p.grants
                    .iter()
                    .filter(|g| g.capability == desc.id)
                    .map(move |g| (p, g))
            })
            .collect();
        for (i, (profile_a, grant_a)) in holders.iter().enumerate() {
            for (profile_b, grant_b) in &holders[i + 1..] {
                if !grant_a.scope.disjoint(&grant_b.scope) {
                    errors.push(ProfileConfigError::ExclusiveOverlap {
                        capability: desc.id.clone(),
                        first: profile_a.name.clone(),
                        first_scope: grant_a.scope.canonical(),
                        second: profile_b.name.clone(),
                        second_scope: grant_b.scope.canonical(),
                    });
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(ValidatedProfileSet {
            profiles: profiles.to_vec(),
            registry: registry.to_vec(),
        })
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{ScopeKey, ScopeValue};

    fn keys() -> Vec<ScopeKey> {
        vec![ScopeKey::new("area").unwrap()]
    }

    fn scope(text: &str) -> ScopeExpr {
        ScopeExpr::parse(text, &keys()).unwrap()
    }

    fn cap(raw: &str) -> CapabilityId {
        CapabilityId::new(raw).unwrap()
    }

    fn name(raw: &str) -> ProfileName {
        ProfileName::new(raw).unwrap()
    }

    fn registry() -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor {
                id: cap("state:assign"),
                class: CheckClass::Exclusive,
                bundle: Some(Bundle::AssignmentLifecycle),
                work_scoped: true,
            },
            CapabilityDescriptor {
                id: cap("state:accept"),
                class: CheckClass::Exclusive,
                bundle: Some(Bundle::AssignmentLifecycle),
                work_scoped: true,
            },
            CapabilityDescriptor {
                id: cap("state:report"),
                class: CheckClass::Fenced,
                bundle: Some(Bundle::AttemptLifecycle),
                work_scoped: true,
            },
            CapabilityDescriptor {
                id: cap("state:handoff"),
                class: CheckClass::Fenced,
                bundle: Some(Bundle::AttemptLifecycle),
                work_scoped: true,
            },
            CapabilityDescriptor {
                id: cap("runtime:observe"),
                class: CheckClass::Shared,
                bundle: None,
                work_scoped: false,
            },
        ]
    }

    fn bundle_grants(scope_text: &str) -> Vec<Grant> {
        vec![
            Grant {
                capability: cap("state:assign"),
                scope: scope(scope_text),
            },
            Grant {
                capability: cap("state:accept"),
                scope: scope(scope_text),
            },
        ]
    }

    fn orchestrator(profile: &str, scope_text: &str) -> ProfileSpec {
        ProfileSpec {
            name: name(profile),
            class: AuthorityClass::Orchestrator,
            grants: bundle_grants(scope_text),
        }
    }

    fn worker(profile: &str) -> ProfileSpec {
        ProfileSpec {
            name: name(profile),
            class: AuthorityClass::Worker,
            grants: vec![
                Grant {
                    capability: cap("state:report"),
                    scope: scope("*"),
                },
                Grant {
                    capability: cap("state:handoff"),
                    scope: scope("*"),
                },
            ],
        }
    }

    #[test]
    fn partitioned_orchestrators_and_overlapping_workers_are_valid() {
        let profiles = vec![
            orchestrator("front-lead", "area=frontend"),
            orchestrator("rest-lead", "area!=frontend"),
            worker("worker-a"),
            worker("worker-b"),
        ];
        let set = validate_profiles(&profiles, &registry()).unwrap();
        assert_eq!(
            set.occupancy_of(&name("front-lead")),
            Some(OccupancyClass::Singleton)
        );
        assert_eq!(
            set.occupancy_of(&name("worker-a")),
            Some(OccupancyClass::Shared)
        );
        assert_eq!(
            set.class_of(&name("worker-a")),
            Some(AuthorityClass::Worker)
        );
    }

    #[test]
    fn exclusive_overlap_names_the_exact_pair() {
        let profiles = vec![
            orchestrator("lead-a", "area=frontend"),
            orchestrator("lead-b", "area=*"),
        ];
        let errors = validate_profiles(&profiles, &registry()).unwrap_err();
        assert!(errors.iter().any(|e| matches!(
            e,
            ProfileConfigError::ExclusiveOverlap { first, second, .. }
                if first.as_str() == "lead-a" && second.as_str() == "lead-b"
        )));
    }

    #[test]
    fn duplicate_grant_cannot_bypass_the_bundle() {
        // Two copies of state:assign at one scope must NOT satisfy the
        // two-member assignment bundle while state:accept is absent.
        let bypass = ProfileSpec {
            name: name("lead"),
            class: AuthorityClass::Orchestrator,
            grants: vec![
                Grant {
                    capability: cap("state:assign"),
                    scope: scope("*"),
                },
                Grant {
                    capability: cap("state:assign"),
                    scope: scope("*"),
                },
            ],
        };
        let errors = validate_profiles(&[bypass], &registry()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProfileConfigError::DuplicateGrant { .. }))
        );
        assert!(errors.iter().any(|e| matches!(
            e,
            ProfileConfigError::BundleIncoherent {
                bundle: Bundle::AssignmentLifecycle,
                ..
            }
        )));
    }

    #[test]
    fn duplicate_profiles_and_descriptors_are_rejected() {
        let profiles = vec![worker("worker-a"), worker("worker-a")];
        let errors = validate_profiles(&profiles, &registry()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProfileConfigError::DuplicateProfileName { .. }))
        );

        let mut bad_registry = registry();
        bad_registry.push(CapabilityDescriptor {
            id: cap("state:report"),
            class: CheckClass::Shared,
            bundle: None,
            work_scoped: true,
        });
        let errors = validate_profiles(&[worker("worker-a")], &bad_registry).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProfileConfigError::DuplicateDescriptor { .. }))
        );
    }

    #[test]
    fn authority_and_bundle_class_compatibility() {
        // A worker holding the exclusive assignment bundle is rejected:
        // it could otherwise accept its own handoff.
        let worker_exclusive = ProfileSpec {
            name: name("sneaky-worker"),
            class: AuthorityClass::Worker,
            grants: bundle_grants("*"),
        };
        let errors = validate_profiles(&[worker_exclusive], &registry()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProfileConfigError::AuthorityIncompatible { .. }))
        );

        // An orchestrator holding fenced worker capabilities is rejected.
        let fenced_orchestrator = ProfileSpec {
            name: name("lead"),
            class: AuthorityClass::Orchestrator,
            grants: vec![Grant {
                capability: cap("state:report"),
                scope: scope("*"),
            }],
        };
        let errors = validate_profiles(&[fenced_orchestrator], &registry()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProfileConfigError::AuthorityIncompatible { .. }))
        );

        // A bundle member descriptor with the wrong check class is a
        // registry defect.
        let mut bad_registry = registry();
        bad_registry.push(CapabilityDescriptor {
            id: cap("state:reject"),
            class: CheckClass::Shared,
            bundle: Some(Bundle::AssignmentLifecycle),
            work_scoped: true,
        });
        let errors = validate_profiles(&[], &bad_registry).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProfileConfigError::DescriptorBundleClassMismatch { .. }))
        );
    }

    #[test]
    fn routing_consumes_only_the_validated_artifact() {
        let profiles = vec![
            orchestrator("front-lead", "area=frontend"),
            orchestrator("design-lead", "area=design"),
        ];
        let set = validate_profiles(&profiles, &registry()).unwrap();
        let frontend = ScopeMap::new(vec![(
            ScopeKey::new("area").unwrap(),
            ScopeValue::new("frontend").unwrap(),
        )])
        .unwrap();
        assert_eq!(
            set.route(&cap("state:assign"), &frontend),
            RouteOutcome::Profile(name("front-lead"))
        );
        assert_eq!(
            set.route(&cap("state:assign"), &ScopeMap::default()),
            RouteOutcome::Unroutable
        );
        // Routing a non-exclusive capability does not apply.
        assert_eq!(
            set.route(&cap("state:report"), &frontend),
            RouteOutcome::NotExclusive
        );
    }

    #[test]
    fn ambiguity_defense_is_loud_never_first_match() {
        // Construct a corrupted set that validation would reject, to
        // prove routing defends in depth instead of first-matching.
        let overlapping = vec![
            orchestrator("lead-a", "area=*"),
            orchestrator("lead-b", "area=frontend"),
        ];
        let set = ValidatedProfileSet::new_unchecked(overlapping, registry());
        let frontend = ScopeMap::new(vec![(
            ScopeKey::new("area").unwrap(),
            ScopeValue::new("frontend").unwrap(),
        )])
        .unwrap();
        assert_eq!(
            set.route(&cap("state:assign"), &frontend),
            RouteOutcome::Ambiguous {
                first: name("lead-a"),
                second: name("lead-b")
            }
        );
    }

    #[test]
    fn redistribution_is_configuration_only() {
        let single = vec![orchestrator("solo-lead", "*")];
        assert!(validate_profiles(&single, &registry()).is_ok());
        let split = vec![
            orchestrator("front-lead", "area=frontend"),
            orchestrator("rest-lead", "area!=frontend"),
        ];
        assert!(validate_profiles(&split, &registry()).is_ok());
    }
}
