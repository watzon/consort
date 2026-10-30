//! Menus and dropdowns. One layer for every right-click menu and select, so
//! they look the same and all flip: a menu opens downward when there is room
//! and upward when there is not (GPUI's `anchored` with anchor switching).

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    Anchor, AnyElement, App, Bounds, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement as _, Styled, Window,
    anchored, canvas, deferred, div, point, prelude::FluentBuilder as _,
};

use super::parts::icon;

/// What a menu item does. The app runs it in `ConsortApp::run_menu`.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Copy(String),
    QuoteInReply(String),
    AskConsort(String),
    CopyMarkdown(String),
    /// Open the thread of a message, by its ID.
    ReplyInThread(String),
    /// Phase 4: reactions, groups, tasks and channel settings. See
    /// `crate::convo`.
    Convo(crate::convo::ConvoAction),
    /// Edit a message in place: its ID and its text.
    EditMessage(String, String),
    DeleteMessage(String),
    OpenThread(String),
    ResolveTask(String),
    ReopenTask(String),
    RetryTask(String),
    StopTask(String),
    /// Hide a failed or resolved task.
    ArchiveTask(String),
    RenameChannel(String),
    ChannelInstructions(String),
    ChangeIcon(String),
    SetIcon(String, String),
    MoveToGroup(String),
    SetGroup(String, String),
    /// Take a channel out of its group.
    NoGroup(String),
    /// Open the dialog that makes a group. With a channel ID, the channel
    /// moves into the new group.
    NewGroup(Option<String>),
    ArchiveChannel(String),
    UnarchiveChannel(String),
    DeleteChannel(String),
    RenameWorker(String),
    EditTags(String),
    /// The Worker tags dialog of a channel.
    ChannelTags(String),
    /// The model picker for a channel's assistant.
    ChannelModel(String),
    /// The worker ID and the account ID.
    RenameAccount(String, String),
    SignOutAccount(String, String),
    RemoveAccount(String, String),
    PauseWorker(String, bool),
    RemoveWorker(String),
    /// The Connectors and skills dialog of a channel.
    ChannelExtensions(String),
    /// Sign a connection in again.
    ReconnectConnection(String),
    /// Ask before a connection is disconnected.
    Disconnect(String),
    /// The calls that the assistant made with a connection.
    ConnectionActivity(String),
    /// An approval rule: of a connection, or of the Connect dialog with
    /// `None`.
    SetRule(
        Option<String>,
        consort_client::ToolKindView,
        consort_client::ApprovalRuleView,
    ),
    /// Add a tag to a Use in, or take it away.
    ToggleScopeTag(crate::extensions::UseInTarget, String),
    SetSkillEnabled(String, bool),
    SkillFiles(String),
    RemoveSkill(String),
    /// Remove a skill source that the user added.
    RemoveSkillSource(String),
    /// The worker ID and the account ID.
    AccountSettings(String, String),
    RemoveComposioKey,
    /// Cut, Copy, Paste or Select all in one of the message boxes.
    TextEdit(TextTarget, EditOp),
    /// Pick the option with the stable ID `option` in the select `id`. The ID
    /// is the ID of the thing picked, so a state change between the opening
    /// of the menu and the click cannot move the pick to another row.
    Select(&'static str, String),
    /// Open an attachment with the app of the system.
    OpenAttachment(consort_client::AttachmentView),
    /// Save a copy of an attachment on this computer.
    SaveAttachment(consort_client::AttachmentView),
    /// Put an attachment in the files of a channel.
    SaveToChannelFiles(String, consort_client::AttachmentView),
    /// The add menu: Upload from this computer.
    AttachFromComputer(consort_client::UploadTarget),
    /// The add menu: Choose from channel files.
    AttachFromChannel(consort_client::UploadTarget),
    RetryUpload(String),
    RemoveUpload(String),
    /// A channel file: Open, Download.
    OpenFile(consort_client::ChannelFileView),
    DownloadFile(consort_client::ChannelFileView),
    /// A channel file: the channel ID and the path.
    RenameFile(String, String),
    MoveFile(String, String),
    DeleteFile(String, String),
}

