//! Shared helpers for driving a card's source through its real lifecycle.
//!
//! Three test modules need this and none of them may take a shortcut: there is
//! no longer a way to declare a source ready without acquiring one, which is
//! the whole point of the change these support. Building an
//! [`AdmittedSourceResult`] in particular can only be done by passing a result
//! through a real [`DutyLedger`] — it has no public constructor — so the ledger
//! dance lives here once instead of in each suite.

use envoix_capabilities::{
    AcquiredSelection, Admission, Duty, DutyKind, DutyLedger, DutyProvenance, DutyReport,
    DutyResult, Registration, SourceAcquisitionKey, SourceReport, SourceRetention,
    SourceSeekability,
};
use envoix_protocol::ContentHash;
use envoix_types::{ByteCount, OfferedName};

use crate::{AcceptedSourceOffer, ProductInput, StagedContent, TransferContent, TransferRecord};

/// The document these suites hand a sending card, and what staging reads.
pub(crate) const STAGED_NAME: &str = "a.zip";
pub(crate) const STAGED_TOTAL: u64 = 100;

/// The acquisition this card is currently asking for.
fn acquisition(record: &TransferRecord) -> DutyProvenance {
    DutyProvenance {
        card: record.identity.card,
        generation: record.generation,
        request: record.source_request(),
    }
}

/// An offer against the acquisition the authority is currently asking for.
pub(crate) fn offer(
    record: &TransferRecord,
    name: &str,
    reported: Option<u64>,
) -> AcceptedSourceOffer {
    AcceptedSourceOffer::of_one_document(
        SourceAcquisitionKey::of(acquisition(record)),
        OfferedName::from_untrusted(name).expect("a bounded test name"),
        reported.map(ByteCount::new),
    )
}

/// The platform's answer, admitted exactly as the host admits one.
pub(crate) fn settled(record: &TransferRecord, report: SourceReport) -> ProductInput {
    let provenance = acquisition(record);
    let mut ledger = DutyLedger::new();
    ledger.advance_generation(provenance.card, provenance.generation);
    assert_eq!(
        ledger.register(Duty {
            provenance,
            kind: DutyKind::SourceHandle,
        }),
        Registration::Registered
    );
    let Admission::Fresh(admitted) = ledger.admit(DutyResult {
        provenance,
        report: DutyReport::Source(report),
    }) else {
        panic!("the ledger admits a fresh result for an outstanding source duty");
    };
    ProductInput::SourceSettled(
        admitted
            .into_source()
            .expect("a source duty answers a source report"),
    )
}

/// A grant that survives a restart on a source that can seek: the streaming
/// case, and the one that needs no copy.
pub(crate) fn acquired() -> SourceReport {
    SourceReport::Acquired(AcquiredSelection::of_one(
        SourceRetention::Persisted,
        SourceSeekability::Seekable,
    ))
}

/// What staging read: a name, a counted total, and which bytes they were.
pub(crate) fn staged(name: &str, total: u64) -> StagedContent {
    StagedContent::new(
        TransferContent::new(
            OfferedName::from_untrusted(name).expect("a bounded test name"),
            ByteCount::new(total),
        ),
        ContentHash::from_bytes([7; 32]),
    )
}

/// Walks a freshly created sender to `Staging`: the picker answers and the
/// platform acquires. Every suite that used to create a card already staging
/// goes through these two inputs instead.
pub(crate) fn give_a_source(record: &mut TransferRecord) {
    let offered = ProductInput::SourceOffered {
        offer: offer(record, STAGED_NAME, None),
    };
    record.reduce(offered).expect("the offer is accepted");
    let settlement = settled(record, acquired());
    record
        .reduce(settlement)
        .expect("the acquisition is applied");
}
