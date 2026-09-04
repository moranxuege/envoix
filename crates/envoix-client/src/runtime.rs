//! Application event replay.

use crate::event::EventEnvelope;
use crate::snapshot::{ApplyError, EngineSnapshot};

pub fn replay(
    mut snapshot: EngineSnapshot,
    events: impl IntoIterator<Item = EventEnvelope>,
) -> Result<EngineSnapshot, ApplyError> {
    snapshot.validate_contract()?;
    for event in events {
        snapshot.apply(event)?;
    }
    Ok(snapshot)
}
