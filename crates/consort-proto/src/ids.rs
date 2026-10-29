//! Prefixed identifiers.
//!
//! Every entity ID is a UUIDv7 with a type prefix, for example
//! `tsk_0192f0c4-7a1e-7c3d-9b4a-2f1e0d9c8b7a`. UUIDv7 sorts by creation time,
//! so `Ord` on an ID sorts by creation time too.
//!
//! Parsing checks the prefix and the UUID syntax. It accepts any UUID version,
//! so test fixtures such as `chn_00000000-0000-0000-0000-000000000001` parse.
//! [`ChannelId::new`] and the other `new` functions always make a UUIDv7.
//!
//! Other identifiers in this module are opaque strings that a harness or a
//! peer chooses: [`TurnId`], [`RequestId`], [`NativeRef`] and [`FrameId`].

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// The error when a string is not a valid prefixed ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The string does not start with the prefix of the ID type.
    #[error("expected an ID that starts with `{expected}`, got `{found}`")]
    WrongPrefix {
        /// The prefix that the ID type needs, for example `tsk_`.
        expected: &'static str,
        /// The string that was given.
        found: String,
    },
    /// The part after the prefix is not a UUID.
    #[error("`{found}` is not a valid ID: the part after `{prefix}` is not a UUID")]
    InvalidUuid {
        /// The prefix of the ID type.
        prefix: &'static str,
        /// The string that was given.
        found: String,
    },
}

/// Parses `<prefix><uuid>` into a UUID.
fn parse_prefixed(prefix: &'static str, s: &str) -> Result<Uuid, IdError> {
    let rest = s.strip_prefix(prefix).ok_or_else(|| IdError::WrongPrefix {
        expected: prefix,
        found: s.to_owned(),
    })?;
    // Accept only the hyphenated form, so that one ID has one spelling.
    if rest.len() != 36 {
        return Err(IdError::InvalidUuid {
            prefix,
            found: s.to_owned(),
        });
    }
    Uuid::try_parse(rest).map_err(|_| IdError::InvalidUuid {
        prefix,
        found: s.to_owned(),
    })
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// The type prefix, including the underscore.
            pub const PREFIX: &'static str = $prefix;

            /// Makes a new ID from a UUIDv7 with the current time.
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wraps an existing UUID.
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The UUID without the prefix.
            pub const fn uuid(&self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0.hyphenated())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}(\"{}\")", stringify!($name), self)
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                parse_prefixed($prefix, s).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = Cow::<'de, str>::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }

            fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "pattern": concat!("^", $prefix, "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"),
                    "description": concat!("An ID that starts with `", $prefix, "`."),
                })
            }
        }
    };
}

define_id!(
    /// A group of channels in the sidebar (`grp_`).
    GroupId, "grp_"
);
define_id!(
    /// A channel (`chn_`).
    ChannelId, "chn_"
);
define_id!(
    /// A message (`msg_`).
    MessageId, "msg_"
);
define_id!(
    /// A task (`tsk_`).
    TaskId, "tsk_"
);
define_id!(
    /// A session (`ses_`).
    SessionId, "ses_"
);
define_id!(
    /// A session item, unique per session (`itm_`).
    ItemId, "itm_"
);
define_id!(
    /// An approval (`apr_`).
    ApprovalId, "apr_"
);
define_id!(
    /// A decision (`dec_`).
    DecisionId, "dec_"
);
define_id!(
    /// A repo (`rep_`).
    RepoId, "rep_"
);
define_id!(
    /// A checkout (`cko_`).
    CheckoutId, "cko_"
);
define_id!(
    /// A routine (`rtn_`).
    RoutineId, "rtn_"
);
define_id!(
    /// A worker (`wrk_`).
    WorkerId, "wrk_"
);
define_id!(
    /// A harness account, unique per worker (`acc_`).
    AccountId, "acc_"
);
define_id!(
    /// A client (`cli_`).
    ClientId, "cli_"
);
define_id!(
    /// One run of a provider's sign-in flow for one harness account
    /// (`sgn_`).
    SignInId, "sgn_"
);
define_id!(
    /// One connected account of one connector service (`con_`).
    ConnectionId, "con_"
);
define_id!(
    /// An installed skill (`skl_`).
    SkillId, "skl_"
);
define_id!(
    /// One record in the audit log of connector tool calls (`ccl_`).
    ConnectorCallId, "ccl_"
);

macro_rules! define_text_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps a string.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The string value.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Returns the string value.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($name), self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

