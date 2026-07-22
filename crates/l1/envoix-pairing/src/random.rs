use std::fmt;

use rand_core::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use crate::PairingError;
use crate::identifiers::SPAKE2_DOMAIN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntropyError {
    Unavailable,
}

impl fmt::Display for EntropyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("entropy source unavailable")
    }
}

impl std::error::Error for EntropyError {}

pub trait EntropySource {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemEntropy;

impl EntropySource for SystemEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError> {
        getrandom::fill(destination).map_err(|_| EntropyError::Unavailable)
    }
}

pub(crate) struct SpakeRng {
    seed: Zeroizing<[u8; 32]>,
    block: Zeroizing<[u8; 32]>,
    block_offset: usize,
    counter: u64,
}

impl SpakeRng {
    pub(crate) fn from_entropy(source: &mut impl EntropySource) -> Result<Self, PairingError> {
        let mut seed = Zeroizing::new([0; 32]);
        source
            .fill(&mut seed[..])
            .map_err(|_| PairingError::EntropyUnavailable)?;
        Ok(Self {
            seed,
            block: Zeroizing::new([0; 32]),
            block_offset: 32,
            counter: 0,
        })
    }

    fn refill(&mut self) {
        let mut hasher = blake3::Hasher::new_keyed(&self.seed);
        hasher.update(SPAKE2_DOMAIN);
        hasher.update(&self.counter.to_be_bytes());
        self.block.copy_from_slice(hasher.finalize().as_bytes());
        self.block_offset = 0;
        self.counter = self.counter.wrapping_add(1);
    }
}

impl RngCore for SpakeRng {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    fn fill_bytes(&mut self, destination: &mut [u8]) {
        let mut written = 0;
        while written < destination.len() {
            if self.block_offset == self.block.len() {
                self.refill();
            }
            let available = self.block.len() - self.block_offset;
            let count = available.min(destination.len() - written);
            destination[written..written + count]
                .copy_from_slice(&self.block[self.block_offset..self.block_offset + count]);
            self.block_offset += count;
            written += count;
        }
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(destination);
        Ok(())
    }
}

impl CryptoRng for SpakeRng {}
