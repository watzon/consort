//! Managed tools for Consort workers.
//!
//! A managed tool is a CLI or runtime that the worker installs in
//! `<consort home>/tools/<tool>/<version>/`: Node, Claude Code, the Claude
//! agent ACP adapter, Codex, OpenCode, Gemini CLI and agents from the ACP
//! registry.
//!
//! The crate has two layers:
//!
//! - Always on: the shared types ([`ToolId`], [`ToolStatus`], [`LaunchSpec`]
//!   and others), [`Platform`] detection and asset names, the ACP
//!   [`registry`] types, and the [`config`] helpers that turn off self-updates.
//!   These need no network and no async runtime.
//! - Feature `install` (on by default): [`ToolStore`], which downloads,
//!   verifies, unpacks and launches tools.
//!
//! The hub uses `default-features = false` to get only the registry types.

pub mod config;
mod error;
mod platform;
pub mod registry;
mod tool;

#[cfg(feature = "install")]
mod install;

pub use error::ToolError;
pub use platform::{Arch, Libc, Os, Platform};
pub use tool::{Installed, LaunchSpec, ParseToolIdError, ToolId, ToolState, ToolStatus};

#[cfg(feature = "install")]
pub use install::{Sources, ToolStore, ToolStoreOptions};
