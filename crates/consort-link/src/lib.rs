//! What a session host and an execution worker share: the machine key, the
//! WebSocket link to the hub with its reconnects, and the signed HTTP calls.
//! The hub gives each kind of machine its own route and its own credentials.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;
pub mod http;
pub mod identity;
pub mod link;
pub mod update;

pub use crate::error::LinkError;
