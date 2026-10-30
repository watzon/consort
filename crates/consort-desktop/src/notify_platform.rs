//! The system side of notifications.
//!
//! - **Posting** uses GPUI's own `show_system_notification`: on macOS the
//!   `UNUserNotificationCenter` API, on Windows a toast, on Linux the
//!   desktop notification service. A click comes back through
//!   `on_system_notification_response` with the tag. On macOS, GPUI posts
//!   only when the app runs from an app bundle; a `cargo run` build has no
//!   bundle, so macOS shows nothing for it.
//! - **The Dock badge** on macOS is `NSApp.dockTile.badgeLabel`, through
//!   the safe `objc2-app-kit` bindings. Windows and Linux have no Dock: the
//!   app shows the count in the window title instead.
//! - **The permission state** on macOS comes from
//!   `UNUserNotificationCenter.getNotificationSettings`, through the
//!   `mac-usernotifications` crate. Reading it without a bundle would stop
//!   the process, so the crate checks for a bundle first.
//!
//! Consort has no `unsafe` code. The `unsafe` of the system calls stays in
//! GPUI, `objc2-app-kit` and `mac-usernotifications`.

use consort_client::notify::Notification;
use gpui_kit::{App, SharedString, SystemNotification};

/// Whether this system has a Dock icon that can show a count.
pub const HAS_DOCK_BADGE: bool = cfg!(target_os = "macos");

/// Whether the system lets Consort show notifications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Permission {
    /// The app is asking.
    Checking,
    /// Banners and alerts show.
    Allowed,
    /// Notifications go to Notification Center quietly.
    Quiet,
    /// The user turned them off.
    Denied,
    /// The system asks at the first notification.
    NotAsked,
    /// macOS cannot show notifications for a build that is not an app
    /// bundle.
    NoBundle,
    /// Windows or Linux: the app cannot read the state, and the system
    /// shows notifications unless the user turned them off there.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    System,
    /// The state could not be read.
    Unknown,
}

impl Permission {
    /// True when notifications show, false when they do not, `None` when
    /// the app does not know.
    pub fn allowed(&self) -> Option<bool> {
        match self {
            Permission::Allowed | Permission::Quiet => Some(true),
            Permission::Denied | Permission::NoBundle => Some(false),
            _ => None,
        }
    }

    /// The line under the switches of Settings > Notifications.
    pub fn line(&self) -> String {
        let os = system_name();
        match self {
            Permission::Checking => format!("Checking whether {os} allows notifications from Consort."),
            Permission::Allowed => format!(
                "{os} allows notifications from Consort. To change their style or sound, open {}.",
                settings_place()
            ),
            Permission::Quiet => format!(
                "{os} delivers notifications from Consort quietly, to Notification Center only. To show banners, open {}.",
                settings_place()
            ),
            Permission::Denied => format!(
                "{os} does not allow notifications from Consort. To turn them on, open {}.",
                settings_place()
            ),
            Permission::NotAsked => format!(
                "{os} asks once whether Consort may show notifications. Choose Test notification to see the question."
            ),
            Permission::NoBundle => "This build of Consort does not run from an app bundle, so macOS does not show its notifications. A build from the installer can. The Dock count still works.".to_owned(),
            Permission::System => format!(
                "{os} shows notifications from Consort. To change them, open {}.",
                settings_place()
            ),
            Permission::Unknown => format!(
                "Consort cannot read whether {os} allows its notifications. To check, open {}.",
                settings_place()
            ),
        }
    }
}

/// The name of this system in copy.
pub fn system_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        _ => "Your desktop",
    }
}

/// Where the user changes notifications on this system.
fn settings_place() -> &'static str {
    match std::env::consts::OS {
        "macos" => "System Settings > Notifications",
        "windows" => "Settings > System > Notifications",
        _ => "the notification settings of your desktop",
    }
}

/// Posts one notification. A notification with the same tag replaces the
/// one before, where the system can.
pub fn post(notification: &Notification, cx: &App) {
    cx.show_system_notification(SystemNotification {
        tag: SharedString::from(notification.tag.clone()),
        title: SharedString::from(notification.title.clone()),
        body: SharedString::from(notification.body.clone()),
        actions: Vec::new(),
    });
}

/// Sets the count on the Dock icon, or takes it away with `None`.
#[cfg(target_os = "macos")]
pub fn set_badge(label: Option<&str>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;
    // AppKit must be used on the main thread, where GPUI runs the app.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let text = label.map(NSString::from_str);
    NSApplication::sharedApplication(mtm)
        .dockTile()
        .setBadgeLabel(text.as_deref());
}

/// Windows and Linux have no Dock badge. The window title shows the count
/// (see `ConsortApp::apply_window_badge`).
#[cfg(not(target_os = "macos"))]
pub fn set_badge(_label: Option<&str>) {}

/// Reads whether the system lets Consort notify.
#[cfg(target_os = "macos")]
pub async fn permission() -> Permission {
    use mac_usernotifications::AuthorizationStatus;
    if mac_usernotifications::check_bundle().is_err() {
        return Permission::NoBundle;
    }
    match mac_usernotifications::get_notification_settings().await {
        Ok(settings) => match settings.authorization_status {
            AuthorizationStatus::Authorized | AuthorizationStatus::Ephemeral => Permission::Allowed,
            AuthorizationStatus::Provisional => Permission::Quiet,
            AuthorizationStatus::Denied => Permission::Denied,
            AuthorizationStatus::NotDetermined => Permission::NotAsked,
            AuthorizationStatus::Unknown => Permission::Unknown,
        },
        Err(_) => Permission::Unknown,
    }
}

/// Windows and Linux: the app cannot read the state.
#[cfg(not(target_os = "macos"))]
pub async fn permission() -> Permission {
    Permission::System
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_permission_line_says_what_to_do() {
        for permission in [
            Permission::Checking,
            Permission::Allowed,
            Permission::Quiet,
            Permission::Denied,
            Permission::NotAsked,
            Permission::NoBundle,
            Permission::System,
            Permission::Unknown,
        ] {
            let line = permission.line();
            assert!(line.ends_with('.'), "{line}");
            assert!(!line.contains('—'), "{line}");
        }
        #[cfg(target_os = "macos")]
        assert_eq!(
            Permission::Allowed.line(),
            "macOS allows notifications from Consort. To change their style or sound, open System Settings > Notifications."
        );
    }
}
