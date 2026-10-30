//! Phase 3 in the app: the state and the actions of Settings > Hub,
//! Settings > Clients, the Pair a new app dialog and Settings >
//! Notifications, and the delivery of notifications.
//!
//! The notification rules live in `consort_client::notify`. This module
//! only gives them the focus of the window and the user's switches, and
//! delivers what they return: a system notification through GPUI, and the
//! count on the Dock icon (see [`crate::notify_platform`]).

use std::collections::HashMap;
use std::time::Duration;

use consort_client::admin::{BackupChanges, BackupFrequency, KEEP_CHOICES, Weekday, backup_times};
use consort_client::notify::{
    Focus, Notification, NotificationSettings, NotificationTarget, Notifier, badge_label,
};
use consort_client::{Client, Command};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AppContext as _, Context, Entity, Subscription, Window};

use crate::app::{ConsortApp, Dialog, Page, Panel, SettingsSection};
use crate::notify_platform::{self, Permission};

/// How many delivered notifications keep their click target.
const TARGETS_KEPT: usize = 200;

/// What the phase 3 pages keep that is not in the view state.
pub struct AdminState {
    /// The client row whose inline Revoke confirm is open.
    pub revoke_confirm: Option<String>,
    /// The backup folder field is open.
    pub folder_editing: bool,
    pub folder_input: Entity<InputState>,
    /// The user's notification switches. They live on this computer.
    pub settings: NotificationSettings,
    pub notifier: Notifier,
    /// Where a click on a delivered notification goes, by tag.
    pub targets: HashMap<String, NotificationTarget>,
    /// The tags in the order they were delivered, to forget old ones.
    target_order: Vec<String>,
    /// The badge that the Dock icon shows now.
    pub badge: Option<String>,
    /// Whether the system lets Consort notify.
    pub permission: Permission,
    /// The countdown of the pairing dialog redraws every second.
    pairing_ticker: bool,
}

impl AdminState {
    pub fn new(
        client: &Client,
        demo: bool,
        window: &mut Window,
        cx: &mut Context<ConsortApp>,
    ) -> (AdminState, Vec<Subscription>) {
        let folder_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("/srv/backups/consort"));
        let subscription = cx.subscribe_in(
            &folder_input,
            window,
            |this: &mut ConsortApp, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_backup_folder(window, cx);
                }
            },
        );
        let permission = if demo {
            // The mock shows what the design shows.
            Permission::Allowed
        } else {
            Permission::Checking
        };
        let state = AdminState {
            revoke_confirm: None,
            folder_editing: false,
            folder_input,
            settings: client.load_notification_settings(),
            notifier: Notifier::new(),
            targets: HashMap::new(),
            target_order: Vec::new(),
            badge: None,
            permission,
            pairing_ticker: false,
        };
        (state, vec![subscription])
    }

    fn remember(&mut self, notification: &Notification) {
        if self
            .targets
            .insert(notification.tag.clone(), notification.target.clone())
            .is_none()
        {
            self.target_order.push(notification.tag.clone());
        }
        while self.target_order.len() > TARGETS_KEPT {
            let old = self.target_order.remove(0);
            self.targets.remove(&old);
        }
    }
}

/// The ID of a select of these pages, for `MenuAction::Select`.
pub const TIME_SELECT: &str = "backup-time";
pub const WEEKDAY_SELECT: &str = "backup-weekday";
pub const KEEP_SELECT: &str = "backup-keep";

