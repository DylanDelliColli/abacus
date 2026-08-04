//! The two first-class authority classes.
//!
//! Exactly two exist (CONTEXT §2, binding operator decision 6):
//! specialist behaviors are profiles or skills layered on a class,
//! never new variants. Core knows no named agent and assumes no
//! singleton orchestrator (CONTEXT I16).

/// The only role taxonomy core knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthorityClass {
    /// Coordinates and decides: assignments, Directives, acceptance.
    Orchestrator,
    /// Executes and submits: Reports, Evidence, Handoffs.
    Worker,
}

impl AuthorityClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::Worker => "worker",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names() {
        assert_eq!(AuthorityClass::Orchestrator.as_str(), "orchestrator");
        assert_eq!(AuthorityClass::Worker.as_str(), "worker");
    }
}
