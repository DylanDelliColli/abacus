//! Scope expressions: the ADR-0002 label-selector algebra.
//!
//! A scope expression is a disjunction of selectors over repository-
//! declared keys, evaluated against a subject's normalized single-valued
//! scope map. Everything here is pure: parsing, canonicalization,
//! matching, conservative disjointness ("overlap unless contradiction"),
//! and conservative containment ("contained only when provable").
//! Single-valuedness of [`ScopeMap`] is the precondition that keeps
//! disjointness sound (ADR-0002 §1); the constructor enforces it.
//!
//! Bounds (ADR-0002 §2): keys ≤ 15 chars, values ≤ 34 (so an encoded
//! `key:value` provider label fits 50), ≤ 8 selectors per expression,
//! ≤ 8 atoms per selector.

use core::fmt;
use std::collections::BTreeMap;

pub const MAX_KEY_LEN: usize = 15;
pub const MAX_VALUE_LEN: usize = 34;
pub const MAX_SELECTORS: usize = 8;
pub const MAX_ATOMS: usize = 8;

/// Distinct scope-algebra failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeError {
    Empty,
    BadKey(String),
    UndeclaredKey(String),
    BadValue(String),
    BadAtom(String),
    TooManySelectors,
    TooManyAtoms,
    /// A selector no subject can satisfy (`k=a & k=b`, `k=a & k!=a`):
    /// an authored defect, rejected rather than ignored.
    UnsatisfiableSelector(String),
    /// Two declared-key labels bind the same key (maps to the
    /// `scope-label-conflict` refusal at the work seam).
    DuplicateKey(String),
}

/// A declared scope key: `[a-z][a-z0-9-]*`, bounded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeKey(String);