/// The menu items of the backup selects: `(label, option, checked)`.
pub fn select_options(
    select: &str,
    state: &consort_client::admin::BackupView,
) -> Vec<(String, String, bool)> {
    match select {
        TIME_SELECT => backup_times()
            .into_iter()
            .map(|(time, label)| {
                let checked = time == state.time;
                (label, time, checked)
            })
            .collect(),
        WEEKDAY_SELECT => Weekday::ALL
            .iter()
            .map(|d| {
                (
                    format!("On {}", d.name()),
                    d.name().to_owned(),
                    *d == state.weekday,
                )
            })
            .collect(),
        KEEP_SELECT => {
            let mut counts: Vec<u32> = KEEP_CHOICES.to_vec();
            if !counts.contains(&state.keep) {
                counts.push(state.keep);
                counts.sort_unstable();
            }
            counts
                .into_iter()
                .map(|k| {
                    (
                        consort_client::admin::keep_label(k),
                        k.to_string(),
                        k == state.keep,
                    )
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The change that a pick of a backup select makes.
pub fn backup_pick(select: &str, option: &str) -> Option<BackupChanges> {
    match select {
        TIME_SELECT => consort_client::admin::backup_times()
            .into_iter()
            .find(|(time, _)| time == option)
            .map(|(time, _)| BackupChanges {
                time: Some(time),
                ..BackupChanges::default()
            }),
        WEEKDAY_SELECT => Weekday::ALL
            .iter()
            .find(|d| d.name() == option)
            .map(|d| BackupChanges {
                weekday: Some(*d),
                ..BackupChanges::default()
            }),
        KEEP_SELECT => option.parse().ok().map(|keep| BackupChanges {
            keep: Some(keep),
            ..BackupChanges::default()
        }),
        _ => None,
    }
}

impl ConsortApp {
    /// Loads what a phase 3 page shows when it opens.
    pub fn enter_admin_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        let client = self.client.clone();
        match section {
            SettingsSection::Hub => {
                self.admin.folder_editing = false;
                self.spawn_command(cx, async move { client.run(Command::LoadHubStatus).await });
            }
            SettingsSection::Clients => {
                self.admin.revoke_confirm = None;
                self.spawn_command(cx, async move { client.run(Command::LoadClients).await });
            }
            SettingsSection::Notifications => self.check_permission(cx),
            _ => {}
        }
    }

    /// Asks the system whether Consort may notify, and keeps the answer.
    pub fn check_permission(&mut self, cx: &mut Context<Self>) {
        if self.demo {
            return;
        }
        cx.spawn(async move |this, cx| {
            let permission = notify_platform::permission().await;
            let _ = this.update(cx, |this, cx| {
                this.admin.permission = permission;
                cx.notify();
            });
        })
        .detach();
    }

    /// What is on screen, for the notification rules.
    pub fn notification_focus(&self) -> Focus {
        Focus {
            channel_id: (self.page == Page::Main).then(|| self.channel_id.clone()),
            task_id: match (&self.page, &self.panel) {
                (Page::Main, Panel::Thread(task_id)) => Some(task_id.clone()),
                _ => None,
            },
            active: self.window_active,
        }
    }

    /// Follows a new view state: delivers new notifications, sets the
    /// Dock badge, and closes an inline confirm whose row went.
    pub fn follow_admin(&mut self, cx: &mut Context<Self>) {
        let focus = self.notification_focus();
        let fresh =
            self.admin
                .notifier
                .observe(&self.state.admin.attention, &focus, &self.admin.settings);
        for notification in fresh {
            self.deliver(&notification, cx);
        }
        self.update_badge();
        if let Some(id) = &self.admin.revoke_confirm
            && !self.state.admin.clients.rows.iter().any(|r| &r.id == id)
        {
            self.admin.revoke_confirm = None;
        }
    }

    fn deliver(&mut self, notification: &Notification, cx: &mut Context<Self>) {
        self.admin.remember(notification);
        notify_platform::post(notification, cx);
    }

    /// Sets the count on the Dock icon when it changed. The mock and the
    /// tests leave the real Dock alone.
    pub fn update_badge(&mut self) {
        let label = badge_label(&self.state.admin.attention, &self.admin.settings);
        if label == self.admin.badge {
            return;
        }
        self.admin.badge = label;
        if !self.demo {
            notify_platform::set_badge(self.admin.badge.as_deref());
        }
    }

    /// The window title, with the count where the system has no Dock badge.
    pub fn apply_window_badge(&self, window: &mut Window) {
        if notify_platform::HAS_DOCK_BADGE || self.demo {
            return;
        }
        window.set_window_title(&window_title(self.admin.badge.as_deref()));
    }

    /// Opens the place of a notification that the user clicked.
    pub fn open_notification(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.admin.targets.get(tag).cloned();
        cx.activate(true);
        match target {
            Some(NotificationTarget::Task {
                channel_id,
                task_id,
            }) => {
                self.page = Page::Main;
                if self.state.channel(&channel_id).is_some() {
                    self.open_channel(channel_id, window, cx);
                }
                self.open_thread(task_id, window, cx);
            }
            Some(NotificationTarget::Channel { channel_id }) => {
                self.page = Page::Main;
                if self.state.channel(&channel_id).is_some() {
                    self.open_channel(channel_id, window, cx);
                }
            }
            Some(NotificationTarget::App) | None => {}
        }
        cx.notify();
    }

    /// Changes one notification switch and saves the switches.
    pub fn set_notification_setting(
        &mut self,
        change: impl FnOnce(&mut NotificationSettings),
        cx: &mut Context<Self>,
    ) {
        change(&mut self.admin.settings);
        if let Err(err) = self.client.save_notification_settings(&self.admin.settings) {
            self.show_error(err.to_string(), cx);
        }
        self.update_badge();
        cx.notify();
    }

    /// Test notification.
    pub fn send_test_notification(&mut self, cx: &mut Context<Self>) {
        let test = consort_client::notify::test_notification();
        self.deliver(&test, cx);
        let message = match self.admin.permission {
            Permission::NoBundle => {
                "Consort sent a test notification, but macOS shows none for this build, because it does not run from an app bundle."
            }
            Permission::Denied => {
                "Consort sent a test notification, but the system does not allow notifications from Consort. Turn them on in the system settings."
            }
            _ => "Consort sent a test notification.",
        };
        self.show_notice(message.to_owned(), cx);
        // The first notification can make the system ask. Read the answer
        // again a little later.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |this, cx| this.check_permission(cx));
        })
        .detach();
    }

    // Settings > Hub

    pub fn back_up_now(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.run(Command::BackUpNow).await });
    }

    pub fn change_backup(&mut self, changes: BackupChanges, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(Command::UpdateBackupSettings(changes)).await
        });
    }

    pub fn set_backup_frequency(&mut self, frequency: BackupFrequency, cx: &mut Context<Self>) {
        self.change_backup(
            BackupChanges {
                frequency: Some(frequency),
                ..BackupChanges::default()
            },
            cx,
        );
    }

    /// Opens the folder field with the folder in it.
    pub fn edit_backup_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder = self.state.admin.hub.backup.folder.clone();
        self.admin.folder_input.update(cx, |input, cx| {
            input.set_value(folder, window, cx);
            input.focus(window, cx);
        });
        self.admin.folder_editing = true;
        cx.notify();
    }

    /// Saves the folder in the field. The hub checks that it can write
    /// there; its answer shows under the field when it cannot.
    pub fn save_backup_folder(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let folder = self.admin.folder_input.read(cx).value().trim().to_owned();
        if folder.is_empty() {
            self.show_error(
                "Enter the full path of a folder on the hub machine.".to_owned(),
                cx,
            );
            return;
        }
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .run(Command::UpdateBackupSettings(BackupChanges {
                    folder: Some(folder),
                    ..BackupChanges::default()
                }))
                .await;
            let _ = this.update(cx, |this, cx| {
                // On an error the field stays open, with the reason under it.
                if result.is_ok() {
                    this.admin.folder_editing = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A pick from one of the backup selects. Returns false for a select of
    /// another page.
    pub fn pick_admin_select(
        &mut self,
        select: &str,
        option: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if ![TIME_SELECT, WEEKDAY_SELECT, KEEP_SELECT].contains(&select) {
            return false;
        }
        match backup_pick(select, option) {
            Some(changes) => self.change_backup(changes, cx),
            None => self.show_notice(
                "That choice is not in the list any more. Open the list and choose again.".into(),
                cx,
            ),
        }
        true
    }

    // Settings > Clients

    pub fn revoke_client(&mut self, client_id: String, cx: &mut Context<Self>) {
        self.admin.revoke_confirm = None;
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(Command::RevokeClient { client_id }).await
        });
        cx.notify();
    }

    /// Opens the Pair a new app dialog: asks for a code, and redraws the
    /// countdown every second while the dialog is open.
    pub fn start_client_pairing(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            if let Err(err) = client.run(Command::ClientPairingCode).await {
                let _ = this.update(cx, |this, cx| {
                    if this.dialog == Some(Dialog::PairClient) {
                        this.dialog_error = Some(err.to_string());
                    } else {
                        this.show_error(err.to_string(), cx);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
        if self.admin.pairing_ticker {
            return;
        }
        self.admin.pairing_ticker = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let open = this
                    .update(cx, |this, cx| {
                        let open = this.dialog == Some(Dialog::PairClient);
                        if open {
                            cx.notify();
                        } else {
                            this.admin.pairing_ticker = false;
                        }
                        open
                    })
                    .unwrap_or(false);
                if !open {
                    break;
                }
            }
        })
        .detach();
    }

    /// New code in the pairing dialog.
    pub fn new_client_code(&mut self, cx: &mut Context<Self>) {
        self.dialog_error = None;
        let client = self.client.clone();
        self.spawn_command(
            cx,
            async move { client.run(Command::ClientPairingCode).await },
        );
    }

    /// The pairing dialog closed: the code is forgotten.
    pub fn end_client_pairing(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(
            cx,
            async move { client.run(Command::CancelClientPairing).await },
        );
    }

    /// The screens of phase 3, for `--screen`. Returns false for another
    /// name.
    pub fn open_admin_screen(
        &mut self,
        screen: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match screen {
            "hub" => self.open_settings(SettingsSection::Hub, cx),
            "clients" => self.open_settings(SettingsSection::Clients, cx),
            "clients-revoke" => {
                self.open_settings(SettingsSection::Clients, cx);
                self.admin.revoke_confirm = self
                    .state
                    .admin
                    .clients
                    .rows
                    .iter()
                    .find(|r| r.name == "Mac mini")
                    .map(|r| r.id.clone());
            }
            "pair-client" => {
                self.open_settings(SettingsSection::Clients, cx);
                self.open_dialog(Dialog::PairClient, window, cx);
            }
            "notifications" => self.open_settings(SettingsSection::Notifications, cx),
            _ => return false,
        }
        true
    }
}

/// The window title: the app name, and the count when there is one.
fn window_title(badge: Option<&str>) -> String {
    match badge {
        Some(count) => format!("Consort ({count})"),
        None => "Consort".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backup_selects_offer_their_choices_and_mark_the_current_one() {
        let view = consort_client::mock_admin::admin().hub.backup;
        let times = select_options(TIME_SELECT, &view);
        assert_eq!(times.len(), 24);
        assert!(
            times
                .iter()
                .any(|(label, time, checked)| label == "At 3:00 AM" && time == "03:00" && *checked)
        );
        let days = select_options(WEEKDAY_SELECT, &view);
        assert_eq!(days[0].0, "On Monday");
        assert!(days[6].2, "Sunday is the default day");
        let keep = select_options(KEEP_SELECT, &view);
        assert!(
            keep.iter()
                .any(|(label, _, checked)| label == "The last 7 backups" && *checked)
        );
    }

    #[test]
    fn a_backup_pick_becomes_one_change() {
        assert_eq!(
            backup_pick(TIME_SELECT, "05:00").unwrap().time.as_deref(),
            Some("05:00")
        );
        assert_eq!(
            backup_pick(WEEKDAY_SELECT, "Friday").unwrap().weekday,
            Some(Weekday::Friday)
        );
        assert_eq!(backup_pick(KEEP_SELECT, "14").unwrap().keep, Some(14));
        assert!(backup_pick(TIME_SELECT, "25:00").is_none());
        assert!(backup_pick("other", "1").is_none());
    }

    #[test]
    fn the_window_title_shows_the_count_where_there_is_no_dock_badge() {
        assert_eq!(window_title(None), "Consort");
        assert_eq!(window_title(Some("3")), "Consort (3)");
        assert_eq!(window_title(Some("99+")), "Consort (99+)");
    }
}
