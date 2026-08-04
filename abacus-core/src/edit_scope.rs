//! Normalized workspace paths and Assignment edit scopes.
//!
//! Acceptance checks the Handoff commit's changed paths against the
//! Assignment's edit scope at decision time (ADR-0001 §9.3): the check
//! is server-side policy, not a hook. Paths are normalized here so the
//! comparison is exact — no `.`/`..` segments, no absolute paths, no
//! trailing slashes.

use core::fmt;

/// Normalized repository-relative path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkPath(String);

/// Shape failures for workspace paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathError {
    Empty,
    Absolute,
    DotSegment,
    EmptySegment,
    InvalidCharacter,
}

impl WorkPath {
    pub fn new(raw: &str) -> Result<Self, PathError> {
        if raw.is_empty() {
            return Err(PathError::Empty);
        }
        if raw.starts_with('/') {
            return Err(PathError::Absolute);
        }
        if raw.bytes().any(|b| b == b'\\' || b.is_ascii_control()) {
            return Err(PathError::InvalidCharacter);
        }
        for segment in raw.split('/') {
            if segment.is_empty() {
                return Err(PathError::EmptySegment);
            }
            if segment == "." || segment == ".." {
                return Err(PathError::DotSegment);
            }
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when `self` equals `entry` or sits beneath it as a directory.
    fn is_covered_by(&self, entry: &WorkPath) -> bool {
        self == entry
            || (self.0.len() > entry.0.len()
                && self.0.as_bytes()[entry.0.len()] == b'/'
                && self.0.starts_with(&entry.0))
    }
}

impl fmt::Display for WorkPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The Assignment's allowed edit surface: exact files or directory roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditScope {
    entries: Vec<WorkPath>,
}

/// Edit-scope construction/conformance failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditScopeError {
    /// A scope with no entries can never accept a Handoff; refuse at
    /// construction rather than at decision time.
    Empty,
    /// Changed paths escaping the scope, enumerated for the refusal.
    OutOfScope(Vec<WorkPath>),
}

impl EditScope {
    pub fn new(entries: Vec<WorkPath>) -> Result<Self, EditScopeError> {
        if entries.is_empty() {
            return Err(EditScopeError::Empty);
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[WorkPath] {
        &self.entries
    }

    /// Every changed path must be covered; violations are enumerated.
    pub fn conforms(&self, changed: &[WorkPath]) -> Result<(), EditScopeError> {
        let violations: Vec<WorkPath> = changed
            .iter()
            .filter(|path| !self.entries.iter().any(|entry| path.is_covered_by(entry)))
            .cloned()
            .collect();
        if violations.is_empty() {
            Ok(())
        } else {
            Err(EditScopeError::OutOfScope(violations))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(raw: &str) -> WorkPath {
        WorkPath::new(raw).unwrap()
    }

    #[test]
    fn path_normalization_rules() {
        assert!(WorkPath::new("abacus-core/src/lib.rs").is_ok());
        assert_eq!(WorkPath::new(""), Err(PathError::Empty));
        assert_eq!(WorkPath::new("/etc/passwd"), Err(PathError::Absolute));
        assert_eq!(WorkPath::new("a//b"), Err(PathError::EmptySegment));
        assert_eq!(WorkPath::new("a/"), Err(PathError::EmptySegment));
        assert_eq!(WorkPath::new("./a"), Err(PathError::DotSegment));
        assert_eq!(WorkPath::new("a/../b"), Err(PathError::DotSegment));
        assert_eq!(WorkPath::new("a\\b"), Err(PathError::InvalidCharacter));
    }

    #[test]
    fn conformance_covers_exact_files_and_directories() {
        let scope = EditScope::new(vec![p("abacus-core/src"), p("docs/test-baselines.md")]).unwrap();
        assert_eq!(
            scope.conforms(&[p("abacus-core/src/lib.rs"), p("docs/test-baselines.md")]),
            Ok(())
        );
        // Prefix similarity without a directory boundary must not cover.
        let out = scope.conforms(&[p("abacus-core/src-extra/x.rs")]);
        assert_eq!(out, Err(EditScopeError::OutOfScope(vec![p("abacus-core/src-extra/x.rs")])));
    }

    #[test]
    fn violations_are_enumerated() {
        let scope = EditScope::new(vec![p("docs")]).unwrap();
        let result = scope.conforms(&[p("docs/a.md"), p("src/x.rs"), p("Cargo.toml")]);
        assert_eq!(
            result,
            Err(EditScopeError::OutOfScope(vec![p("src/x.rs"), p("Cargo.toml")]))
        );
    }

    #[test]
    fn empty_scope_is_refused_at_construction() {
        assert_eq!(EditScope::new(vec![]), Err(EditScopeError::Empty));
    }
}
