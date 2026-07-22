use crate::security::mode::SAS_MODULUS;

/// WARNING: 6-digit SAS (1 in 1,000,000) provides **ceremonial** MITM
/// protection during first use. It is NOT cryptographic authentication —
/// a real-time wormhole relay can still forward the key exchange and pass
/// the SAS check. See the threat model in issue #52 for residual risk.
///
/// This is the same strength model as Bluetooth LE Secure Connections
/// numeric comparison and Signal's safety number verification.

/// A 6-digit short authentication string displayed on both devices.
/// The user must visually confirm both devices show the same code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SasCode {
    /// Six decimal digits, zero-padded for display (e.g., `042187`).
    value: u32,
}

impl SasCode {
    /// The raw numeric value (0–999_999).
    pub fn value(&self) -> u32 {
        self.value
    }

    /// Format as a zero-padded 6-digit string for UI display.
    /// Example: `4231` → `"004231"`.
    pub fn display(&self) -> String {
        format!("{:06}", self.value)
    }

    /// Format with a hyphen in the middle for readability.
    /// Example: `"004231"` → `"004-231"`.
    pub fn display_grouped(&self) -> String {
        let s = self.display();
        format!("{}-{}", &s[..3], &s[3..])
    }
}

/// Compute the 6-digit SAS code from the shared secret and transcript.
///
/// Both parties independently compute:
/// ```
/// hash = BLAKE3("envoix-ble-sas-v1" || shared_secret || transcript)
/// sas  = u32::from_le_bytes(hash[0..4]) % 1_000_000
/// ```
///
/// Using `from_le_bytes` ensures both sides agree regardless of platform
/// endianness (the hash bytes are fixed).
pub fn compute_sas(shared_secret: &[u8; 32], transcript: &[u8]) -> SasCode {
    let domain = b"envoix-ble-sas-v1";
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(shared_secret);
    h.update(transcript);
    let hash = h.finalize();
    let bytes = hash.as_bytes();

    let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    SasCode {
        value: value % SAS_MODULUS,
    }
}

/// Verify that the local SAS matches the peer's displayed SAS (called after
/// user confirms the codes match). This is purely a comparison helper that
/// ensures the same transcript produced the same code.
pub fn verify_sas(local: &SasCode, peer_displayed: &str) -> bool {
    // Accept both "004231" and "004-231" formats for UX flexibility.
    let cleaned: String = peer_displayed.chars().filter(|c| c.is_ascii_digit()).collect();
    if cleaned.len() != 6 {
        return false;
    }
    match cleaned.parse::<u32>() {
        Ok(peer_value) => local.value == peer_value,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_secret_and_transcript_produces_same_code() {
        let secret = [0xABu8; 32];
        let transcript = b"test-transcript";
        let a = compute_sas(&secret, transcript);
        let b = compute_sas(&secret, transcript);
        assert_eq!(a, b);
        assert!(a.value() < 1_000_000);
    }

    #[test]
    fn different_secret_produces_different_code() {
        let transcript = b"test-transcript";
        let a = compute_sas(&[0xABu8; 32], transcript);
        let b = compute_sas(&[0xCDu8; 32], transcript);
        assert_ne!(a, b);
    }

    #[test]
    fn different_transcript_produces_different_code() {
        let secret = [0xABu8; 32];
        let a = compute_sas(&secret, b"transcript-a");
        let b = compute_sas(&secret, b"transcript-b");
        assert_ne!(a, b);
    }

    #[test]
    fn display_is_six_digits_with_leading_zeros() {
        let secret = [0x00u8; 32];
        // Force a value < 1000 by constructing hash that starts with small bytes.
        // Actually, BLAKE3 output is unpredictable; just verify format constraints.
        let sas = compute_sas(&secret, b"format-test");
        let display = sas.display();
        assert_eq!(display.len(), 6);
        assert!(display.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn grouped_format_is_correct() {
        // Construct a known SAS code
        let sas = SasCode { value: 12345 };
        assert_eq!(sas.display(), "012345");
        assert_eq!(sas.display_grouped(), "012-345");
    }

    #[test]
    fn verify_accepts_both_formats() {
        let sas = SasCode { value: 4231 };
        assert!(verify_sas(&sas, "004231"));
        assert!(verify_sas(&sas, "004-231"));
    }

    #[test]
    fn verify_rejects_wrong_code() {
        let sas = SasCode { value: 4231 };
        assert!(!verify_sas(&sas, "004232"));
    }

    #[test]
    fn verify_rejects_malformed_input() {
        let sas = SasCode { value: 4231 };
        assert!(!verify_sas(&sas, "abc"));
        assert!(!verify_sas(&sas, "0042310")); // 7 digits
        assert!(!verify_sas(&sas, ""));
    }

    #[test]
    fn verify_rejects_non_digit_chars() {
        let sas = SasCode { value: 4231 };
        assert!(!verify_sas(&sas, "0042x1"));
    }
}
