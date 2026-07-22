use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportConfigError {
    EmptyEndpoint(&'static str),
    EmptyCandidateRule,
    EmptyCandidateSet,
}

impl fmt::Display for TransportConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyEndpoint(kind) => write!(formatter, "{kind} endpoint must not be empty"),
            Self::EmptyCandidateRule => formatter.write_str("candidate rule must not be empty"),
            Self::EmptyCandidateSet => {
                formatter.write_str("filtered candidate policy must contain candidates")
            }
        }
    }
}

impl std::error::Error for TransportConfigError {}

macro_rules! endpoint_type {
    ($name:ident, $kind:expr) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl AsRef<str>) -> Result<Self, TransportConfigError> {
                let value = value.as_ref().trim();
                if value.is_empty() {
                    return Err(TransportConfigError::EmptyEndpoint($kind));
                }
                Ok(Self(value.to_owned()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                if value.is_empty() || value.trim() != value {
                    return Err(D::Error::custom(concat!(
                        "expected a canonical non-empty ",
                        $kind,
                        " endpoint"
                    )));
                }
                Ok(Self(value))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

endpoint_type!(RendezvousEndpoint, "rendezvous");
endpoint_type!(RelayEndpoint, "relay");
endpoint_type!(MailboxEndpoint, "mailbox");

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CandidateRule(String);

impl CandidateRule {
    pub fn new(value: impl AsRef<str>) -> Result<Self, TransportConfigError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(TransportConfigError::EmptyCandidateRule);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CandidateRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() || value.trim() != value {
            return Err(D::Error::custom(
                "expected a canonical non-empty candidate rule",
            ));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for CandidateRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CandidateSet(Vec<CandidateRule>);

impl CandidateSet {
    pub fn new(
        candidates: impl IntoIterator<Item = CandidateRule>,
    ) -> Result<Self, TransportConfigError> {
        let candidates = candidates.into_iter().collect::<Vec<_>>();
        if candidates.is_empty() {
            return Err(TransportConfigError::EmptyCandidateSet);
        }
        Ok(Self(candidates))
    }

    pub fn as_slice(&self) -> &[CandidateRule] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CandidateSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let candidates = Vec::<CandidateRule>::deserialize(deserializer)?;
        Self::new(candidates).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidatePolicy {
    Any,
    AllowOnly(CandidateSet),
    Deny(CandidateSet),
    AllowThenDeny {
        allow: CandidateSet,
        deny: CandidateSet,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataPathPolicy {
    Automatic { relay: Option<RelayEndpoint> },
    RelayOnly { relay: RelayEndpoint },
    DirectOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RendezvousFallback {
    Disabled,
    LocalDiscovery,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportPolicy {
    LocalDiscovery {
        candidates: CandidatePolicy,
    },
    Rendezvous {
        endpoint: RendezvousEndpoint,
        data_path: DataPathPolicy,
        candidates: CandidatePolicy,
        fallback: RendezvousFallback,
    },
}
