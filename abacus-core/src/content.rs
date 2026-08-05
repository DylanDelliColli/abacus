//! Content-addressed value types: hashes, commits, and workspace digests.
//!
//! Evidence records outcomes bound to artifacts (I4): commit identity
//! and content digests are how every binding is expressed. Core
//! validates shape only; producing a hash is I/O and lives with the
//! composer behind ports.

use core::fmt;

/// Shape failures for content-addressed values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentError {
    Empty,
    BadLength,
    NotHex,
}

fn validate_hex(raw: &str, allowed_lens: &[usize]) -> Result<(), ContentError> {
    if raw.is_empty() {
        return Err(ContentError::Empty);
    }
    if !allowed_lens.contains(&raw.len()) {
        return Err(ContentError::BadLength);
    }
    if !raw
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ContentError::NotHex);
    }
    Ok(())
}

macro_rules! hex_value {
    ($(#[$doc:meta])* $name:ident, $lens:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(raw: &str) -> Result<Self, ContentError> {
                validate_hex(raw, &$lens)?;
                Ok(Self(raw.to_owned()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

hex_value! {
    /// 64-hex content hash (bead content, profile content, file digests).
    ContentHash, [64usize]
}
hex_value! {
    /// Git commit identity: 40-hex (SHA-1) or 64-hex (SHA-256 repos).
    CommitId, [40usize, 64usize]
}
hex_value! {
    /// Whole-workspace digest captured before/after a verification run.
    WorkspaceDigest, [64usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    const H64: &str = "255fe0badd0cc75fc00fae15315f969c38a34ee0477c844d55fb1915671f0aa8";
    const H40: &str = "7b917e1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn accepts_valid_lengths() {
        assert!(ContentHash::new(H64).is_ok());
        assert!(WorkspaceDigest::new(H64).is_ok());
        assert!(CommitId::new(H40).is_ok());
        assert!(CommitId::new(H64).is_ok());
    }

    #[test]
    fn rejects_bad_shapes() {
        assert_eq!(ContentHash::new(""), Err(ContentError::Empty));
        assert_eq!(ContentHash::new(H40), Err(ContentError::BadLength));
        assert_eq!(CommitId::new(&H40[..39]), Err(ContentError::BadLength));
        let upper = H64.to_uppercase();
        assert_eq!(ContentHash::new(&upper), Err(ContentError::NotHex));
        let bad = format!("{}g", &H64[..63]);
        assert_eq!(ContentHash::new(&bad), Err(ContentError::NotHex));
    }
}
