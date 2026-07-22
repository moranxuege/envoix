use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

macro_rules! integer_identity {
    ($name:ident, $inner:ty) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name($inner);

        impl $name {
            pub const fn new(value: $inner) -> Self {
                Self(value)
            }

            pub const fn get(self) -> $inner {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

macro_rules! opaque_128_identity {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(
            #[serde(
                serialize_with = "serialize_u128_hex",
                deserialize_with = "deserialize_u128_hex"
            )]
            u128,
        );

        impl $name {
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(u128::from_be_bytes(bytes))
            }

            pub const fn to_bytes(self) -> [u8; 16] {
                self.0.to_be_bytes()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{:032x}", self.0)
            }
        }
    };
}

integer_identity!(RecordId, u64);
integer_identity!(AttemptGen, u32);
opaque_128_identity!(TransferId);
opaque_128_identity!(ArtifactId);
opaque_128_identity!(RequestId);

fn serialize_u128_hex<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format!("{value:032x}"))
}

fn deserialize_u128_hex<'de, D>(deserializer: D) -> Result<u128, D::Error>
where
    D: Deserializer<'de>,
{
    let encoded = String::deserialize(deserializer)?;
    if encoded.len() != 32 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(D::Error::custom(
            "expected exactly 32 hexadecimal characters",
        ));
    }
    u128::from_str_radix(&encoded, 16).map_err(D::Error::custom)
}