/// The menu of an attachment in a message or a thread post: Open, Save, and
/// Save to channel files. An artifact is in the channel files already, so it
/// has no Save to channel files.
pub fn attachment_menu(
    attachment: &consort_client::AttachmentView,
    channel_id: &str,
) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::new("Open", MenuAction::OpenAttachment(attachment.clone())),
        MenuItem::new("Save", MenuAction::SaveAttachment(attachment.clone())),
    ];
    if attachment.artifact.is_none() {
        items.push(
            MenuItem::new(
                "Save to channel files",
                MenuAction::SaveToChannelFiles(channel_id.to_owned(), attachment.clone()),
            )
            .separated(),
        );
    }
    items
}

/// The menu of a file row in the Files tab: Open, Download, Rename, Move,
/// then Delete in `status.failed`.
pub fn file_menu(file: &consort_client::ChannelFileView) -> Vec<MenuItem> {
    let (channel, path) = (file.channel_id.clone(), file.path.clone());
    vec![
        MenuItem::new("Open", MenuAction::OpenFile(file.clone())),
        MenuItem::new("Download", MenuAction::DownloadFile(file.clone())),
        MenuItem::new(
            "Rename",
            MenuAction::RenameFile(channel.clone(), path.clone()),
        ),
        MenuItem::new("Move", MenuAction::MoveFile(channel.clone(), path.clone())),
        MenuItem::new("Delete", MenuAction::DeleteFile(channel, path))
            .danger()
            .separated(),
    ]
}

/// The add menu of the composer and the reply box.
pub fn add_menu(target: &consort_client::UploadTarget) -> Vec<MenuItem> {
    vec![
        MenuItem::new(
            "Upload from this computer",
            MenuAction::AttachFromComputer(target.clone()),
        )
        .icon("upload"),
        MenuItem::new(
            "Choose from channel files",
            MenuAction::AttachFromChannel(target.clone()),
        )
        .icon("folder"),
    ]
}

/// The menu of an attachment chip in the composer or the reply box: Try
/// again (a failed upload only), then Remove.
pub fn upload_menu(upload: &consort_client::UploadView) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if matches!(upload.status, consort_client::UploadStatus::Failed(_)) {
        items.push(MenuItem::new(
            "Try again",
            MenuAction::RetryUpload(upload.id.clone()),
        ));
    }
    items.push(MenuItem::new(
        "Remove",
        MenuAction::RemoveUpload(upload.id.clone()),
    ));
    items
}

