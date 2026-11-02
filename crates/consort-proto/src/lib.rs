//! The Consort wire contract: pure data and pure functions, with no I/O.
//!
//! The hub, the worker and the clients all build on these types. The
//! design docs are `docs/protocol.md` and `docs/data-model.md`. The terms
//! come from `CONTEXT.md`.
//!
//! # Modules
//!
//! - [`ids`] has the prefixed UUIDv7 IDs (`ChannelId` is `chn_...`, and so
//!   on), and the opaque [`ids::TurnId`], [`ids::RequestId`],
//!   [`ids::NativeRef`] and [`ids::FrameId`].
//! - [`time`] has [`time::Timestamp`], Unix milliseconds, with RFC 3339
//!   input and output.
//! - [`error`] has [`error::ErrorCode`] and the `{code, message}`
//!   [`error::ErrorBody`].
//! - [`frame`] has the `{v, id, type, body}` and `{v, re, ok, body|error}`
//!   envelope, the [`frame::Message`], [`frame::Request`] and
//!   [`frame::MessageFamily`] traits, the version constants, and the size
//!   limits that frames and message bodies share (with [`frame::json_len`],
//!   the way to measure them).
//! - [`model`] has every entity in data-model.md.
//! - [`release`] has the repository of the releases, [`release::Version`], the
//!   names of the release files, `release.json` and [`release::InstallKind`].
//! - [`local`] has the folder layout, the state file and the JSON of a hub on
//!   the computer of the user.
//! - [`logs`] has the log rule that keeps the WebSocket libraries at `info`.
//! - [`status`] has the task status machine, the Tasks panel tabs and the
//!   "your turn" groups.
//! - [`auth`] has the pairing bodies, pairing codes, `auth.challenge`,
//!   `auth.response`, `hello` for both links, `auth.http_token`, the hub
//!   origin and the exact bytes to sign.
//! - [`session_host_link`] has [`session_host_link::SessionHostToHub`],
//!   [`session_host_link::HubToSessionHost`] and [`session_host_link::SessionEvent`].
//! - [`execution_link`] has [`execution_link::ExecutionToHub`],
//!   [`execution_link::HubToExecution`] and the tools that run on a worker.
//! - [`client_api`] has [`client_api::ClientRequest`],
//!   [`client_api::HubToClient`], [`client_api::FeedEntry`] and
//!   [`client_api::Snapshot`].
//! - [`tools`] has the hub tools, their audiences, JSON Schemas, typed
//!   arguments and results.
//! - [`turn_input`] has the coordinator wake events and the `<events>`
//!   block.
//!
//! # Serde rules
//!
//! Field names and enum values are snake_case. Message types and session
//! events use the dotted names from protocol.md, such as `task.create` and
//! `turn.started`. Readers ignore unknown fields, so a newer peer can add a
//! field without breaking an older one. An empty optional field serializes
//! as `null`, and a missing optional field or list reads as empty.

// Tests may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod attachment_names;
pub mod auth;
pub mod client_api;
pub mod error;
pub mod execution_link;
pub mod frame;
mod hex;
pub mod ids;
pub mod local;
pub mod logs;
pub mod model;
pub mod release;
pub mod session_host_link;
pub mod status;
pub mod time;
pub mod tools;
pub mod turn_input;