impl ScopeKey {
    pub fn new(raw: &str) -> Result<Self, ScopeError> {
        let ok = !raw.is_empty()
            && raw.len() <= MAX_KEY_LEN
            && raw.as_bytes()[0].is_ascii_lowercase()
            && raw
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(Self(raw.to_owned()))
        } else {
            Err(ScopeError::BadKey(raw.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A scope value: `[a-z0-9_-]+`, provider-representable, bounded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeValue(String);

impl ScopeValue {
    pub fn new(raw: &str) -> Result<Self, ScopeError> {
        let ok = !raw.is_empty()
            && raw.len() <= MAX_VALUE_LEN
            && raw
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if ok {
            Ok(Self(raw.to_owned()))
        } else {
            Err(ScopeError::BadValue(raw.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A subject's normalized scope facts: at most one value per key,
/// enforced at construction (the single-valuedness precondition).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeMap(BTreeMap<ScopeKey, ScopeValue>);

impl ScopeMap {
    pub fn new(pairs: Vec<(ScopeKey, ScopeValue)>) -> Result<Self, ScopeError> {
        let mut map = BTreeMap::new();
        for (key, value) in pairs {
            if map.insert(key.clone(), value).is_some() {
                return Err(ScopeError::DuplicateKey(key.as_str().to_owned()));
            }
        }
        Ok(Self(map))
    }

    pub fn get(&self, key: &ScopeKey) -> Option<&ScopeValue> {
        self.0.get(key)
    }

    /// Key-ordered pairs, for snapshotting into durable records.
    pub fn pairs(&self) -> impl Iterator<Item = (&ScopeKey, &ScopeValue)> {
        self.0.iter()
    }
}

/// One atom over a key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Atom {
    Eq(ScopeKey, ScopeValue),
    Ne(ScopeKey, ScopeValue),
    Present(ScopeKey),
}

impl Atom {
    /// Fixed absent-key semantics (ADR-0002 §2): `k=v` and `k=*` are
    /// false on absence; `k!=v` is true on absence.
    fn matches(&self, map: &ScopeMap) -> bool {
        match self {
            Atom::Eq(k, v) => map.get(k) == Some(v),
            Atom::Ne(k, v) => map.get(k) != Some(v),
            Atom::Present(k) => map.get(k).is_some(),
        }
    }

    /// `self` implies `other` (ADR-0002 §5 implication table).
    fn implies(&self, other: &Atom) -> bool {
        match (self, other) {
            (Atom::Eq(k1, v1), Atom::Eq(k2, v2)) => k1 == k2 && v1 == v2,
            (Atom::Eq(k1, _), Atom::Present(k2)) => k1 == k2,
            (Atom::Eq(k1, v1), Atom::Ne(k2, v2)) => k1 == k2 && v1 != v2,
            (Atom::Ne(k1, v1), Atom::Ne(k2, v2)) => k1 == k2 && v1 == v2,
            (Atom::Present(k1), Atom::Present(k2)) => k1 == k2,
            _ => false,
        }
    }

    /// Jointly unsatisfiable with `other` — exactly two contradiction
    /// forms exist (ADR-0002 §4).
    fn contradicts(&self, other: &Atom) -> bool {
        match (self, other) {
            (Atom::Eq(k1, v1), Atom::Eq(k2, v2)) => k1 == k2 && v1 != v2,
            (Atom::Eq(k1, v1), Atom::Ne(k2, v2)) | (Atom::Ne(k2, v2), Atom::Eq(k1, v1)) => {
                k1 == k2 && v1 == v2
            }
            _ => false,
        }
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Atom::Eq(k, v) => write!(f, "{}={}", k.as_str(), v.as_str()),
            Atom::Ne(k, v) => write!(f, "{}!={}", k.as_str(), v.as_str()),
            Atom::Present(k) => write!(f, "{}=*", k.as_str()),
        }
    }
}

/// A conjunction of atoms, canonicalized (deduplicated, sorted).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Selector(Vec<Atom>);

impl Selector {
    fn canonicalize(mut atoms: Vec<Atom>) -> Result<Self, ScopeError> {
        atoms.sort_by_key(|a| a.to_string());
        atoms.dedup();
        if atoms.len() > MAX_ATOMS {
            return Err(ScopeError::TooManyAtoms);
        }
        for (i, a) in atoms.iter().enumerate() {
            for b in &atoms[i + 1..] {
                if a.contradicts(b) {
                    let text = atoms
                        .iter()
                        .map(Atom::to_string)
                        .collect::<Vec<_>>()
                        .join(" & ");
                    return Err(ScopeError::UnsatisfiableSelector(text));
                }
            }
        }
        Ok(Self(atoms))
    }

    fn matches(&self, map: &ScopeMap) -> bool {
        self.0.iter().all(|a| a.matches(map))
    }

    /// Provably disjoint from `other`: some key pair contradicts.
    fn disjoint(&self, other: &Selector) -> bool {
        self.0
            .iter()
            .any(|a| other.0.iter().any(|b| a.contradicts(b)))
    }

    /// `self` is contained in `other`: every atom of `other` is implied
    /// by some atom of `self` (conjunction weakening).
    fn contained_in(&self, other: &Selector) -> bool {
        other.0.iter().all(|t| self.0.iter().any(|s| s.implies(t)))
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            &self
                .0
                .iter()
                .map(Atom::to_string)
                .collect::<Vec<_>>()
                .join(" & "),
        )
    }
}

/// A validated scope expression in canonical form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ScopeExpr {
    Universal,
    Selectors(Vec<Selector>),
}

impl ScopeExpr {
    /// Whitespace-tolerant parse against the repository's declared
    /// keys. When no keys are declared, only `*` is valid.
    pub fn parse(text: &str, declared: &[ScopeKey]) -> Result<Self, ScopeError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(ScopeError::Empty);
        }
        if trimmed == "*" {
            return Ok(Self::Universal);
        }
        let mut selectors = Vec::new();
        for selector_text in trimmed.split('|') {
            let mut atoms = Vec::new();
            for atom_text in selector_text.split('&') {
                atoms.push(Self::parse_atom(atom_text.trim(), declared)?);
            }
            selectors.push(Selector::canonicalize(atoms)?);
        }
        selectors.sort_by_key(|s| s.to_string());
        selectors.dedup();
        if selectors.len() > MAX_SELECTORS {
            return Err(ScopeError::TooManySelectors);
        }
        Ok(Self::Selectors(selectors))
    }

    fn parse_atom(text: &str, declared: &[ScopeKey]) -> Result<Atom, ScopeError> {
        if text.is_empty() {
            return Err(ScopeError::BadAtom(text.to_owned()));
        }
        let (key_text, op_ne, value_text) = if let Some((k, v)) = text.split_once("!=") {
            (k.trim(), true, v.trim())
        } else if let Some((k, v)) = text.split_once('=') {
            (k.trim(), false, v.trim())
        } else {
            return Err(ScopeError::BadAtom(text.to_owned()));
        };
        let key = ScopeKey::new(key_text)?;
        if !declared.contains(&key) {
            return Err(ScopeError::UndeclaredKey(key_text.to_owned()));
        }
        if !op_ne && value_text == "*" {
            return Ok(Atom::Present(key));
        }
        let value = ScopeValue::new(value_text)?;
        Ok(if op_ne {
            Atom::Ne(key, value)
        } else {
            Atom::Eq(key, value)
        })
    }

    /// The canonical serialization: comparison, hashing, and durable
    /// records use exactly this form (byte-equality is expression
    /// equality).
    pub fn canonical(&self) -> String {
        match self {
            Self::Universal => "*".to_owned(),
            Self::Selectors(selectors) => selectors
                .iter()
                .map(Selector::to_string)
                .collect::<Vec<_>>()
                .join(" | "),
        }
    }

    /// Membership over a normalized scope map.
    pub fn matches(&self, map: &ScopeMap) -> bool {
        match self {
            Self::Universal => true,
            Self::Selectors(selectors) => selectors.iter().any(|s| s.matches(map)),
        }
    }

    /// Conservative disjointness: overlap unless every selector pair
    /// carries a literal contradiction. `*` overlaps everything.
    pub fn disjoint(&self, other: &ScopeExpr) -> bool {
        match (self, other) {
            (Self::Universal, _) | (_, Self::Universal) => false,
            (Self::Selectors(a), Self::Selectors(b)) => {
                a.iter().all(|sa| b.iter().all(|sb| sa.disjoint(sb)))
            }
        }
    }

    /// Conservative containment: `self` contains `other` only when
    /// provable. `*` contains everything; only `*` contains `*`.
    pub fn contains(&self, other: &ScopeExpr) -> bool {
        match (self, other) {
            (Self::Universal, _) => true,
            (_, Self::Universal) => false,
            (Self::Selectors(grant), Self::Selectors(subject)) => subject
                .iter()
                .all(|s| grant.iter().any(|g| s.contained_in(g))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<ScopeKey> {
        vec![
            ScopeKey::new("area").unwrap(),
            ScopeKey::new("epic").unwrap(),
        ]
    }

    fn expr(text: &str) -> ScopeExpr {
        ScopeExpr::parse(text, &keys()).unwrap()
    }

    fn map(pairs: &[(&str, &str)]) -> ScopeMap {
        ScopeMap::new(
            pairs
                .iter()
                .map(|(k, v)| (ScopeKey::new(k).unwrap(), ScopeValue::new(v).unwrap()))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn parse_and_canonicalize() {
        assert_eq!(expr("*").canonical(), "*");
        assert_eq!(expr("  area=frontend  ").canonical(), "area=frontend");
        // Atoms and selectors are sorted and deduplicated.
        assert_eq!(
            expr("epic=abacus-9nh & area!=docs | area=frontend | area=frontend").canonical(),
            "area!=docs & epic=abacus-9nh | area=frontend"
        );
        // Whitespace tolerance round-trips through the canonical form.
        let canonical = expr("area=frontend|area=design").canonical();
        assert_eq!(
            ScopeExpr::parse(&canonical, &keys()).unwrap().canonical(),
            canonical
        );
    }

    #[test]
    fn parse_rejections() {
        let declared = keys();
        assert_eq!(ScopeExpr::parse("", &declared), Err(ScopeError::Empty));
        assert!(matches!(
            ScopeExpr::parse("tier=ui", &declared),
            Err(ScopeError::UndeclaredKey(k)) if k == "tier"
        ));
        assert!(matches!(
            ScopeExpr::parse("area=Front", &declared),
            Err(ScopeError::BadValue(_))
        ));
        assert!(matches!(
            ScopeExpr::parse("area=a.b", &declared),
            Err(ScopeError::BadValue(_))
        ));
        assert!(matches!(
            ScopeExpr::parse("area", &declared),
            Err(ScopeError::BadAtom(_))
        ));
        // Both unsatisfiable-selector forms.
        assert!(matches!(
            ScopeExpr::parse("area=a & area=b", &declared),
            Err(ScopeError::UnsatisfiableSelector(_))
        ));
        assert!(matches!(
            ScopeExpr::parse("area=a & area!=a", &declared),
            Err(ScopeError::UnsatisfiableSelector(_))
        ));
        // With no declared keys, only `*` parses.
        assert!(ScopeExpr::parse("*", &[]).is_ok());
        assert!(ScopeExpr::parse("area=a", &[]).is_err());
    }

    #[test]
    fn bounds_are_enforced() {
        let declared = keys();
        let too_many_selectors = (0..9)
            .map(|i| format!("area=v{i}"))
            .collect::<Vec<_>>()
            .join(" | ");
        assert_eq!(
            ScopeExpr::parse(&too_many_selectors, &declared),
            Err(ScopeError::TooManySelectors)
        );
        assert!(ScopeKey::new("a-very-long-scope-key").is_err());
        assert!(ScopeValue::new(&"v".repeat(35)).is_err());
        assert!(ScopeValue::new(&"v".repeat(34)).is_ok());
    }

    #[test]
    fn matching_follows_the_absent_key_table() {
        let m = map(&[("area", "frontend")]);
        let empty = map(&[]);
        assert!(expr("*").matches(&m) && expr("*").matches(&empty));
        assert!(expr("area=frontend").matches(&m));
        assert!(!expr("area=frontend").matches(&empty));
        assert!(expr("area=*").matches(&m));
        assert!(!expr("area=*").matches(&empty));
        assert!(!expr("area!=frontend").matches(&m));
        // Absence satisfies inequality: the catch-all-rest property.
        assert!(expr("area!=frontend").matches(&empty));
        assert!(expr("area=frontend | epic=abacus-x").matches(&m));
        assert!(!expr("area=frontend & epic=abacus-x").matches(&m));
    }

    #[test]
    fn scope_map_rejects_duplicates() {
        let dup = ScopeMap::new(vec![
            (
                ScopeKey::new("area").unwrap(),
                ScopeValue::new("a").unwrap(),
            ),
            (
                ScopeKey::new("area").unwrap(),
                ScopeValue::new("b").unwrap(),
            ),
        ]);
        assert_eq!(dup, Err(ScopeError::DuplicateKey("area".into())));
    }

    #[test]
    fn disjointness_is_conservative() {
        // Literal partition: provably disjoint.
        assert!(expr("area=frontend").disjoint(&expr("area=backend")));
        // Negation partition: provably disjoint.
        assert!(expr("area=frontend").disjoint(&expr("area!=frontend")));
        // Different keys: not provable, treated as overlapping.
        assert!(!expr("area=frontend").disjoint(&expr("epic=abacus-x")));
        // `*` overlaps everything.
        assert!(!expr("*").disjoint(&expr("area=frontend")));
        // Presence overlaps any same-key literal.
        assert!(!expr("area=*").disjoint(&expr("area=frontend")));
        // Every selector pair must contradict.
        assert!(expr("area=a | area=b").disjoint(&expr("area=c")));
        assert!(!expr("area=a | epic=e").disjoint(&expr("area=c")));
    }

    #[test]
    fn containment_is_conservative() {
        assert!(expr("*").contains(&expr("area=frontend")));
        assert!(!expr("area=*").contains(&expr("*")));
        assert!(expr("*").contains(&expr("*")));
        // Eq is inside Present, inside Ne-of-other, and inside itself.
        assert!(expr("area=*").contains(&expr("area=frontend")));
        assert!(expr("area!=docs").contains(&expr("area=frontend")));
        assert!(expr("area=frontend").contains(&expr("area=frontend")));
        // A conjunction is inside its weakening.
        assert!(expr("area=frontend").contains(&expr("area=frontend & epic=abacus-x")));
        // Not the converse.
        assert!(!expr("area=frontend & epic=abacus-x").contains(&expr("area=frontend")));
        // Disjunction on the grant side covers each branch.
        assert!(expr("area=frontend | area=design").contains(&expr("area=design")));
        // Conservative: true containment that is not selector-provable
        // is refused (area=a is semantically inside area=a | epic=*,
        // and provable; but a subject disjunction needs every branch).
        assert!(!expr("area=frontend").contains(&expr("area=frontend | area=design")));
        assert!(!expr("area=frontend").contains(&expr("area!=docs")));
    }
}
