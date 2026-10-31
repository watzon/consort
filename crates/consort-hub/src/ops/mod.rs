//! Client API handlers and the shared operations behind them.

pub(crate) mod accounts;
mod command_check;
mod conversation;
pub(crate) mod drafts;
mod exec;
pub(crate) mod flow;
pub(crate) mod hooks;
pub(crate) mod knowledge;
pub(crate) mod machines;
pub(crate) mod model_switch;
mod preferences;
pub(crate) mod registry;
mod routines;
mod sync;
mod tasks;
pub(crate) mod waiting;
