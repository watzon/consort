//! What the app says about versions: which side of a link is too old, the
//! command that updates a hub, and where the update of a worker is.
//!
//! This is pure text. The hub says what it runs and how it was installed. The
//! command comes from that. A hub from before the install kind has no
//! `consort hub update`, so it gets the upgrade guide and no command.

pub use consort_proto::error::HubVersions;
pub use consort_proto::frame::TooOld;
use consort_proto::frame::{
    SUPPORTED_PROTOCOL_VERSIONS, oldest_compatible_software, who_is_too_old,
};
use consort_proto::model::WorkerUpdateStatus;
use consort_proto::release::{InstallKind, RELEASES_REPO, Version};

/// How long a client waits between two tries with a hub that does not speak
/// its protocol. An update takes minutes, so it does not try often.
pub const INCOMPATIBLE_RETRY: std::time::Duration = std::time::Duration::from_secs(60);

/// The page that explains an upgrade of a hub.
pub fn upgrade_guide_url() -> String {
    format!("https://github.com/{RELEASES_REPO}/blob/main/docs/deploy.md#upgrade")
}

/// The command that updates a hub, or `None` when no command fits: a local
/// hub updates with the app, and a hub that does not say how it was installed
/// is too old to have the command.
pub fn update_command(install: Option<InstallKind>, version: Option<&str>) -> Option<String> {
    let flag = version.map_or_else(String::new, |v| format!(" --version {v}"));
    match install? {
        InstallKind::Service => Some(format!("sudo consort hub update{flag}")),
        InstallKind::Manual => Some(format!("consort hub update{flag}")),
        InstallKind::Docker => Some(
            "git pull && docker compose -f deploy/hub/docker-compose.yml up -d --build".to_owned(),
        ),
        InstallKind::Local => None,
    }
}

/// Which side is too old, from the versions in a `protocol_mismatch` error.
pub fn too_old(hub: Option<&HubVersions>) -> TooOld {
    who_is_too_old(
        SUPPORTED_PROTOCOL_VERSIONS,
        hub.map(|hub| hub.protocol_versions.as_slice()),
    )
}

/// The card above the composer while the hub and this app have no common
/// protocol version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncompatibleView {
    pub too_old: TooOld,
    pub title: String,
    pub body: String,
    /// The command to run on the server. The user can select and copy it.
    pub command: Option<String>,
}

/// The text of the card.
pub fn incompatible_view(
    too_old: TooOld,
    hub: Option<&HubVersions>,
    app_version: &str,
) -> IncompatibleView {
    let hub_version = hub.map(|hub| hub.hub_version.as_str());
    match too_old {
        TooOld::Hub => IncompatibleView {
            too_old,
            title: "The hub needs an update".to_owned(),
            body: format!(
                "This app runs {app_version}. {} You can read your channels. Your messages wait here, and they go out when the hub is updated.",
                hub_version.map_or_else(
                    || "Your hub is too old for this app.".to_owned(),
                    |version| format!("Your hub runs {version}, and it is too old for this app."),
                ),
            ),
            command: update_command(hub.and_then(|hub| hub.install), Some(app_version)),
        },
        TooOld::ThisSide => IncompatibleView {
            too_old,
            title: "This app needs an update".to_owned(),
            body: format!(
                "Your hub runs {}, and it no longer works with app {app_version}. You can read your channels until you update.",
                hub_version.unwrap_or("a newer version"),
            ),
            command: None,
        },
    }
}

/// The error on the Connect screen when a pairing fails on the protocol
/// version. The title goes on the first line of the box.
pub fn pair_error(hub: Option<&HubVersions>) -> (String, String) {
    let hub_part = hub.map_or_else(String::new, |hub| {
        format!("The hub runs {}. ", hub.hub_version)
    });
    match too_old(hub) {
        TooOld::Hub => {
            let needs = oldest_compatible_software().map_or_else(String::new, |version| {
                format!("This app needs {version} or later. ")
            });
            (
                "This hub is too old for this app".to_owned(),
                format!(
                    "{hub_part}{needs}Update the hub, and then connect again. The setup code still works."
                ),
            )
        }
        TooOld::ThisSide => (
            "This app is too old for this hub".to_owned(),
            format!(
                "{hub_part}Update this app, and then connect again. The setup code still works."
            ),
        ),
    }
}

