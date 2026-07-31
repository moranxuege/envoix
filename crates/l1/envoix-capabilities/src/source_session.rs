//! Reading the bytes of a source the platform holds.
//!
//! The other half of [`SourceReport`]. That says what the platform PROMISES
//! about a document — whether the hold survives a restart, whether it can be
//! re-read from an offset — and this is how the bytes behind those promises are
//! actually reached. Both are the same subject, so both live here.
//!
//! Positional, never sequential. Two readers of one platform descriptor share a
//! file offset, so a sequential read makes a second run's result depend on a
//! first run's progress; and the transfer machine reads its source positionally
//! anyway, so this is what it will be adapted to.
//!
//! Deliberately NOT `envoix-transfer`'s `SourceReader`, though it is the same
//! operation. That trait's error is the transfer machine's `StorageFault`, whose
//! vocabulary spans receive-staging operations this has nothing to do with — and
//! the runtime, which has to name the capability in order to hand it from the
//! resolver to the executor, may not depend on L2 at all. The adaptation is one
//! newtype in the composition root, which is where both vocabularies are already
//! in scope.
//!
//! [`SourceReport`]: crate::SourceReport

use envoix_types::ByteCount;

/// One open source, readable at an offset.
///
/// `Send` and not `Sync`: it is moved into the one attempt that reads it, and
/// reading needs `&mut self`. A session shared between attempts would be two
/// attempts sending one document, which no card can ask for.
pub trait SourceSession: Send {
    /// Reads at `offset`. A short read is normal and zero means end of source —
    /// the same contract the transfer machine's reader states, so adapting one
    /// to the other never has to reconcile two conventions.
    fn read_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, SourceReadError>;
}

/// A read that did not happen.
///
/// One opaque failure, on purpose. Every distinction a caller might draw —
/// revoked grant, closed descriptor, medium error — leads to the same place: the
/// source cannot be sent, the card fails with `SourceUnreadable`, and the person
/// chooses again. Naming the differences would be inventing product concepts
/// nothing acts on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceReadError;

impl std::fmt::Display for SourceReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("source could not be read")
    }
}

impl std::error::Error for SourceReadError {}
