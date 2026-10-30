//! The views of the app.

/// What the app calls the main channel. The code and the hub still call it
/// Tutti (`ChannelKind::Tutti`) until the rename pass.
pub const TUTTI: &str = "Tutti";

pub mod add_account;
pub mod add_worker;
pub mod admin_parts;
pub mod attachments;
pub mod channel_extensions;
pub mod channel_panel;
pub mod connectors;
pub mod dialogs;
pub mod ext_parts;
pub mod hub_cards;
pub mod icons;
pub mod links;
pub mod mark;
pub mod markdown;
pub mod memory;
pub mod menu;
pub mod model_picker;
pub mod motion;
pub mod onboarding;
pub mod panels;
pub mod parts;
pub mod proactivity;
pub mod routines;
pub mod schedule;
pub mod settings;
pub mod settings_clients;
pub mod settings_hub;
pub mod settings_notifications;
pub mod sidebar;
pub mod skills;
pub mod startup_error;
pub mod switcher;
pub mod timeline;
pub mod update_notice;
pub mod window_controls;
pub mod workers;
