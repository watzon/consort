//! Preferences: the choices of the user that every client shares, such as
//! the favorite models of the model picker. The hub keeps one record.

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// The favorite models, in the order that the user starred them.
    pub favorite_models: Vec<FavoriteModel>,
    /// When an account reaches a usage limit, work moves to another signed-in
    /// account of the same harness that has the same model.
    pub account_fallback: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            favorite_models: Vec::new(),
            account_fallback: true,
        }
    }
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
    }
}
