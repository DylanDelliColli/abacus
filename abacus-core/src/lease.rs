//! Leases and fencing tokens.
//!
//! A Lease is time-bounded exclusivity held by the current Attempt of an
//! Assignment (CONTEXT §2). Fencing tokens are monotonic across that
//! Assignment's Attempts; every mutating worker call presents its token,
//! and a stale token fails the call loudly (core invariant 4). Time is
//! an input (I13): core compares caller-supplied instants and never
//! reads a clock.

/// Caller-supplied instant, opaque to core beyond ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub u64);

/// Monotonic per-Assignment fencing token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FencingToken(pub u64);

impl FencingToken {
    /// Token for the next fenced Attempt of the same Assignment.
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Time-bounded exclusivity for the current Attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub token: FencingToken,
    pub expires_at: Timestamp,
}

impl Lease {
    /// Expiry is a derived fact: strictly after the deadline. An expired
    /// lease makes the Attempt *reclaimable*; nothing reassigns silently.
    pub fn is_expired(&self, now: Timestamp) -> bool {
        now > self.expires_at
    }
}

/// Distinct fencing failures (normalized error categories).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FencingError {
    /// Presented token is older than the current one: a replaced or
    /// expired Attempt trying to mutate current state (invariant 4).
    StaleToken,
    /// Presented token is ahead of the current one: impossible under
    /// monotonic issuance, so it is corrupt input, refused loudly.
    UnknownToken,
}

/// Validate a presented fencing token against the current one.
pub fn validate_fencing(
    current: FencingToken,
    presented: FencingToken,
) -> Result<(), FencingError> {
    match presented.cmp(&current) {
        core::cmp::Ordering::Equal => Ok(()),
        core::cmp::Ordering::Less => Err(FencingError::StaleToken),
        core::cmp::Ordering::Greater => Err(FencingError::UnknownToken),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_monotonic() {
        let t0 = FencingToken(1);
        let t1 = t0.next();
        assert!(t1 > t0);
        assert_eq!(t1, FencingToken(2));
    }

    #[test]
    fn fencing_accepts_only_the_current_token() {
        let current = FencingToken(3);
        assert_eq!(validate_fencing(current, FencingToken(3)), Ok(()));
        assert_eq!(
            validate_fencing(current, FencingToken(2)),
            Err(FencingError::StaleToken)
        );
        assert_eq!(
            validate_fencing(current, FencingToken(4)),
            Err(FencingError::UnknownToken)
        );
    }

    #[test]
    fn expiry_is_strictly_after_deadline() {
        let lease = Lease {
            token: FencingToken(1),
            expires_at: Timestamp(100),
        };
        assert!(!lease.is_expired(Timestamp(99)));
        assert!(!lease.is_expired(Timestamp(100)));
        assert!(lease.is_expired(Timestamp(101)));
    }
}
