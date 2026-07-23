use std::fmt;

use crate::InviteError;
use crate::identifiers::ROOM_CODE_NAMESPACE_PREFIX;

pub const MAX_ROOM_CODE_LENGTH: usize = 64;
const NAMEPLATE_SPACE: u32 = 1_000_000;
const RANDOM_DRAWS: usize = 3;
const MAX_REJECTION_DRAWS: usize = 32;

const WORDS: &[&str] = &[
    "amber", "anchor", "apple", "arrow", "aspen", "azure", "basil", "beacon", "birch", "blaze",
    "brass", "bridge", "cabin", "cedar", "chant", "cliff", "clover", "comet", "coral", "crane",
    "delta", "ember", "fable", "falcon", "fern", "flint", "frost", "garnet", "glade", "grove",
    "harbor", "hazel", "indigo", "ivory", "jade", "kelp", "lantern", "lily", "lunar", "maple",
    "marble", "meadow", "nimbus", "ocean", "onyx", "opal", "orbit", "petal", "pine", "quartz",
    "raven", "reed", "river", "saffron", "sage", "slate", "spruce", "thistle", "tundra", "umber",
    "violet", "willow", "yarrow", "zephyr",
];

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

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct RoomCode(String);

impl RoomCode {
    pub fn parse(code: impl Into<String>) -> Result<Self, InviteError> {
        let code = code.into();
        if !is_canonical_room_code(&code) {
            return Err(InviteError::InvalidField(crate::InviteField::Code));
        }
        Ok(Self(code))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn namespaced_key(&self) -> NamespacedRoomKey {
        let nameplate = self.0.split('-').next().unwrap_or("");
        // The word entropy is the SPAKE2 password. Only the semi-public
        // nameplate may cross the broker boundary.
        NamespacedRoomKey(format!("{ROOM_CODE_NAMESPACE_PREFIX}{nameplate}"))
    }
}

impl fmt::Debug for RoomCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RoomCode([redacted])")
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct NamespacedRoomKey(String);

impl NamespacedRoomKey {
    pub fn parse(key: impl Into<String>) -> Result<Self, InviteError> {
        let key = key.into();
        let Some(nameplate) = key.strip_prefix(ROOM_CODE_NAMESPACE_PREFIX) else {
            return Err(InviteError::InvalidField(crate::InviteField::Code));
        };
        if !is_six_digits(nameplate) {
            return Err(InviteError::InvalidField(crate::InviteField::Code));
        }
        Ok(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NamespacedRoomKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NamespacedRoomKey([redacted])")
    }
}

pub fn generate_room_code(source: &mut impl EntropySource) -> Result<RoomCode, InviteError> {
    let mut values = [0; RANDOM_DRAWS];
    values[0] = sample_below(source, NAMEPLATE_SPACE)?;
    values[1] = sample_below(source, WORDS.len() as u32)?;
    values[2] = sample_below(source, WORDS.len() as u32)?;
    RoomCode::parse(format!(
        "{:06}-{}-{}",
        values[0], WORDS[values[1] as usize], WORDS[values[2] as usize]
    ))
}

pub(crate) fn looks_like_bare_room_code(input: &str) -> bool {
    if is_six_digits(input) {
        return true;
    }
    let mut parts = input.split('-');
    let Some(nameplate) = parts.next() else {
        return false;
    };
    let Some(first_word) = parts.next() else {
        return false;
    };
    let Some(second_word) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && is_six_digits(nameplate)
        && is_word(first_word)
        && is_word(second_word)
}

fn is_canonical_room_code(code: &str) -> bool {
    code.len() <= MAX_ROOM_CODE_LENGTH
        && code.bytes().all(|byte| byte.is_ascii())
        && looks_like_bare_room_code(code)
        && code.contains('-')
}

fn is_six_digits(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_word(value: &str) -> bool {
    !value.is_empty() && value.len() <= 16 && value.bytes().all(|byte| byte.is_ascii_lowercase())
}

fn sample_below(source: &mut impl EntropySource, exclusive_upper: u32) -> Result<u32, InviteError> {
    let acceptance_limit =
        ((u32::MAX as u64 + 1) / exclusive_upper as u64) * exclusive_upper as u64;
    for _ in 0..MAX_REJECTION_DRAWS {
        let mut bytes = [0; 4];
        source
            .fill(&mut bytes)
            .map_err(|_| InviteError::EntropyUnavailable)?;
        let value = u32::from_le_bytes(bytes);
        if (value as u64) < acceptance_limit {
            return Ok(value % exclusive_upper);
        }
    }
    Err(InviteError::UnusableEntropy)
}
