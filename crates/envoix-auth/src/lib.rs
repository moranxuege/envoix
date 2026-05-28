//! Pairing and peer-authentication configuration.

use envoix_error::CoreError;

/// Minimum accepted shared-token length for prototype pairing.
pub const MIN_SHARED_TOKEN_LEN: usize = 12;

/// Error type returned by pairing authentication.
pub type AuthError = CoreError;

/// Pairing method selected for a session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingConfig {
    /// Experimental SPAKE2 pairing using a shared ASCII token.
    Spake2SharedToken {
        /// Shared token known to both peers.
        token: String,
    },
}

impl PairingConfig {
    /// Creates a validated experimental SPAKE2 shared-token config.
    pub fn spake2_shared_token(token: impl Into<String>) -> Result<Self, AuthError> {
        let config = Self::Spake2SharedToken {
            token: token.into(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Validates pairing config invariants that are independent of transport.
    pub fn validate(&self) -> Result<(), AuthError> {
        match self {
            Self::Spake2SharedToken { token } => validate_shared_token(token),
        }
    }
}

fn validate_shared_token(token: &str) -> Result<(), AuthError> {
    if !token.is_ascii() {
        return Err(CoreError::InvalidInput(
            "SPAKE2 shared token must be ASCII".into(),
        ));
    }
    if token.len() < MIN_SHARED_TOKEN_LEN {
        return Err(CoreError::InvalidInput(format!(
            "SPAKE2 shared token must be at least {MIN_SHARED_TOKEN_LEN} ASCII bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ascii_token_at_minimum_length() {
        let config = PairingConfig::spake2_shared_token("abcdefghijkl").unwrap();

        assert_eq!(
            config,
            PairingConfig::Spake2SharedToken {
                token: "abcdefghijkl".into()
            }
        );
    }

    #[test]
    fn rejects_short_token() {
        let error = PairingConfig::spake2_shared_token("short").unwrap_err();

        assert!(matches!(error, CoreError::InvalidInput(_)));
    }

    #[test]
    fn rejects_non_ascii_token() {
        let error = PairingConfig::spake2_shared_token("abcdefghijklé").unwrap_err();

        assert!(matches!(error, CoreError::InvalidInput(_)));
    }
}