/// The message box that a Cut, Copy, Paste or Select all acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextTarget {
    /// The composer under the timeline.
    Composer,
    /// The reply box in a task thread.
    Reply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditOp {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The menu of a worker row, in the sidebar and in Settings: Rename, Edit
/// tags, Pause (Resume when it is paused) and Remove. Every surface that
/// shows a worker builds its menu here, so they never differ.
pub fn worker_menu(worker_id: &str, paused: bool) -> Vec<MenuItem> {
    let id = worker_id.to_owned();
    vec![
        MenuItem::new("Rename", MenuAction::RenameWorker(id.clone())),
        MenuItem::new("Edit tags", MenuAction::EditTags(id.clone())),
        MenuItem::new(
            if paused { "Resume" } else { "Pause" },
            MenuAction::PauseWorker(id.clone(), !paused),
        ),
        MenuItem::new("Remove", MenuAction::RemoveWorker(id))
            .danger()
            .separated(),
    ]
}

/// The menu of an account row in Settings > Workers: Account settings,
/// Rename, Sign out and Remove. Sign out is there only for an account that is signed in or has a
/// saved key.
pub fn account_menu(worker_id: &str, account_id: &str, can_sign_out: bool) -> Vec<MenuItem> {
    let (w, a) = (worker_id.to_owned(), account_id.to_owned());
    let mut items = vec![
        MenuItem::new(
            "Account settings",
            MenuAction::AccountSettings(w.clone(), a.clone()),
        ),
        MenuItem::new("Rename", MenuAction::RenameAccount(w.clone(), a.clone())),
    ];
    if can_sign_out {
        items.push(MenuItem::new(
            "Sign out",
            MenuAction::SignOutAccount(w.clone(), a.clone()),
        ));
    }
    items.push(
        MenuItem::new("Remove", MenuAction::RemoveAccount(w, a))
            .danger()
            .separated(),
    );
    items
}

/// The menu of a channel row: Rename, Instructions, Model, Worker tags,
/// Connectors and skills, Change icon, Move to group, Archive and Delete.
pub fn channel_menu(channel_id: &str) -> Vec<MenuItem> {
    let id = channel_id.to_owned();
    vec![
        MenuItem::new("Rename", MenuAction::RenameChannel(id.clone())),
        MenuItem::new("Instructions", MenuAction::ChannelInstructions(id.clone())),
        MenuItem::new("Model", MenuAction::ChannelModel(id.clone())),
        MenuItem::new("Worker tags", MenuAction::ChannelTags(id.clone())),
        MenuItem::new(
            "Connectors and skills",
            MenuAction::ChannelExtensions(id.clone()),
        ),
        MenuItem::new("Change icon", MenuAction::ChangeIcon(id.clone())),
        MenuItem::new("Move to group", MenuAction::MoveToGroup(id.clone())),
        MenuItem::new("Archive", MenuAction::ArchiveChannel(id.clone())).separated(),
        MenuItem::new("Delete", MenuAction::DeleteChannel(id)).danger(),
    ]
}

/// The menu of an archived channel: Unarchive and Delete.
pub fn archived_channel_menu(channel_id: &str) -> Vec<MenuItem> {
    let id = channel_id.to_owned();
    vec![
        MenuItem::new("Unarchive", MenuAction::UnarchiveChannel(id.clone())),
        MenuItem::new("Delete", MenuAction::DeleteChannel(id))
            .danger()
            .separated(),
    ]
}

/// The menu of a message box: Cut, Copy, Paste, Select all.
pub fn text_edit_menu(target: TextTarget) -> Vec<MenuItem> {
    vec![
        MenuItem::new("Cut", MenuAction::TextEdit(target, EditOp::Cut)),
        MenuItem::new("Copy", MenuAction::TextEdit(target, EditOp::Copy)),
        MenuItem::new("Paste", MenuAction::TextEdit(target, EditOp::Paste)),
        MenuItem::new(
            "Select all",
            MenuAction::TextEdit(target, EditOp::SelectAll),
        )
        .separated(),
    ]
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: SharedString,
    pub action: MenuAction,
    pub danger: bool,
    pub checked: bool,
    pub icon: Option<&'static str>,
    pub separator_before: bool,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>, action: MenuAction) -> MenuItem {
        MenuItem {
            label: label.into(),
            action,
            danger: false,
            checked: false,
            icon: None,
            separator_before: false,
        }
    }

    pub fn danger(mut self) -> MenuItem {
        self.danger = true;
        self
    }

    pub fn checked(mut self, checked: bool) -> MenuItem {
        self.checked = checked;
        self
    }

    pub fn icon(mut self, name: &'static str) -> MenuItem {
        self.icon = Some(name);
        self
    }

    pub fn separated(mut self) -> MenuItem {
        self.separator_before = true;
        self
    }
}

/// Where a menu opens.
#[derive(Clone, Copy, Debug)]
pub enum MenuAnchor {
    /// At the pointer, for a right click.
    Point(Point<Pixels>),
    /// Under a trigger, for a select or a "more" button. It opens above the
    /// trigger when there is no room below.
    Trigger(Bounds<Pixels>),
}

#[derive(Clone, Debug)]
pub struct OpenMenu {
    pub anchor: MenuAnchor,
    pub items: Vec<MenuItem>,
    pub width: Option<Pixels>,
    pub highlighted: Option<usize>,
    /// The quick emoji row at the top of a message menu.
    pub reactions: Option<crate::convo::ReactionBar>,
}

impl OpenMenu {
    pub fn at(position: Point<Pixels>, items: Vec<MenuItem>) -> OpenMenu {
        OpenMenu {
            anchor: MenuAnchor::Point(position),
            items,
            width: None,
            highlighted: None,
            reactions: None,
        }
    }

    pub fn under(bounds: Bounds<Pixels>, items: Vec<MenuItem>, match_width: bool) -> OpenMenu {
        OpenMenu {
            anchor: MenuAnchor::Trigger(bounds),
            items,
            width: match_width.then_some(bounds.size.width),
            highlighted: None,
            reactions: None,
        }
    }

