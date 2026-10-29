//! The user's preferences: one JSON record in `meta`, under the key
//! `preferences`. Every write sends a feed upsert, so other clients update.

use consort_proto::client_api::EntityPayload;
use consort_proto::model::Preferences;
use rusqlite::Connection;

use super::{Tx, meta_get, parse_doc, to_doc};
use crate::error::Result;

const KEY: &str = "preferences";

/// The preferences. A hub that has none stored yet has empty ones.
pub fn preferences(conn: &Connection) -> Result<Preferences> {
    match meta_get(conn, KEY)? {
        Some(text) => parse_doc(&text),
        None => Ok(Preferences::default()),
    }
}

impl Tx<'_> {
    /// Stores the preferences and sends a feed upsert.
    pub fn save_preferences(&mut self, preferences: &Preferences) -> Result<()> {
        self.meta_set(KEY, &to_doc(preferences)?)?;
        self.emit(EntityPayload::Preferences(preferences.clone()))
    }
}
