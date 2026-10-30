//! Settings > Proactivity on the hub backend: what each proactivity
//! [`Command`] asks of the hub, and how the answers go into the cache.

use std::sync::Arc;

use consort_proto::client_api::{
    ChannelUpdate, MessageRate, ProactivityCancelFollowUps, ProactivityLog, ProactivitySnooze,
};

use super::{Shared, commands::parse_id};
use crate::connectors::LoadState;
use crate::proactivity::{LOG_PAGE, set_request};
use crate::{ClientError, Command, CommandOutput};

/// An `unsupported` answer in words that say what the hub cannot do yet.
fn older_hub(err: ClientError) -> ClientError {
    match err {
        ClientError::Hub { code, .. } if code == "unsupported" => ClientError::Hub {
            code,
            message: "This hub is older than the app and has no proactivity yet. Update the hub, then try again.".to_owned(),
        },
        other => other,
    }
}

/// Keeps the error of a change for the page, and gives it back.
fn note_error(shared: &Arc<Shared>, err: ClientError) -> ClientError {
    let err = older_hub(err);
    if let Ok(mut cache) = shared.cache.lock() {
        cache.proactivity.error = Some(err.to_string());
    }
    shared.publish();
    err
}

fn clear_error(shared: &Arc<Shared>) {
    if let Ok(mut cache) = shared.cache.lock() {
        cache.proactivity.error = None;
    }
}

/// Runs one proactivity command.
pub(super) async fn run(
    shared: &Arc<Shared>,
    command: Command,
) -> Result<CommandOutput, ClientError> {
    match command {
        Command::LoadProactivityLog => {
            if let Ok(mut cache) = shared.cache.lock()
                && cache.proactivity.log_load != LoadState::Loaded
            {
                cache.proactivity.log_load = LoadState::Loading;
            }
            shared.publish();
            match shared
                .call(ProactivityLog {
                    limit: Some(LOG_PAGE),
                    ..Default::default()
                })
                .await
            {
                Ok(page) => {
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache.proactivity.load_page(page.wakes, page.has_more);
                    }
                }
                Err(err) => {
                    let err = older_hub(err);
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache.proactivity.log_load = LoadState::Failed(err.to_string());
                    }
                    shared.publish();
                    return Err(err);
                }
            }
        }
        Command::SetProactivity(changes) => {
            let current = shared
                .cache
                .lock()
                .ok()
                .and_then(|c| c.proactivity.settings.clone());
            let request = set_request(&changes, current.as_ref());
            match shared.call(request).await {
                Ok(result) => {
                    clear_error(shared);
                    if let Ok(mut cache) = shared.cache.lock() {
                        cache.proactivity.apply_settings(result.settings);
                    }
                }
                Err(err) => return Err(note_error(shared, err)),
            }
        }
        Command::SnoozeProactivity(option) => match shared
            .call(ProactivitySnooze {
                choice: option.choice(),
            })
            .await
        {
            Ok(result) => {
                clear_error(shared);
                if let Ok(mut cache) = shared.cache.lock() {
                    cache.proactivity.apply_settings(result.settings);
                }
            }
            Err(err) => return Err(note_error(shared, err)),
        },
        Command::CancelAllFollowUps { channel_id } => {
            let channel_id = channel_id
                .as_deref()
                .map(|id| parse_id(id, "channel"))
                .transpose()?;
            shared
                .call(ProactivityCancelFollowUps { channel_id })
                .await
                .map_err(|err| note_error(shared, err))?;
        }
        Command::SetChannelProactivity { channel_id, level } => {
            let mut update = ChannelUpdate::new(parse_id(&channel_id, "channel")?);
            update.proactivity = Some(level);
            shared.call(update).await?;
        }
        Command::RateMessage { message_id, rating } => {
            shared
                .call(MessageRate {
                    message_id: parse_id(&message_id, "message")?,
                    rating,
                })
                .await
                .map_err(older_hub)?;
        }
        other => {
            return Err(ClientError::Other(format!(
                "{other:?} is not a proactivity command"
            )));
        }
    }
    shared.publish();
    Ok(CommandOutput::Done)
}