    /// Moves the keyboard highlight by `delta` rows.
    pub fn step(&mut self, delta: isize) {
        let len = self.items.len() as isize;
        if len == 0 {
            return;
        }
        let next = match self.highlighted {
            None if delta > 0 => 0,
            None => len - 1,
            Some(i) => (i as isize + delta).rem_euclid(len),
        };
        self.highlighted = Some(next as usize);
    }
}

/// The bounds of named triggers, recorded while they paint, so a select can
/// open under its trigger.
#[derive(Clone, Default)]
pub struct TriggerBounds(Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>);

impl TriggerBounds {
    pub fn get(&self, id: &str) -> Option<Bounds<Pixels>> {
        self.0.borrow().get(id).copied()
    }

    /// An invisible child that records the bounds of its parent.
    pub fn track(&self, id: &'static str) -> impl IntoElement {
        let store = self.0.clone();
        canvas(
            move |bounds, _, _| {
                store.borrow_mut().insert(id, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// Renders the open menu above everything, with a layer behind it that
/// closes it on a click outside.
pub fn render_menu<V: 'static>(
    menu: &OpenMenu,
    t: &Tokens,
    window: &Window,
    cx: &mut Context<V>,
    on_pick: impl Fn(&mut V, MenuAction, &mut Window, &mut Context<V>) + Clone + 'static,
    on_close: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let c = &t.colors;
    let mut list = div()
        .id("menu")
        .occlude()
        .flex()
        .flex_col()
        .p(t.space.xs)
        .min_w(t.layout.speaker_gutter * 3.0)
        .when_some(menu.width, |d, w| d.w(w))
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(c.border_subtle)
        .rounded(t.radius.lg)
        .shadow(t.shadow_float())
        .font_family(t.font_sans.clone())
        .type_style(t.text.ui);
    if let Some(bar) = &menu.reactions {
        list = list
            .w(t.component.convo.menu_width)
            .child(crate::convo::render_reaction_bar(
                bar,
                t,
                cx,
                on_pick.clone(),
            ))
            .child(
                div()
                    .my(t.space.xs)
                    .mx(t.space.sm)
                    .h(t.border_width)
                    .bg(c.border_subtle),
            );
    }
    for (ix, item) in menu.items.iter().enumerate() {
        if item.separator_before && ix > 0 {
            list = list.child(
                div()
                    .my(t.space.xs)
                    .mx(t.space.sm)
                    .h(t.border_width)
                    .bg(c.border_subtle),
            );
        }
        let action = item.action.clone();
        let pick = on_pick.clone();
        let color = if item.danger {
            c.status_failed
        } else {
            c.text_primary
        };
        let highlighted = menu.highlighted == Some(ix);
        let hover = if item.danger {
            c.tint_failed
        } else {
            c.surface_selected
        };
        list = list.child(
            div()
                .id(("menu-item", ix))
                .flex()
                .items_center()
                .gap(t.space.sm)
                .h(t.space.xxl - t.space.xs)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .text_color(color)
                .cursor_pointer()
                .when(highlighted, |d| d.bg(hover))
                .hover(move |s| s.bg(hover))
                .when_some(item.icon, |d, name| {
                    d.child(icon(name, t.icon.size, c.text_secondary))
                })
                .child(div().flex_1().whitespace_nowrap().child(item.label.clone()))
                .when(item.checked, |d| {
                    d.child(icon("check", t.icon.dense, c.accent_text))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    pick(this, action.clone(), window, cx);
                })),
        );
    }
    floating(
        menu.anchor,
        list.into_any_element(),
        t,
        window,
        cx,
        on_close,
    )
}

/// Draws `content` at `anchor` above everything, with a layer behind it
/// that closes it on a click outside. A menu, a popover or a select uses it,
/// so all of them flip: the content opens downward when there is room and
/// upward when there is not (GPUI's `anchored` with anchor switching).
pub fn floating<V: 'static>(
    anchor: MenuAnchor,
    content: AnyElement,
    t: &Tokens,
    window: &Window,
    cx: &mut Context<V>,
    on_close: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let gap = t.space.xs;
    let (position, pad) = match anchor {
        MenuAnchor::Point(p) => (p, Pixels::ZERO),
        // From the middle of the trigger, padded by half its height and the
        // gap, the content clears the trigger whichever way it opens.
        MenuAnchor::Trigger(b) => (
            point(b.origin.x, b.origin.y + b.size.height / 2.0),
            b.size.height / 2.0 + gap,
        ),
    };
    let close = on_close.clone();
    let viewport = window.viewport_size();
    let catcher = div()
        .id("menu-catcher")
        .absolute()
        .top_0()
        .left_0()
        .w(viewport.width)
        .h(viewport.height)
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| close(this, window, cx)),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _, window, cx| on_close(this, window, cx)),
        );
    deferred(
        anchored()
            .position(point(Pixels::ZERO, Pixels::ZERO))
            .child(
                div()
                    .w(viewport.width)
                    .h(viewport.height)
                    .child(catcher)
                    .child(
                        anchored()
                            .position(position)
                            .anchor(Anchor::TopLeft)
                            .child(div().py(pad).child(content)),
                    ),
            ),
    )
    .with_priority(2)
    .into_any_element()
}

