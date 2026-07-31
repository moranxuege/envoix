//! What a [`DutyKind::SourceHandle`] duty is allowed to answer.
//!
//! A bare [`OutcomeCode`] cannot carry this. `completed` says the platform did
//! something; it does not say whether the hold survives a restart, or whether
//! the source can be re-read from an offset — and those two facts are exactly
//! what decides whether the send streams from the provider or must copy first.
//! An acquisition that answered only `completed` is the defect `duty/2` exists
//! to close: the product would have had to invent the missing facts, and the
//! honest values it could invent are all wrong for some real provider.
//!
//! These types live here rather than beside the product lifecycle because the
//! PLATFORM answers them. The product stores the answer; it never authors one.
//!
//! [`DutyKind::SourceHandle`]: crate::DutyKind::SourceHandle

use envoix_types::SourceItemId;
use serde::{Deserialize, Serialize};

/// How long the PLATFORM's hold on the document lasts, exactly as the source
/// duty reported it.
///
/// **Never rewritten.** An earlier version of this model promoted `Process` to
/// `Persisted` once bytes were copied, which quietly changed the meaning of an
/// admitted duty result: an exact replay of the original
/// `source_acquired(Process)` would then look like a conflict with state that
/// had moved underneath it. What owns the bytes after staging is a separate
/// question with a separate answer (the product's `SourceBacking`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceRetention {
    /// Readable in THIS platform process only. Honest and usable: the transfer
    /// can proceed now, and a restart returns the card to awaiting selection.
    Process,
    /// The platform can reopen this after a restart.
    Persisted,
}

/// Whether the provider will serve the same bytes again from an offset.
///
/// Independent of retention, and kept apart from it for the reason the
/// retention/backing split exists: a grant that survives a restart says nothing
/// about whether the stream can be rewound, and a source that rewinds says
/// nothing about whether the grant outlives the process. Collapsing the two
/// into one word is what made the earlier model unable to explain its own
/// restore behaviour.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSeekability {
    /// Re-openable at an offset, so resume can continue rather than restart.
    Seekable,
    /// One pass only. Resume would have to re-read from zero, which is why a
    /// sequential source is copied instead of streamed.
    SequentialOnly,
}

/// Why an acquisition failed, in the vocabulary the PLATFORM can actually
/// speak.
///
/// Deliberately smaller than the product's prompt reasons. An acquisition duty
/// cannot answer `Initial` — it was asked, so this is not a first ask — and it
/// cannot answer `StagingFailed`, because staging had not started. Both are
/// product conclusions, and leaving them unrepresentable here is what stops an
/// adapter authoring one.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAcquisitionFailure {
    /// The platform could not read what was chosen.
    Unreadable,
    /// A grant that existed has gone — revoked, or the document moved.
    PermissionLost,
    /// Storage refused. Distinct from unreadable: the document was fine.
    StorageFault,
    /// The platform failed in a way it could not classify.
    Internal,
}

/// What the platform promises about ONE item of a selection.
///
/// Per item, never aggregated. Recovery is decided per document — a selection
/// whose first file is a persisted seekable provider and whose second is a
/// process-lifetime stream can resume the first and not the second — and one
/// pair of words for the whole selection cannot say that. An aggregate is
/// derivable from these; these are not derivable from an aggregate, which is
/// why the per-item answers are what the platform reports.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AcquiredItem {
    /// The item's ordinal in the accepted selection — the one the authority
    /// minted, not one the adapter chose.
    pub item: SourceItemId,
    pub retention: SourceRetention,
    pub seekability: SourceSeekability,
}

/// Every item of the selection, held.
///
/// ALL OF IT OR NONE, and there is deliberately no per-item failure: a person
/// chose a set, and silently sending a subset of what they chose is worse than
/// not sending. One unreadable document fails the whole acquisition through
/// [`SourceReport::Failed`], rather than leaving a hole in this list.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AcquiredSelection(Vec<AcquiredItem>);

/// Why a platform's answer does not describe the selection it was asked about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquiredSelectionError {
    /// No item at all. An acquisition that held nothing is a failure, not a
    /// success with an empty list.
    Empty,
    /// The ordinals are not the selection's, in the selection's order. An
    /// adapter answering out of order — or about an item that was never
    /// offered — is describing documents nobody asked about.
    NotTheSelection,
}

impl AcquiredSelection {
    /// The platform's answers for a selection of `expected` items.
    ///
    /// Checked against the ordinals the authority minted, in order, so a
    /// mismatched answer is refused where it arrives rather than being stored
    /// and puzzled over later.
    pub fn of(items: Vec<AcquiredItem>, expected: usize) -> Result<Self, AcquiredSelectionError> {
        if items.is_empty() || expected == 0 {
            return Err(AcquiredSelectionError::Empty);
        }
        if items.len() != expected
            || items
                .iter()
                .enumerate()
                .any(|(ordinal, item)| item.item.get() as usize != ordinal)
        {
            return Err(AcquiredSelectionError::NotTheSelection);
        }
        Ok(Self(items))
    }

    /// The answers for a selection of exactly one document.
    ///
    /// Its ordinal is 0 because a lone document is the first and only item of
    /// its selection — the same numbering [`Selection::of_one`] mints.
    ///
    /// [`Selection::of_one`]: https://docs.rs/envoix-product
    pub fn of_one(retention: SourceRetention, seekability: SourceSeekability) -> Self {
        Self(vec![AcquiredItem {
            item: SourceItemId::new(0),
            retention,
            seekability,
        }])
    }

    pub fn items(&self) -> &[AcquiredItem] {
        &self.0
    }

    /// The weakest retention any item promises. A selection survives a restart
    /// only if every one of its documents does.
    pub fn retention(&self) -> SourceRetention {
        if self
            .0
            .iter()
            .all(|item| item.retention == SourceRetention::Persisted)
        {
            SourceRetention::Persisted
        } else {
            SourceRetention::Process
        }
    }

    /// The weakest seekability any item promises, by the same rule.
    pub fn seekability(&self) -> SourceSeekability {
        if self
            .0
            .iter()
            .all(|item| item.seekability == SourceSeekability::Seekable)
        {
            SourceSeekability::Seekable
        } else {
            SourceSeekability::SequentialOnly
        }
    }
}

/// The platform's answer about one source acquisition.
///
/// Closed, so a source duty has exactly two things it can say. The product's
/// transition table is total over this enum rather than over an outcome code
/// whose other ten variants would each need a "cannot happen" arm.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceReport {
    /// The platform holds every document and will serve them under these terms.
    Acquired(AcquiredSelection),
    /// It does not, and will not without a fresh acquisition.
    Failed(SourceAcquisitionFailure),
}
