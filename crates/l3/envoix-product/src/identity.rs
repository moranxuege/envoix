use std::fmt;

use envoix_types::{ArtifactId, AttemptGen, RecordId, RequestId, TransferId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityError {
    EntropyUnavailable,
    GenerationExhausted,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntropyUnavailable => formatter.write_str("identity entropy is unavailable"),
            Self::GenerationExhausted => {
                formatter.write_str("attempt generation space is exhausted")
            }
        }
    }
}

impl std::error::Error for IdentityError {}

pub trait IdentitySource {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), IdentityError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemIdentitySource;

impl IdentitySource for SystemIdentitySource {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), IdentityError> {
        getrandom::fill(destination).map_err(|_| IdentityError::EntropyUnavailable)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProductIdentity {
    pub card: RecordId,
    pub transfer: TransferId,
    pub artifact: ArtifactId,
}

impl ProductIdentity {
    pub(crate) fn mint(
        source: &mut impl IdentitySource,
    ) -> Result<(Self, AttemptGen, RequestId), IdentityError> {
        let card = RecordId::new(mint_nonzero_u64(source)?);
        let transfer = TransferId::from_bytes(mint_nonzero_128(source)?);
        let artifact = ArtifactId::from_bytes(mint_nonzero_128(source)?);
        let receipt_request = RequestId::from_bytes(mint_nonzero_128(source)?);

        let mut generation = [0; 4];
        source.fill(&mut generation)?;
        // Leave half the space available for monotonic retries while retaining
        // an unpredictable, non-zero initial generation.
        let generation = u32::from_be_bytes(generation) & 0x7fff_ffff;
        let generation = AttemptGen::new(generation.max(1));

        Ok((
            Self {
                card,
                transfer,
                artifact,
            },
            generation,
            receipt_request,
        ))
    }
}

pub(crate) fn next_generation(current: AttemptGen) -> Result<AttemptGen, IdentityError> {
    current
        .get()
        .checked_add(1)
        .map(AttemptGen::new)
        .ok_or(IdentityError::GenerationExhausted)
}

fn mint_nonzero_u64(source: &mut impl IdentitySource) -> Result<u64, IdentityError> {
    let mut bytes = [0; 8];
    source.fill(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes).max(1))
}

fn mint_nonzero_128(source: &mut impl IdentitySource) -> Result<[u8; 16], IdentityError> {
    let mut bytes = [0; 16];
    source.fill(&mut bytes)?;
    if bytes == [0; 16] {
        bytes[15] = 1;
    }
    Ok(bytes)
}