define_text_id!(
    /// A turn in a session. The harness adapter chooses the value.
    TurnId
);
define_text_id!(
    /// A pending harness request (an approval or a question) in a session.
    /// The harness adapter chooses the value. The hub maps it to an
    /// [`ApprovalId`] or a [`DecisionId`].
    RequestId
);
define_text_id!(
    /// The harness's own session ID, used to resume a session.
    NativeRef
);
define_text_id!(
    /// The `id` of a request frame, echoed as `re` in its response. For a
    /// hub command on the worker link, this is also the command ID.
    FrameId
);

impl FrameId {
    /// Makes a new frame ID from a UUIDv7, without a prefix.
    pub fn generate() -> Self {
        Self(Uuid::now_v7().simple().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ids_have_prefix_and_v7() {
        let id = TaskId::new();
        let s = id.to_string();
        assert!(s.starts_with("tsk_"));
        assert_eq!(s.len(), 4 + 36);
        assert_eq!(id.uuid().get_version_num(), 7);
        assert_eq!(s.parse::<TaskId>(), Ok(id));
    }

    #[test]
    fn every_prefix_is_distinct_and_round_trips() {
        let ids = [
            GroupId::new().to_string(),
            ChannelId::new().to_string(),
            MessageId::new().to_string(),
            TaskId::new().to_string(),
            SessionId::new().to_string(),
            ItemId::new().to_string(),
            ApprovalId::new().to_string(),
            DecisionId::new().to_string(),
            RepoId::new().to_string(),
            CheckoutId::new().to_string(),
            RoutineId::new().to_string(),
            WorkerId::new().to_string(),
            AccountId::new().to_string(),
            ClientId::new().to_string(),
            SignInId::new().to_string(),
            ConnectionId::new().to_string(),
            SkillId::new().to_string(),
            ConnectorCallId::new().to_string(),
        ];
        let prefixes: Vec<&str> = ids.iter().map(|s| &s[..4]).collect();
        assert_eq!(
            prefixes,
            [
                "grp_", "chn_", "msg_", "tsk_", "ses_", "itm_", "apr_", "dec_", "rep_", "cko_",
                "rtn_", "wrk_", "acc_", "cli_", "sgn_", "con_", "skl_", "ccl_"
            ]
        );
        assert!(ids[0].parse::<GroupId>().is_ok());
        assert!(ids[13].parse::<ClientId>().is_ok());
        assert!(ids[14].parse::<SignInId>().is_ok());
        assert!(ids[15].parse::<ConnectionId>().is_ok());
        assert!(ids[16].parse::<SkillId>().is_ok());
        assert!(ids[17].parse::<ConnectorCallId>().is_ok());
        assert!(ids[15].parse::<SkillId>().is_err());
    }

    #[test]
    fn wrong_prefix_is_rejected() {
        let chn = ChannelId::new().to_string();
        let err = chn.parse::<TaskId>().unwrap_err();
        assert!(matches!(
            err,
            IdError::WrongPrefix {
                expected: "tsk_",
                ..
            }
        ));
        // The error names the prefix that the type expects.
        assert!(err.to_string().contains("tsk_"));
    }

    #[test]
    fn bad_uuid_is_rejected() {
        for bad in [
            "tsk_",
            "tsk_123",
            "tsk_0192f0c47a1e7c3d9b4a2f1e0d9c8b7a",
            "tsk_0192f0c4-7a1e-7c3d-9b4a-2f1e0d9c8b7z",
            "TSK_0192f0c4-7a1e-7c3d-9b4a-2f1e0d9c8b7a",
            "tsk_{0192f0c4-7a1e-7c3d-9b4a-2f1e0d9c8b7a}",
        ] {
            assert!(bad.parse::<TaskId>().is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn fixture_ids_of_any_version_parse() {
        let id: ChannelId = "chn_00000000-0000-0000-0000-000000000001".parse().unwrap();
        assert_eq!(id.to_string(), "chn_00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn serde_uses_the_prefixed_string() {
        let id: MessageId = "msg_0192f0c4-7a1e-7c3d-9b4a-2f1e0d9c8b7a".parse().unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"msg_0192f0c4-7a1e-7c3d-9b4a-2f1e0d9c8b7a\"");
        assert_eq!(serde_json::from_str::<MessageId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<TaskId>(&json).is_err());
        assert!(serde_json::from_str::<MessageId>("42").is_err());
    }

    #[test]
    fn ids_sort_by_creation_time() {
        let a = TaskId::from_uuid(Uuid::from_u128(1));
        let b = TaskId::from_uuid(Uuid::from_u128(2));
        assert!(a < b);
        let first = TaskId::new();
        let second = TaskId::new();
        assert!(first < second);
    }

    #[test]
    fn frame_ids_are_unique_strings() {
        let a = FrameId::generate();
        let b = FrameId::generate();
        assert_ne!(a, b);
        assert_eq!(a.as_str().len(), 32);
    }
}
