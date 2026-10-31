//! Preferences: the choices of the user that every client shares, such as
//! the favorite models of the model picker. The hub keeps one record.

use serde::{Deserialize, Serialize};

use super::fallback::FallbackPath;

/// The feed `entity_id` of the one preferences record.
pub const PREFERENCES_ENTITY_ID: &str = "preferences";

/// The most favorite models that the hub keeps.
pub const FAVORITE_MODELS_MAX: usize = 200;

/// The most characters in the account label or the model ID of a favorite.
pub const FAVORITE_FIELD_MAX_CHARS: usize = 200;

/// A model that the user marked with a star in the model picker: an account
/// label and a model ID, as in a [`crate::model::ModelSelection`]. A
/// favorite whose account or model is gone stays stored. The picker does not
/// show it until the account or the model comes back.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FavoriteModel {
    /// The account label, such as `Claude Personal`.
    pub account_label: String,
    /// The model ID, as the account reports it.
    pub model: String,
}

/// The user's preferences. Every client reads the same record.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// The favorite models, in the order that the user starred them.
    pub favorite_models: Vec<FavoriteModel>,
    /// The fallback path: where work goes when the model of a channel
    /// cannot answer.
    pub fallback_path: FallbackPath,
    /// The source of the command check.
    pub command_check: CommandCheckSetting,
    /// The API endpoints that the user added. Their keys stay on the hub.
    pub endpoints: Vec<ApiEndpoint>,
}

/// An API endpoint. The key stays on the hub. Clients see its last four
/// characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiEndpoint {
    /// The ID.
    pub id: String,
    /// The name that the user gave.
    pub name: String,
    /// The base URL.
    pub base_url: String,
    /// The last four characters of the key.
    pub key_last_four: String,
}

/// The kind of source of the command check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCheckKind {
    /// A model of an account.
    #[default]
    Account,
    /// A decision model: a `state` and named `questions`.
    Decision,
    /// A chat model on an OpenAI-compatible endpoint.
    Chat,
}

/// The default confidence limit of a decision model, in percent.
pub const DEFAULT_CONFIDENCE_LIMIT: u8 = 80;

fn default_confidence_limit() -> u8 {
    DEFAULT_CONFIDENCE_LIMIT
}

/// The source of the command check. It keeps the choice of each kind, so a
/// change of kind loses nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandCheckSetting {
    /// The kind that runs.
    pub kind: CommandCheckKind,
    /// The model of an account. `None` means that Consort chooses the
    /// smallest model.
    pub account_model: Option<FavoriteModel>,
    /// The decision model.
    pub decision: Option<CommandCheckEndpoint>,
    /// The chat model.
    pub chat: Option<CommandCheckEndpoint>,
}

/// A model on an API endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCheckEndpoint {
    /// The endpoint.
    pub endpoint_id: String,
    /// The model name.
    pub model: String,
    /// The confidence limit, in percent. Only a decision model uses it.
    #[serde(default = "default_confidence_limit")]
    pub confidence_limit: u8,
}

impl Preferences {
    /// Adds or removes one favorite. Adding a favorite that is in the list
    /// already, or removing one that is not, changes nothing. Returns true
    /// when the list changed.
    pub fn set_favorite(&mut self, favorite: FavoriteModel, on: bool) -> bool {
        let at = self.favorite_models.iter().position(|f| *f == favorite);
        match (at, on) {
            (None, true) => {
                self.favorite_models.push(favorite);
                true
            }
            (Some(index), false) => {
                self.favorite_models.remove(index);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fav(label: &str, model: &str) -> FavoriteModel {
        FavoriteModel {
            account_label: label.into(),
            model: model.into(),
        }
    }

    #[test]
    fn set_favorite_adds_once_and_removes() {
        let mut p = Preferences::default();
        assert!(p.set_favorite(fav("A", "m"), true));
        assert!(!p.set_favorite(fav("A", "m"), true), "no copy");
        assert!(p.set_favorite(fav("B", "m"), true));
        assert_eq!(p.favorite_models, [fav("A", "m"), fav("B", "m")]);
        assert!(p.set_favorite(fav("A", "m"), false));
        assert!(!p.set_favorite(fav("A", "m"), false));
        assert_eq!(p.favorite_models, [fav("B", "m")]);
    }

    #[test]
    fn an_old_record_reads_as_empty() {
        let p: Preferences = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(p.favorite_models.is_empty());
        assert_eq!(p.fallback_path, FallbackPath::default());
    }

    #[test]
    fn the_old_switch_in_a_stored_record_is_ignored() {
        let p: Preferences =
            serde_json::from_value(serde_json::json!({"account_fallback": false})).unwrap();
        assert!(p.fallback_path.other_accounts);
    }
}