/// Keeps `App` in the import list for doc links on every platform.
#[allow(dead_code)]
fn _app(_: &App) {}

/// The menu of a skill source row in Settings > Skills: Remove. Only a source
/// that the user added has a menu. The built-in sources cannot go.
pub fn skill_source_menu(source: &consort_client::DirectorySourceView) -> Option<Vec<MenuItem>> {
    source.removable.then(|| {
        vec![MenuItem::new("Remove", MenuAction::RemoveSkillSource(source.id.clone())).danger()]
    })
}

/// The menu of a connection row in Settings > Connectors: Reconnect, Copy
/// handle and Disconnect.
pub fn connection_menu(connection_id: &str, handle: &str) -> Vec<MenuItem> {
    let id = connection_id.to_owned();
    vec![
        MenuItem::new("Reconnect", MenuAction::ReconnectConnection(id.clone())),
        MenuItem::new("Activity", MenuAction::ConnectionActivity(id.clone())),
        MenuItem::new("Copy handle", MenuAction::Copy(handle.to_owned())),
        MenuItem::new("Disconnect", MenuAction::Disconnect(id))
            .danger()
            .separated(),
    ]
}

/// The menu of a skill row in Settings > Skills: Turn off (Turn on when it
/// is off), View files and Remove.
pub fn skill_menu(skill_id: &str, enabled: bool) -> Vec<MenuItem> {
    let id = skill_id.to_owned();
    vec![
        MenuItem::new(
            if enabled { "Turn off" } else { "Turn on" },
            MenuAction::SetSkillEnabled(id.clone(), !enabled),
        ),
        MenuItem::new("View files", MenuAction::SkillFiles(id.clone())),
        MenuItem::new("Remove", MenuAction::RemoveSkill(id))
            .danger()
            .separated(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_box_has_the_same_menu_in_the_same_order() {
        for target in [TextTarget::Composer, TextTarget::Reply] {
            let items = text_edit_menu(target);
            let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
            assert_eq!(labels, ["Cut", "Copy", "Paste", "Select all"]);
            assert!(
                items
                    .iter()
                    .all(|i| matches!(i.action, MenuAction::TextEdit(t, _) if t == target))
            );
            assert!(items[3].separator_before);
        }
    }

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_ref()).collect()
    }

    /// AGENTS.md: a worker row has Rename, Edit tags, Pause and Remove.
    /// The Settings list rows and the "more" menu lacked some of them.
    #[test]
    fn every_worker_menu_has_the_same_items_in_the_same_order() {
        let running = worker_menu("wrk_1", false);
        assert_eq!(labels(&running), ["Rename", "Edit tags", "Pause", "Remove"]);
        let paused = worker_menu("wrk_1", true);
        assert_eq!(labels(&paused), ["Rename", "Edit tags", "Resume", "Remove"]);
        // Pause pauses a running worker, and Resume resumes a paused one.
        assert_eq!(
            running[2].action,
            MenuAction::PauseWorker("wrk_1".into(), true)
        );
        assert_eq!(
            paused[2].action,
            MenuAction::PauseWorker("wrk_1".into(), false)
        );
        assert!(running[3].danger && running[3].separator_before);
        assert!(running.iter().all(|i| !matches!(&i.action,
            MenuAction::RenameWorker(id) | MenuAction::EditTags(id) | MenuAction::RemoveWorker(id)
            | MenuAction::PauseWorker(id, _) if id != "wrk_1")));
    }

    #[test]
    fn the_channel_menu_keeps_the_names_and_the_order() {
        assert_eq!(
            labels(&channel_menu("chn_1")),
            [
                "Rename",
                "Instructions",
                "Model",
                "Worker tags",
                "Connectors and skills",
                "Change icon",
                "Move to group",
                "Archive",
                "Delete"
            ]
        );
        assert_eq!(
            labels(&archived_channel_menu("chn_1")),
            ["Unarchive", "Delete"]
        );
    }

    #[test]
    fn the_account_menu_keeps_the_names_and_the_order() {
        let signed_in = account_menu("wrk_1", "acc_1", true);
        assert_eq!(
            labels(&signed_in),
            ["Account settings", "Rename", "Sign out", "Remove"]
        );
        assert!(signed_in[3].danger && signed_in[3].separator_before);
        assert_eq!(
            signed_in[2].action,
            MenuAction::SignOutAccount("wrk_1".into(), "acc_1".into())
        );
        assert_eq!(
            labels(&account_menu("wrk_1", "acc_1", false)),
            ["Account settings", "Rename", "Remove"]
        );
    }

    #[test]
    fn the_connection_and_skill_menus_keep_the_names_and_the_order() {
        let connection = connection_menu("con_1", "gmail_work");
        assert_eq!(
            labels(&connection),
            ["Reconnect", "Activity", "Copy handle", "Disconnect"]
        );
        assert_eq!(
            connection[1].action,
            MenuAction::ConnectionActivity("con_1".into())
        );
        assert_eq!(connection[2].action, MenuAction::Copy("gmail_work".into()));
        assert!(connection[3].danger && connection[3].separator_before);
        assert_eq!(
            labels(&skill_menu("skl_1", true)),
            ["Turn off", "View files", "Remove"]
        );
        assert_eq!(labels(&skill_menu("skl_1", false))[0], "Turn on");
    }

    /// AGENTS.md rule 2: the new menus of phase 1 keep their names and
    /// their order.
    #[test]
    fn the_file_menus_keep_the_names_and_the_order() {
        let file = consort_client::ChannelFileView {
            channel_id: "chn_1".into(),
            path: "photos/front.jpg".into(),
            ..Default::default()
        };
        let items = file_menu(&file);
        assert_eq!(
            labels(&items),
            ["Open", "Download", "Rename", "Move", "Delete"]
        );
        assert!(items[4].danger && items[4].separator_before);
        assert_eq!(
            items[3].action,
            MenuAction::MoveFile("chn_1".into(), "photos/front.jpg".into())
        );
        let attachment = consort_client::AttachmentView::default();
        assert_eq!(
            labels(&attachment_menu(&attachment, "chn_1")),
            ["Open", "Save", "Save to channel files"]
        );
        let artifact = consort_client::AttachmentView {
            artifact: Some(consort_client::files::ArtifactTask::default()),
            ..Default::default()
        };
        assert_eq!(
            labels(&attachment_menu(&artifact, "chn_1")),
            ["Open", "Save"]
        );
        let target = consort_client::UploadTarget::Composer {
            channel_id: "chn_1".into(),
        };
        assert_eq!(
            labels(&add_menu(&target)),
            ["Upload from this computer", "Choose from channel files"]
        );
        let failed = consort_client::UploadView {
            status: consort_client::UploadStatus::Failed("x".into()),
            ..Default::default()
        };
        assert_eq!(labels(&upload_menu(&failed)), ["Try again", "Remove"]);
        assert_eq!(
            labels(&upload_menu(&consort_client::UploadView::default())),
            ["Remove"]
        );
    }

    #[test]
    fn the_keyboard_steps_through_the_rows_and_wraps() {
        let mut menu = OpenMenu::at(
            point(Pixels::ZERO, Pixels::ZERO),
            text_edit_menu(TextTarget::Composer),
        );
        menu.step(1);
        assert_eq!(menu.highlighted, Some(0));
        menu.step(-1);
        assert_eq!(menu.highlighted, Some(3));
        menu.step(1);
        assert_eq!(menu.highlighted, Some(0));
    }
}
