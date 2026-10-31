//! Live dictation in a message box (docs/DICTATION-PLAN.md).
//!
//! This crate holds the state of one dictation and calls the Sayso SDK. It has
//! no GPUI and no hub I/O: the audio, the models and the Deepgram key stay on
//! this computer. `consort-desktop` calls it and shows what it reports.

mod cloud;
mod dictation;
mod models;
mod settings;
mod text;

pub use cloud::{CloudChoice, CloudLoss};
pub use dictation::{Deps, Dictation, Failure, Phase, Preflight, Request, SystemPreflight, Update};
pub use models::{
    ModelNeed, Purpose, cleanup_choices, cleanup_name, format_size, plan_models, speech_languages,
};
pub use sayso_sdk::cleanup::{Runtime, RuntimeCleaner};
pub use sayso_sdk::{CaptureMicrophone, Device, EngineSource, ModelInfo, devices, permission};
pub use settings::{
    DEFAULT_SHORTCUT, Engine, KeyStore, Settings, SettingsStore, StopAfter, StoreError,
};
pub use text::{Draft, Edit, LiveWords, Spacing};

#[cfg(test)]
mod tests;
