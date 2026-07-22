use zeroize::ZeroizeOnDrop;
use crate::security::BleError;

/// An ephemeral X25519 key pair. The secret is zeroed on drop.
pub struct EphemeralKeyPair {
    secret: x25519_dalek::EphemeralSecret,
    public: [u8; 32],
}

impl EphemeralKeyPair {
    /// Generate a fresh ephemeral X25519 key pair.
    pub fn generate() -> Result<Self, BleError> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|_| BleError::Entropy)?;
        let static_secret = x25519_dalek::StaticSecret::from(seed);
        let secret = x25519_dalek::EphemeralSecret::from(static_secret);
        let public = x25519_dalek::PublicKey::from(&secret).to_bytes();
        Ok(Self { secret, public })
    }

    /// The public key bytes to send to the peer.
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public
    }

    /// Perform X25519 Diffie-Hellman with the peer's public key.
    /// Returns the 32-byte shared secret.
    pub fn agree(self, peer_public: &[u8; 32]) -> [u8; 32] {
        let pk = x25519_dalek::PublicKey::from(*peer_public);
        let shared = self.secret.diffie_hellman(&pk);
        shared.to_bytes()
    }
}

impl ZeroizeOnDrop for EphemeralKeyPair {}

/// Validate a peer's ephemeral public key.
///
/// Rejects the identity element (all-zeros), which would produce a
/// predictable (zero) shared secret and break the SAS binding.
pub fn validate_peer_public(peer_public: &[u8; 32]) -> Result<(), BleError> {
    if peer_public.iter().all(|&b| b == 0) {
        return Err(BleError::InvalidPublicKey);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_key_pairs_agree_on_shared_secret() {
        let a = EphemeralKeyPair::generate().unwrap();
        let b = EphemeralKeyPair::generate().unwrap();

        let shared_a = a.agree(b.public_key());
        let shared_b = b.agree(a.public_key());

        assert_eq!(shared_a, shared_b);
        assert!(!shared_a.iter().all(|&b| b == 0));
    }

    #[test]
    fn different_pairs_produce_different_secrets() {
        let a1 = EphemeralKeyPair::generate().unwrap();
        let b1 = EphemeralKeyPair::generate().unwrap();
        let shared_1 = a1.agree(b1.public_key());

        let a2 = EphemeralKeyPair::generate().unwrap();
        let b2 = EphemeralKeyPair::generate().unwrap();
        let shared_2 = a2.agree(b2.public_key());

        assert_ne!(shared_1, shared_2);
    }

    #[test]
    fn public_key_is_32_bytes() {
        let kp = EphemeralKeyPair::generate().unwrap();
        assert_eq!(kp.public_key().len(), 32);
    }

    #[test]
    fn identity_public_key_rejected() {
        assert!(validate_peer_public(&[0u8; 32]).is_err());
    }

    #[test]
    fn valid_public_key_accepted() {
        let kp = EphemeralKeyPair::generate().unwrap();
        assert!(validate_peer_public(kp.public_key()).is_ok());
    }
}
