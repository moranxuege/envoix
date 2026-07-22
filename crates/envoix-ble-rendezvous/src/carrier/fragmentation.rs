//! Bounded fragment assembly and disassembly for BLE GATT envelopes.
//!
//! Since BLE ATT MTU is typically 512 bytes or less, larger envelopes must be
//! fragmented across multiple GATT writes/notifications. This module handles
//! the fragmentation state, bounds-checking, and reassembly.

use crate::security::BleError;

use super::gatt_service::{MAX_ENVELOPE_SIZE, MAX_FRAGMENTS, MAX_FRAGMENT_SIZE};

/// A single fragment of an envelope, ready for GATT transmission.
#[derive(Clone, Debug)]
pub struct Fragment {
    /// Index of this fragment (0-based).
    pub index: u16,
    /// Whether this is the last fragment.
    pub is_last: bool,
    /// Payload bytes (≤ `MAX_FRAGMENT_SIZE`).
    pub data: Vec<u8>,
}

/// Disassembles an envelope into bounded fragments for GATT transfer.
pub fn fragment_envelope(envelope: &[u8]) -> Result<Vec<Fragment>, BleError> {
    let total_len = envelope.len() as u32;
    if total_len > MAX_ENVELOPE_SIZE {
        return Err(BleError::Protocol(format!(
            "envelope too large: {total_len} > {MAX_ENVELOPE_SIZE}"
        )));
    }

    let max_payload = MAX_FRAGMENT_SIZE as usize;
    let mut fragments = Vec::new();
    let mut offset = 0;
    let mut index = 0u16;

    while offset < envelope.len() {
        let end = (offset + max_payload).min(envelope.len());
        let is_last = end == envelope.len();
        fragments.push(Fragment {
            index,
            is_last,
            data: envelope[offset..end].to_vec(),
        });
        offset = end;
        index += 1;

        if index > MAX_FRAGMENTS {
            return Err(BleError::Protocol(format!(
                "envelope exceeds max fragments ({MAX_FRAGMENTS})"
            )));
        }
    }

    Ok(fragments)
}

/// Reassembles fragments into the complete envelope.
pub struct FragmentAssembler {
    buffers: Vec<Option<Vec<u8>>>,
    total_fragments: Option<u16>,
    received_size: u32,
}

impl FragmentAssembler {
    /// Create a new assembler for an expected number of fragments.
    pub fn new() -> Self {
        Self {
            buffers: Vec::new(),
            total_fragments: None,
            received_size: 0,
        }
    }

    /// Process an incoming fragment. Returns the complete envelope if this
    /// was the last fragment, or `None` if more fragments are expected.
    pub fn push(&mut self, fragment: Fragment) -> Result<Option<Vec<u8>>, BleError> {
        let idx = fragment.index as usize;

        // Ensure buffer capacity
        if idx >= self.buffers.len() {
            self.buffers.resize(idx + 1, None);
        }

        // Prevent duplicates
        if self.buffers[idx].is_some() {
            return Err(BleError::Protocol(format!(
                "duplicate fragment index {}",
                fragment.index
            )));
        }

        let len = fragment.data.len() as u32;
        self.received_size += len;
        if self.received_size > MAX_ENVELOPE_SIZE {
            return Err(BleError::Protocol("envelope exceeds maximum size".into()));
        }

        self.buffers[idx] = Some(fragment.data.clone());

        if fragment.is_last {
            self.total_fragments = Some(fragment.index + 1);
            // Check we have all fragments
            let total = self.total_fragments.unwrap() as usize;
            if self.buffers.len() != total {
                return Err(BleError::Protocol(format!(
                    "fragment index gap: expected {total} fragments, got indices up to {}",
                    self.buffers.len()
                )));
            }
            if self.buffers.iter().any(|b| b.is_none()) {
                return Err(BleError::Protocol("missing fragments".into()));
            }

            // Reassemble
            let mut envelope = Vec::with_capacity(self.received_size as usize);
            for buf in &self.buffers {
                if let Some(data) = buf {
                    envelope.extend_from_slice(data);
                }
            }
            Ok(Some(envelope))
        } else {
            Ok(None)
        }
    }
}

impl Default for FragmentAssembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_envelope_no_fragmentation() {
        let envelope = b"hello";
        let fragments = fragment_envelope(envelope).unwrap();
        assert_eq!(fragments.len(), 1);
        assert!(fragments[0].is_last);
        assert_eq!(fragments[0].data, envelope);
    }

    #[test]
    fn large_envelope_is_fragmented() {
        let envelope = vec![0xABu8; (MAX_FRAGMENT_SIZE as usize) * 3 + 1];
        let fragments = fragment_envelope(&envelope).unwrap();
        assert!(fragments.len() > 1);
        assert!(fragments.last().unwrap().is_last);
        assert!(!fragments[0].is_last);
    }

    #[test]
    fn reassembly_round_trips() {
        let envelope = vec![0xCDu8; 10_000];
        let fragments = fragment_envelope(&envelope).unwrap();
        let mut assembler = FragmentAssembler::new();
        let mut result = None;
        for frag in fragments {
            if let Some(complete) = assembler.push(frag).unwrap() {
                result = Some(complete);
                break;
            }
        }
        assert_eq!(result.unwrap(), envelope);
    }

    #[test]
    fn duplicate_fragment_rejected() {
        let fragments = fragment_envelope(b"test data").unwrap();
        let mut assembler = FragmentAssembler::new();
        assembler.push(fragments[0].clone()).unwrap();
        assert!(assembler.push(fragments[0].clone()).is_err());
    }

    #[test]
    fn oversized_envelope_rejected() {
        let envelope = vec![0u8; (MAX_ENVELOPE_SIZE + 1) as usize];
        assert!(fragment_envelope(&envelope).is_err());
    }
}
