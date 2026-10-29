//! The user's preferences, which every client shares.

use consort_proto::client_api::{PreferencesResult, PreferencesSetFavoriteModel};
use consort_proto::model::{FAVORITE_FIELD_MAX_CHARS, FAVORITE_MODELS_MAX};

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::preferences;

impl Hub {
    /// Adds or removes one favorite model. A request that changes nothing
    /// writes nothing and sends no feed entry. The account and the model do
    /// not have to exist now: a favorite stays when an account is signed out
    /// or a model goes away, and the clients hide it until it comes back.
    pub(crate) async fn preferences_set_favorite_model(
        &self,
        _caller: &Caller,
        request: PreferencesSetFavoriteModel,
    ) -> Result<PreferencesResult> {
        let mut favorite = request.model;
        favorite.account_label = favorite.account_label.trim().to_owned();
        favorite.model = favorite.model.trim().to_owned();
        if favorite.account_label.is_empty() {
            return Err(HubError::invalid(
                "A favorite model needs an account label. Send the label of the account.",
            ));
        }
        for (field, value) in [
            ("account label", &favorite.account_label),
            ("model", &favorite.model),
        ] {
            if value.chars().count() > FAVORITE_FIELD_MAX_CHARS {
                return Err(HubError::invalid(format!(
                    "The {field} of a favorite model is too long. Use at most {FAVORITE_FIELD_MAX_CHARS} characters."
                )));
            }
        }
        let on = request.favorite;
        let preferences = self
            .store
            .write(move |tx| {
                let mut preferences = preferences::preferences(tx.db())?;
                let full = preferences.favorite_models.len() >= FAVORITE_MODELS_MAX;
                if on && full && !preferences.favorite_models.contains(&favorite) {
                    return Err(HubError::invalid(format!(
                        "You have {FAVORITE_MODELS_MAX} favorite models, which is the most. Remove the star from a model, then try again."
                    )));
                }
                if preferences.set_favorite(favorite, on) {
                    tx.save_preferences(&preferences)?;
                }
                Ok(preferences)
            })
            .await?;
        Ok(PreferencesResult { preferences })
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::client_api::{EntityPayload, FeedChange, SnapshotScope};
    use consort_proto::model::{FavoriteModel, Preferences};

    use super::*;
    use crate::hub::test_hub;
    use crate::store::feed;

    fn star(label: &str, model: &str, favorite: bool) -> PreferencesSetFavoriteModel {
        PreferencesSetFavoriteModel {
            model: FavoriteModel {
                account_label: label.into(),
                model: model.into(),
            },
            favorite,
        }
    }

    /// The preferences upserts in the feed after `seq`.
    async fn upserts_after(hub: &Hub, seq: u64) -> Vec<Preferences> {
        hub.store
            .read(move |conn| feed::entries_after(conn, seq, 100))
            .await
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.change {
                FeedChange::Upsert(EntityPayload::Preferences(p)) => {
                    assert_eq!(e.entity_id, "preferences");
                    assert_eq!(e.channel_id, None);
                    Some(p)
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn the_hub_stores_favorites_and_sends_a_feed_upsert() {
        let (hub, _dir) = test_hub().await;
        let start = hub.snapshot(SnapshotScope::All).await.unwrap();
        assert_eq!(start.preferences, Some(Preferences::default()));
        let result = hub
            .preferences_set_favorite_model(&Caller::Hub, star(" Claude Work ", "opus", true))
            .await
            .unwrap();
        let opus = FavoriteModel {
            account_label: "Claude Work".into(),
            model: "opus".into(),
        };
        assert_eq!(
            result.preferences.favorite_models,
            std::slice::from_ref(&opus)
        );
        let upserts = upserts_after(&hub, start.seq).await;
        assert_eq!(upserts, std::slice::from_ref(&result.preferences));

        // A second star of the same model changes nothing and sends nothing.
        let again = hub
            .preferences_set_favorite_model(&Caller::Hub, star("Claude Work", "opus", true))
            .await
            .unwrap();
        assert_eq!(
            again.preferences.favorite_models,
            std::slice::from_ref(&opus)
        );
        assert_eq!(upserts_after(&hub, start.seq).await.len(), 1);

        // A favorite of an account that no worker has is kept.
        hub.preferences_set_favorite_model(&Caller::Hub, star("Gone", "m", true))
            .await
            .unwrap();
        let snapshot = hub.snapshot(SnapshotScope::All).await.unwrap();
        let stored = snapshot.preferences.unwrap().favorite_models;
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[1].account_label, "Gone");

        // Removing the star sends the new list.
        let removed = hub
            .preferences_set_favorite_model(&Caller::Hub, star("Claude Work", "opus", false))
            .await
            .unwrap();
        assert_eq!(removed.preferences.favorite_models.len(), 1);
        let upserts = upserts_after(&hub, start.seq).await;
        assert_eq!(upserts.len(), 3);
        assert_eq!(upserts.last(), Some(&removed.preferences));
        // A channel snapshot leaves the preferences out.
        let everything = snapshot.channels[0].id;
        let channel = hub
            .snapshot(SnapshotScope::Channel(everything))
            .await
            .unwrap();
        assert_eq!(channel.preferences, None);
    }

    #[tokio::test]
    async fn a_favorite_needs_an_account_label_and_short_fields() {
        let (hub, _dir) = test_hub().await;
        let empty = hub
            .preferences_set_favorite_model(&Caller::Hub, star("  ", "m", true))
            .await;
        assert!(empty.is_err());
        let long = "x".repeat(FAVORITE_FIELD_MAX_CHARS + 1);
        let too_long = hub
            .preferences_set_favorite_model(&Caller::Hub, star("A", &long, true))
            .await;
        assert!(too_long.is_err());
    }
}
