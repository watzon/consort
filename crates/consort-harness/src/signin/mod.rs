//! Sign-in and sign-out through each harness's own commands.
//!
//! - [`claude`]: `claude auth login` in a pseudo-terminal, `claude auth
//!   status --json` and `claude auth logout`.
//! - [`crate::codex::login`]: ChatGPT and API key sign-in through
//!   `codex app-server`.
//! - [`crate::acp::authenticate`] and [`crate::acp::logout`]: an ACP agent's
//!   own auth methods.
//!
//! None of these read a credential file. The worker never logs a code or a
//! secret value that passes through them.

pub mod claude;