/// The card in Settings > Hub when a newer version exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubUpdateView {
    pub title: String,
    pub body: String,
    /// The command to run on the server, or `None` for a hub that gets the
    /// upgrade guide or that updates with the app.
    pub command: Option<String>,
    /// The card offers the upgrade guide.
    pub guide: bool,
}

/// The newest version that this app knows is `app` or `latest`. The card
/// shows only when that is newer than the hub.
pub fn hub_update_view(
    hub_version: &str,
    install: Option<InstallKind>,
    app: &Version,
    latest: Option<&Version>,
) -> Option<HubUpdateView> {
    let hub = Version::parse(hub_version)?;
    let newest = latest.filter(|latest| *latest > app).unwrap_or(app);
    if *newest <= hub {
        return None;
    }
    let command = update_command(install, None);
    let next = match (install, &command) {
        (Some(InstallKind::Local), _) => "It updates with this app.",
        (_, Some(_)) => "Back up first. Run this on the server.",
        (_, None) => "Back up first. The upgrade guide says what to do on the server.",
    };
    Some(HubUpdateView {
        title: format!("Version {newest} is available"),
        body: format!("This hub runs {hub}, and it still works with this app. {next}"),
        guide: command.is_none() && install != Some(InstallKind::Local),
        command,
    })
}

/// How a line about a worker update looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkerUpdateTone {
    /// It waits.
    #[default]
    Quiet,
    /// It runs now.
    Working,
    /// It failed, and the detail says why.
    Failed,
}

/// Where the update of a worker is, for Settings > Workers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkerUpdateView {
    /// The short text on the right of the row.
    pub line: String,
    pub tone: WorkerUpdateTone,
    /// The full text in the detail of the worker. A user may copy it.
    pub detail: Option<String>,
}

