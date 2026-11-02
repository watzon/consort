//! The mock states for the version screens (Paper `TB3-0` and `TED-0`).

use consort_proto::error::HubVersions;
use consort_proto::frame::TooOld;
use consort_proto::model::WorkerUpdateStatus;
use consort_proto::release::InstallKind;

use crate::{
    mock, mock_offline, versions,
    view::{Connection, ViewState},
};

/// The screens that this file builds.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    match screen {
        "hub-too-old" => Some(hub_too_old()),
        "app-too-old" => Some(app_too_old()),
        "update-held" => Some(update_held()),
        "workers-updating" => Some(workers_updating()),
        "pair-too-old" => Some(ViewState {
            connection: Connection::NotPaired,
            ..mock::view_state()
        }),
        _ => None,
    }
}

fn incompatible(too_old: TooOld, hub: HubVersions, app_version: &str) -> ViewState {
    // The saved data and the waiting message stay, as with a hub that is down.
    let mut state = mock_offline::hub_offline();
    state.hub_versions = Some(hub.clone());
    state.connection = Connection::Incompatible {
        too_old,
        hub: Some(hub),
        app_version: app_version.to_owned(),
        retry_in_ms: versions::INCOMPATIBLE_RETRY.as_millis() as u64,
    };
    state
}

/// V1: the hub is too old for this app.
pub fn hub_too_old() -> ViewState {
    incompatible(
        TooOld::Hub,
        HubVersions {
            hub_version: "0.6.2".into(),
            protocol_versions: vec![1],
            install: Some(InstallKind::Service),
        },
        "0.8.0",
    )
}

/// V2: this app is too old for the hub.
pub fn app_too_old() -> ViewState {
    incompatible(
        TooOld::ThisSide,
        HubVersions {
            hub_version: "0.9.0".into(),
            protocol_versions: vec![3],
            install: Some(InstallKind::Service),
        },
        "0.6.2",
    )
}

/// V2: the update waits for the hub. The sidebar shows the hold.
pub fn update_held() -> ViewState {
    let mut state = mock::view_state();
    state.hub_versions = Some(HubVersions {
        hub_version: "0.6.2".into(),
        protocol_versions: vec![1],
        install: None,
    });
    state
}

/// V2: the states of Settings > Workers.
pub fn workers_updating() -> ViewState {
    let mut state = mock::view_state();
    let statuses = [
        None,
        Some(WorkerUpdateStatus::WaitingForWork {
            version: "0.8.0".into(),
        }),
        Some(WorkerUpdateStatus::Downloading {
            version: "0.8.0".into(),
        }),
    ];
    for (worker, status) in state.workers.iter_mut().zip(statuses) {
        worker.update = status.and_then(|status| versions::worker_update_view(&status));
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screens_are_built_here_and_nothing_else() {
        for screen in [
            "hub-too-old",
            "app-too-old",
            "update-held",
            "workers-updating",
            "pair-too-old",
        ] {
            assert!(screen_state(screen).is_some(), "{screen}");
        }
        assert!(screen_state("tutti").is_none());
    }

    #[test]
    fn an_incompatible_screen_keeps_the_saved_data() {
        let state = hub_too_old();
        assert!(matches!(
            state.connection,
            Connection::Incompatible {
                too_old: TooOld::Hub,
                ..
            }
        ));
        assert!(state.saved_data.is_some());
    }
}
