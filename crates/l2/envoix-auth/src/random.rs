use blake3::Hasher;
use envoix_pairing::EntropySource;
use rand_core::{CryptoRng, Error, RngCore};
use zeroize::{Zeroize, Zeroizing};

use crate::{AuthError, identifiers};

pub(crate) struct AuthRng {
    seed: Zeroizing<[u8; 32]>,
    block: Zeroizing<[u8; 32]>,
    block_offset: usize,
    counter: u64,
}

impl AuthRng {
    pub(crate) fn from_entropy(entropy: &mut impl EntropySource) -> Result<Self, AuthError> {
        let mut seed = Zeroizing::new([0_u8; 32]);
        entropy
            .fill(seed.as_mut())
            .map_err(|_| AuthError::EntropyUnavailable)?;

        Ok(Self {
            seed,
            block: Zeroizing::new([0_u8; 32]),
            block_offset: 32,
            counter: 0,
        })
    }

    fn refill(&mut self) {
        let mut hasher = Hasher::new_keyed(&self.seed);
        hasher.update(identifiers::SPAKE2_DOMAIN);
        hasher.update(&self.counter.to_be_bytes());
        self.block.copy_from_slice(hasher.finalize().as_bytes());
        self.block_offset = 0;
        self.counter = self.counter.wrapping_add(1);
    }
}

impl Drop for AuthRng {
    fn drop(&mut self) {
        self.counter.zeroize();
        self.block_offset.zeroize();
    }
}

impl RngCore for AuthRng {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0_u8; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0_u8; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        let mut written = 0;
        while written < dest.len() {
            if self.block_offset == self.block.len() {
                self.refill();
            }
            let available = self.block.len() - self.block_offset;
            let count = available.min(dest.len() - written);
            dest[written..written + count]
                .copy_from_slice(&self.block[self.block_offset..self.block_offset + count]);
            self.block_offset += count;
            written += count;
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for AuthRng {}