/// The text for a status. `None` for a status that a newer worker sent and
/// that this app does not know.
pub fn worker_update_view(status: &WorkerUpdateStatus) -> Option<WorkerUpdateView> {
    Some(match status {
        WorkerUpdateStatus::Downloading { version }
        | WorkerUpdateStatus::Restarting { version } => WorkerUpdateView {
            line: format!("Updating to {version}"),
            tone: WorkerUpdateTone::Working,
            detail: None,
        },
        WorkerUpdateStatus::WaitingForWork { version } => WorkerUpdateView {
            line: "Updates when its task ends".to_owned(),
            tone: WorkerUpdateTone::Quiet,
            detail: Some(format!(
                "Version {version} is ready. This worker restarts when no work runs."
            )),
        },
        WorkerUpdateStatus::Failed { version, message } => WorkerUpdateView {
            line: format!("Could not update to {version}"),
            tone: WorkerUpdateTone::Failed,
            detail: Some(format!(
                "This worker could not update to {version}: {message}. It tries again in one hour."
            )),
        },
        WorkerUpdateStatus::NotAvailable { version } => WorkerUpdateView {
            line: format!("Update it to {version} by hand"),
            tone: WorkerUpdateTone::Quiet,
            detail: Some(format!(
                "This worker runs from a build folder, so it does not replace itself. Build version {version}, or install a release."
            )),
        },
        WorkerUpdateStatus::Unknown => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(version: &str, protocols: &[u32], install: Option<InstallKind>) -> HubVersions {
        HubVersions {
            hub_version: version.to_owned(),
            protocol_versions: protocols.to_vec(),
            install,
        }
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn the_command_follows_the_install_kind() {
        let command = |install, version| update_command(Some(install), version);
        assert_eq!(
            command(InstallKind::Service, Some("0.8.0")).as_deref(),
            Some("sudo consort hub update --version 0.8.0")
        );
        assert_eq!(
            command(InstallKind::Service, None).as_deref(),
            Some("sudo consort hub update")
        );
        assert_eq!(
            command(InstallKind::Manual, Some("0.8.0")).as_deref(),
            Some("consort hub update --version 0.8.0")
        );
        assert!(
            command(InstallKind::Docker, Some("0.8.0"))
                .unwrap()
                .contains("docker compose")
        );
        assert_eq!(command(InstallKind::Local, None), None);
        // A hub that does not say is too old to have the command.
        assert_eq!(update_command(None, Some("0.8.0")), None);
    }

    #[test]
    fn the_hub_too_old_card_names_both_versions_and_the_command() {
        let view = incompatible_view(
            TooOld::Hub,
            Some(&hub("0.6.2", &[1], Some(InstallKind::Service))),
            "0.8.0",
        );
        assert_eq!(view.title, "The hub needs an update");
        assert!(view.body.contains("This app runs 0.8.0."));
        assert!(view.body.contains("Your hub runs 0.6.2"));
        assert_eq!(
            view.command.as_deref(),
            Some("sudo consort hub update --version 0.8.0")
        );
    }

    #[test]
    fn a_hub_that_names_nothing_gets_no_command() {
        let view = incompatible_view(TooOld::Hub, None, "0.8.0");
        assert!(view.body.contains("Your hub is too old"));
        assert_eq!(view.command, None);
    }

    #[test]
    fn the_app_too_old_card_has_no_command_for_the_server() {
        let view = incompatible_view(
            TooOld::ThisSide,
            Some(&hub("0.9.0", &[3], Some(InstallKind::Service))),
            "0.6.2",
        );
        assert_eq!(view.title, "This app needs an update");
        assert!(view.body.contains("Your hub runs 0.9.0"));
        assert!(view.body.contains("app 0.6.2"));
        assert_eq!(view.command, None);
    }

    #[test]
    fn the_pairing_error_says_which_side_is_too_old_and_that_the_code_still_works() {
        let (title, detail) = pair_error(Some(&hub("0.6.2", &[0], None)));
        assert_eq!(title, "This hub is too old for this app");
        assert!(detail.contains("The hub runs 0.6.2."), "{detail}");
        assert!(detail.contains("or later"), "{detail}");
        assert!(detail.contains("The setup code still works."), "{detail}");
        let (title, detail) = pair_error(Some(&hub("0.9.0", &[99], None)));
        assert_eq!(title, "This app is too old for this hub");
        assert!(detail.contains("Update this app"), "{detail}");
        let (title, _) = pair_error(None);
        assert_eq!(title, "This hub is too old for this app");
    }

    #[test]
    fn the_settings_card_shows_only_for_a_newer_version() {
        let app = v("0.8.0");
        let service = Some(InstallKind::Service);
        let card = hub_update_view("0.7.1", service, &app, None).unwrap();
        assert_eq!(card.title, "Version 0.8.0 is available");
        assert!(card.body.contains("This hub runs 0.7.1"));
        assert_eq!(card.command.as_deref(), Some("sudo consort hub update"));
        assert!(!card.guide);
        // A newer release counts too.
        let latest = v("0.9.0");
        let card = hub_update_view("0.8.0", service, &app, Some(&latest)).unwrap();
        assert_eq!(card.title, "Version 0.9.0 is available");
        // Nothing to say for a hub that is current, ahead, or has no version.
        assert_eq!(hub_update_view("0.8.0", service, &app, None), None);
        assert_eq!(hub_update_view("0.9.0", service, &app, None), None);
        assert_eq!(hub_update_view("test", service, &app, None), None);
    }

    #[test]
    fn the_settings_card_of_a_hub_with_no_command_offers_the_guide_or_nothing() {
        let app = v("0.8.0");
        let card = hub_update_view("0.7.1", None, &app, None).unwrap();
        assert_eq!(card.command, None);
        assert!(card.guide);
        let card = hub_update_view("0.7.1", Some(InstallKind::Local), &app, None).unwrap();
        assert_eq!(card.command, None);
        assert!(!card.guide);
    }

    #[test]
    fn each_worker_status_has_a_line() {
        let view = |status| worker_update_view(&status).unwrap();
        let version = || "0.8.0".to_owned();
        assert_eq!(
            view(WorkerUpdateStatus::Downloading { version: version() }).line,
            "Updating to 0.8.0"
        );
        assert_eq!(
            view(WorkerUpdateStatus::Restarting { version: version() }).tone,
            WorkerUpdateTone::Working
        );
        assert_eq!(
            view(WorkerUpdateStatus::WaitingForWork { version: version() }).line,
            "Updates when its task ends"
        );
        let failed = view(WorkerUpdateStatus::Failed {
            version: version(),
            message: "the disk is full".into(),
        });
        assert_eq!(failed.tone, WorkerUpdateTone::Failed);
        assert!(failed.detail.unwrap().contains("the disk is full"));
        assert!(worker_update_view(&WorkerUpdateStatus::Unknown).is_none());
    }
}
