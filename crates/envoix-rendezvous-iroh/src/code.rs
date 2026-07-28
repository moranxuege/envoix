//! Human pairing codes.
//!
//! A code looks like `7831-amber-comet`: the leading **nameplate** is the room
//! id (semi-public - it is sent to the broker to match the two peers), and the
//! **whole code** is the SPAKE2 password (never sent to the broker). Because the
//! word part carries entropy the broker never sees, the broker cannot derive the
//! shared key even though it knows the nameplate.

use std::fmt::Write as _;

/// Short, distinct words (>=4 chars) for the password part of a code.
const WORDS: &[&str] = &[
    "amber", "anchor", "apple", "arrow", "aspen", "azure", "basil", "beacon", "birch", "blaze",
    "brass", "bridge", "cabin", "cedar", "chant", "cliff", "clover", "comet", "coral", "crane",
    "delta", "ember", "fable", "falcon", "fern", "flint", "frost", "garnet", "glade", "grove",
    "harbor", "hazel", "indigo", "ivory", "jade", "kelp", "lantern", "lily", "lunar", "maple",
    "marble", "meadow", "nimbus", "ocean", "onyx", "opal", "orbit", "petal", "pine", "quartz",
    "raven", "reed", "river", "saffron", "sage", "slate", "spruce", "thistle", "tundra", "umber",
    "violet", "willow", "yarrow", "zephyr",
];

/// Largest room-id space for the nameplate. 6 digits keeps the code typeable
/// while making collisions between concurrent sessions rare; a colliding
/// nameplate only causes a failed pairing (SPAKE2 mismatch), never a wrong
/// pairing. Server-allocated nameplates would remove collisions entirely.
const NAMEPLATE_SPACE: u32 = 1_000_000;

/// Generate a pairing code with `words` random words after the nameplate.
pub fn generate_code(words: usize) -> Result<String, String> {
    let nameplate = random_u32()? % NAMEPLATE_SPACE;
    let mut code = format!("{nameplate:06}");
    for _ in 0..words.max(1) {
        let word = WORDS[random_index(WORDS.len())?];
        let _ = write!(code, "-{word}");
    }
    Ok(code)
}

/// Split a code into `(room_id, password)`: the room id is the nameplate (before
/// the first `-`, sent to the broker); the password is the whole code.
pub fn split_code(code: &str) -> (&str, &str) {
    let room_id = code.split('-').next().unwrap_or(code);
    (room_id, code)
}

fn random_u32() -> Result<u32, String> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(|_| "entropy source unavailable".to_string())?;
    Ok(u32::from_le_bytes(bytes))
}

/// A roughly uniform index in `0..len` (`len` <= u32::MAX). The slight modulo
/// bias is irrelevant for a 64-word list.
fn random_index(len: usize) -> Result<usize, String> {
    Ok(random_u32()? as usize % len)
}

#[cfg(test)]
#[path = "code_tests.rs"]
mod tests;
