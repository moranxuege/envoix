use std::fmt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigField {
    RoomTtl,
    RelayTtl,
    JoinDeadline,
    CloseGrace,
    ReplyDeadline,
    RoomKeyLength,
    WaitingRooms,
}

impl fmt::Display for ConfigField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RoomTtl => formatter.write_str("room TTL"),
            Self::RelayTtl => formatter.write_str("relay TTL"),
            Self::JoinDeadline => formatter.write_str("join deadline"),
            Self::CloseGrace => formatter.write_str("close grace"),
            Self::ReplyDeadline => formatter.write_str("reply deadline"),
            Self::RoomKeyLength => formatter.write_str("room-key length"),
            Self::WaitingRooms => formatter.write_str("waiting-room limit"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    ZeroDuration { field: ConfigField },
    ZeroLimit { field: ConfigField },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDuration { field } => write!(formatter, "{field} must be non-zero"),
            Self::ZeroLimit { field } => write!(formatter, "{field} must be non-zero"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlLimits {
    max_room_key_length: usize,
}

impl ControlLimits {
    pub fn new(max_room_key_length: usize) -> Result<Self, ConfigError> {
        if max_room_key_length == 0 {
            return Err(ConfigError::ZeroLimit {
                field: ConfigField::RoomKeyLength,
            });
        }
        Ok(Self {
            max_room_key_length,
        })
    }

    pub const fn max_room_key_length(self) -> usize {
        self.max_room_key_length
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistryConfig {
    room_ttl: Duration,
    relay_ttl: Duration,
    join_deadline: Duration,
    close_grace: Duration,
    control: ControlLimits,
    max_waiting_rooms: usize,
}

impl RegistryConfig {
    pub fn new(
        room_ttl: Duration,
        relay_ttl: Duration,
        join_deadline: Duration,
        close_grace: Duration,
        control: ControlLimits,
        max_waiting_rooms: usize,
    ) -> Result<Self, ConfigError> {
        for (field, value) in [
            (ConfigField::RoomTtl, room_ttl),
            (ConfigField::RelayTtl, relay_ttl),
            (ConfigField::JoinDeadline, join_deadline),
            (ConfigField::CloseGrace, close_grace),
        ] {
            if value.is_zero() {
                return Err(ConfigError::ZeroDuration { field });
            }
        }
        if max_waiting_rooms == 0 {
            return Err(ConfigError::ZeroLimit {
                field: ConfigField::WaitingRooms,
            });
        }
        Ok(Self {
            room_ttl,
            relay_ttl,
            join_deadline,
            close_grace,
            control,
            max_waiting_rooms,
        })
    }

    pub const fn room_ttl(self) -> Duration {
        self.room_ttl
    }

    pub const fn relay_ttl(self) -> Duration {
        self.relay_ttl
    }

    pub const fn join_deadline(self) -> Duration {
        self.join_deadline
    }

    pub const fn close_grace(self) -> Duration {
        self.close_grace
    }

    pub const fn control(self) -> ControlLimits {
        self.control
    }

    pub const fn max_waiting_rooms(self) -> usize {
        self.max_waiting_rooms
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientConfig {
    reply_deadline: Duration,
    control: ControlLimits,
}

impl ClientConfig {
    pub fn new(reply_deadline: Duration, control: ControlLimits) -> Result<Self, ConfigError> {
        if reply_deadline.is_zero() {
            return Err(ConfigError::ZeroDuration {
                field: ConfigField::ReplyDeadline,
            });
        }
        Ok(Self {
            reply_deadline,
            control,
        })
    }

    pub const fn reply_deadline(self) -> Duration {
        self.reply_deadline
    }

    pub const fn control(self) -> ControlLimits {
        self.control
    }
}
