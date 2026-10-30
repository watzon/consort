//! The app side of connectors and skills: the state of their dialogs, the
//! connect flow, and the hub cards that wait for the browser or a sign-in.
//! The views are in `ui/connectors.rs`, `ui/skills.rs`,
//! `ui/channel_extensions.rs` and `ui/hub_cards.rs`.

use consort_client::{
    ApprovalChoice, AuthKind, ChannelKind, Command, CommandOutput, ConnectionChanges,
    ConnectionStatusView, ConnectorCallView, CredentialFieldView, HubCardView, NewConnection,
    SecretEnv, ServiceLogo, ServiceView, SkillChanges, SkillFileContentView, SkillFileView,
    SkillInstallInput, SkillPreviewView, TimelineItem, ToolApprovalsView, ToolKindView, UseInView,
    ViewState,
};
use gpui_kit::{
    AppContext as _, Context, Entity, Window,
    component::input::{InputEvent, InputState},
};

use crate::app::{ConsortApp, Dialog, Page, SettingsSection};

/// What a Use in control changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UseInTarget {
    Connection(String),
    Skill(String),
    /// The open Connect dialog.
    ConnectDialog,
    /// The open Add skill dialog.
    AddSkillDialog,
}

/// The tab of the Add skill dialog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddSkillTab {
    #[default]
    Directories,
    Git,
    Upload,
}

/// The open Connect dialog. The name is in `ExtInputs::connect_name`.
#[derive(Default)]
pub struct ConnectForm {
    pub service: ServiceView,
    pub use_in: UseInView,
    pub approvals: ToolApprovalsView,
    /// The connect card that the dialog answers.
    pub approval_id: Option<String>,
    /// The connection that the dialog signs in again.
    pub reconnect: Option<String>,
    /// The connection that the dialog started, once the hub made it.
    pub attempt: Option<String>,
    pub redirect_url: Option<String>,
    /// The credential form of a scheme with no browser step.
    pub fields: Vec<(CredentialFieldView, Entity<InputState>)>,
    /// The dialog opened from the Add connector dialog, so Back goes there.
    pub from_catalog: bool,
}

impl ConnectForm {
    /// The scheme that the dialog signs in with.
    pub fn scheme(&self) -> Option<AuthKind> {
        self.service.preferred_scheme().map(|s| s.kind)
    }

    pub fn uses_browser(&self) -> bool {
        self.scheme().is_none_or(AuthKind::uses_browser)
    }
}

/// The open Add skill dialog.
#[derive(Clone, Debug, Default)]
pub struct AddSkillForm {
    pub tab: AddSkillTab,
    /// The skill that the dialog checked, with its source.
    pub preview: Option<(SkillInstallInput, SkillPreviewView)>,
    /// The entry whose check runs now.
    pub checking: Option<String>,
    pub upload_path: Option<String>,
    pub use_in: UseInView,
}

/// The open Activity dialog of a connection: the audit log, newest first.
#[derive(Clone, Debug, Default)]
pub struct ActivityForm {
    pub connection_id: String,
    pub calls: Vec<ConnectorCallView>,
    /// Older calls exist.
    pub has_more: bool,
    /// The first page loads.
    pub loading: bool,
    /// An older page loads.
    pub loading_more: bool,
    /// What the hub or the link said when a page failed.
    pub error: Option<String>,
    /// Counts the loads, so the answer of an old load is dropped.
    pub serial: u64,
}

/// The open files dialog of a skill.
#[derive(Clone, Debug, Default)]
pub struct SkillFilesForm {
    pub skill_id: String,
    pub files: Vec<SkillFileView>,
    pub open: Option<SkillFileContentView>,
}

/// A sign-in card whose sign-in runs in the sign-in dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignInCardWait {
    pub approval_id: String,
    pub worker_id: String,
    pub account_id: String,
    /// The card names a method, so Allow makes the hub start the sign-in.
    /// The app then opens that sign-in when the card names it. Without a
    /// method the app starts the sign-in, and answers the card when the
    /// account is signed in.
    pub hub_starts: bool,
    /// The sign-in dialog is open on the hub's sign-in.
    pub opened: bool,
}

/// The text inputs of connectors and skills.
pub struct ExtInputs {
    pub catalog_search: Entity<InputState>,
    pub connect_name: Entity<InputState>,
    pub composio_key: Entity<InputState>,
    /// The name field on a connector page.
    pub connection_name: Entity<InputState>,
    pub skill_search: Entity<InputState>,
    pub git_url: Entity<InputState>,
    pub git_path: Entity<InputState>,
    /// The URL field of the Add source dialog.
    pub source_url: Entity<InputState>,
}

impl ExtInputs {
    pub fn new(window: &mut Window, cx: &mut Context<ConsortApp>) -> ExtInputs {
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<ConsortApp>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        ExtInputs {
            catalog_search: field("Search services", window, cx),
            connect_name: field("Gmail Work", window, cx),
            composio_key: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Paste the project key")
                    .masked(true)
            }),
            connection_name: field("Name", window, cx),
            skill_search: field("Search skills", window, cx),
            git_url: field("https://github.com/owner/repo", window, cx),
            git_path: field("Folder in the repo, such as skills/pdf", window, cx),
            source_url: field(
                "https://github.com/owner/repo or a marketplace.json URL",
                window,
                cx,
            ),
        }
    }

    pub fn all(&self) -> [&Entity<InputState>; 8] {
        [
            &self.catalog_search,
            &self.connect_name,
            &self.composio_key,
            &self.connection_name,
            &self.skill_search,
            &self.git_url,
            &self.git_path,
            &self.source_url,
        ]
    }
}

/// Everything the app keeps about connectors and skills between draws.
#[derive(Default)]
pub struct ExtState {
    /// The connection open in Settings > Connectors.
    pub connection: Option<String>,
    /// The skill open in Settings > Skills.
    pub skill: Option<String>,
    pub connect: ConnectForm,
    pub add_skill: AddSkillForm,
    pub files: SkillFilesForm,
    pub activity: ActivityForm,
    /// Connect cards that the user continued: the app opens the sign-in page
    /// once the hub has made the connection.
    pub browser_cards: Vec<String>,
    /// The connections whose sign-in page the app opened.
    pub opened: Vec<String>,
    pub sign_in_card: Option<SignInCardWait>,
    /// The provider extras list is open in the account dialog.
    pub extras_list_open: bool,
    /// Counts catalog and directory searches typed, so a search runs only
    /// after the typing stops.
    pub search_serial: u64,
    /// The service grid of the Add connector dialog.
    pub catalog_scroll: gpui_kit::ScrollHandle,
    /// The result list of the Add skill dialog.
    pub results_scroll: gpui_kit::ScrollHandle,
}

/// Every tag that a Use in control offers: the tags of the channels, then
/// the worker tags, each once, sorted within each part.
pub fn scope_tags(state: &ViewState) -> Vec<String> {
    let mut channel: Vec<String> = state.channels.iter().flat_map(|c| c.tags.clone()).collect();
    channel.sort();
    channel.dedup();
    let mut rest: Vec<String> = state
        .tags
        .iter()
        .filter(|t| !channel.contains(t))
        .cloned()
        .collect();
    rest.sort();
    channel.extend(rest);
    channel
}

/// A Use in with one tag added or taken away. No tag left means every
/// channel.
pub fn toggle_tag(use_in: &UseInView, tag: &str) -> UseInView {
    let mut tags = match use_in {
        UseInView::Tags(tags) => tags.clone(),
        _ => Vec::new(),
    };
    if let Some(at) = tags.iter().position(|t| t == tag) {
        tags.remove(at);
    } else {
        tags.push(tag.to_owned());
    }
    if tags.is_empty() {
        UseInView::EveryChannel
    } else {
        UseInView::Tags(tags)
    }
}

/// The Use in that a new connection or skill starts with: the tags of the
/// open channel, or every channel from Tutti or a channel with no tags.
pub fn default_use_in(state: &ViewState, channel_id: &str) -> UseInView {
    match state.channel(channel_id) {
        Some(c) if c.kind != ChannelKind::Tutti && !c.tags.is_empty() => {
            UseInView::Tags(c.tags.clone())
        }
        _ => UseInView::EveryChannel,
    }
}

/// A service as far as a card or a connection knows it, for the Connect
/// dialog when the catalog has not loaded it.
pub fn service_from(slug: &str, name: &str, logo: &ServiceLogo, state: &ViewState) -> ServiceView {
    state
        .connector_catalog
        .services
        .iter()
        .find(|s| s.slug == slug)
        .cloned()
        .unwrap_or_else(|| ServiceView {
            slug: slug.to_owned(),
            name: name.to_owned(),
            logo: logo.clone(),
            ..ServiceView::default()
        })
}

/// The hub card with this approval ID, wherever it shows.
pub fn find_card(state: &ViewState, approval_id: &str) -> Option<HubCardView> {
    state.timelines.iter().find_map(|t| {
        t.items.iter().find_map(|i| match i {
            TimelineItem::Approval(card) if card.approval.id == approval_id => {
                card.approval.card.as_deref().cloned()
            }
            TimelineItem::TaskCard(card) => card
                .approval
                .as_ref()
                .filter(|a| a.id == approval_id)
                .and_then(|a| a.card.as_deref().cloned()),
            _ => None,
        })
    })
}

impl ConsortApp {
    /// Subscribes to the inputs of connectors and skills.
    pub fn subscribe_ext_inputs(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui_kit::Subscription> {
        let mut subs = Vec::new();
        for field in [
            &self.ext_inputs.connect_name,
            &self.ext_inputs.composio_key,
            &self.ext_inputs.git_url,
            &self.ext_inputs.git_path,
            &self.ext_inputs.source_url,
        ] {
            subs.push(
                cx.subscribe_in(
                    field,
                    window,
                    |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => this.submit_ext_enter(window, cx),
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    },
                ),
            );
        }
        subs.push(cx.subscribe_in(
            &self.ext_inputs.catalog_search,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.search_soon(false, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.ext_inputs.skill_search,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.search_soon(true, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.ext_inputs.connection_name,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.save_connection_name(cx),
                _ => {}
            },
        ));
        subs
    }

    /// Enter in a field of a connector or skill dialog or page.
    fn submit_ext_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.dialog {
            Some(Dialog::Connect) => self.submit_connect(window, cx),
            Some(Dialog::ComposioKey) => self.save_composio_key(window, cx),
            Some(Dialog::AddSkill) if self.ext.add_skill.tab == AddSkillTab::Git => {
                self.check_git_skill(cx)
            }
            Some(Dialog::AddSkillSource) => self.add_skill_source(window, cx),
            None if self.page == Page::Settings
                && self.settings_section == SettingsSection::Connectors =>
            {
                self.save_composio_key(window, cx)
            }
            _ => {}
        }
    }

    /// Runs the catalog or directory search a moment after the typing
    /// stops.
    fn search_soon(&mut self, skills: bool, cx: &mut Context<Self>) {
        self.ext.search_serial += 1;
        let serial = self.ext.search_serial;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.ext.search_serial == serial {
                    if skills {
                        this.search_skills(cx);
                    } else {
                        this.search_catalog(cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Searches the catalog with the text of the search field and the
    /// chosen category.
    pub fn search_catalog(&mut self, cx: &mut Context<Self>) {
        let search = self.ext_inputs.catalog_search.read(cx).value().to_string();
        let category = self.state.connector_catalog.category.clone();
        self.search_catalog_in(search, category, cx);
    }

    pub fn search_catalog_in(
        &mut self,
        search: String,
        category: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .run(Command::SearchConnectorCatalog { search, category })
                .await
        });
    }

    pub fn search_skills(&mut self, cx: &mut Context<Self>) {
        let search = self.ext_inputs.skill_search.read(cx).value().to_string();
        let source = self.state.skill_directory.source.clone();
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .run(Command::SearchSkillDirectory { search, source })
                .await
        });
    }

    /// Settings > Connectors opened: the Composio status, a selection and
    /// the tools of the selected service.
    pub fn enter_connectors(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(
            cx,
            async move { client.run(Command::LoadComposioStatus).await },
        );
        if self
            .ext
            .connection
            .as_ref()
            .is_none_or(|id| self.state.connection(id).is_none())
        {
            self.ext.connection = self.state.connections.first().map(|c| c.id.clone());
        }
        self.select_connection(self.ext.connection.clone(), None, cx);
    }

    pub fn enter_skills(&mut self, cx: &mut Context<Self>) {
        // The sources section lists what the hub searches now.
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(Command::LoadSkillDirectorySources).await
        });
        if self
            .ext
            .skill
            .as_ref()
            .is_none_or(|id| self.state.skill(id).is_none())
        {
            self.ext.skill = self.state.skills.first().map(|s| s.id.clone());
        }
    }

    /// Opens a connection in Settings > Connectors: its name goes in the
    /// name field, and its tools load.
    pub fn select_connection(
        &mut self,
        id: Option<String>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.ext.connection = id.clone();
        let Some(connection) = id.and_then(|id| self.state.connection(&id).cloned()) else {
            cx.notify();
            return;
        };
        if let Some(window) = window {
            self.ext_inputs.connection_name.update(cx, |input, cx| {
                input.set_value(connection.name.clone(), window, cx)
            });
        } else {
            let name = connection.name.clone();
            self.pending_connection_name = Some(name);
        }
        if self.state.tools_of(&connection.service).is_none() {
            let client = self.client.clone();
            let service = connection.service.clone();
            self.spawn_command(cx, async move {
                client.run(Command::LoadServiceTools { service }).await
            });
        }
        cx.notify();
    }

    /// Saves the name field of the open connection when it changed.
    fn save_connection_name(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self
            .ext
            .connection
            .as_ref()
            .and_then(|id| self.state.connection(id))
            .cloned()
        else {
            return;
        };
        let name = self
            .ext_inputs
            .connection_name
            .read(cx)
            .value()
            .trim()
            .to_owned();
        if name.is_empty() || name == connection.name {
            return;
        }
        self.update_connection(
            connection.id,
            ConnectionChanges {
                name: Some(name),
                ..ConnectionChanges::default()
            },
            cx,
        );
    }

    pub fn update_connection(
        &mut self,
        connection_id: String,
        changes: ConnectionChanges,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .run(Command::UpdateConnection {
                    connection_id,
                    changes,
                })
                .await
        });
    }

    pub fn update_skill(
        &mut self,
        skill_id: String,
        changes: SkillChanges,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(Command::UpdateSkill { skill_id, changes }).await
        });
    }

    /// Changes a Use in.
    pub fn set_use_in(&mut self, target: UseInTarget, use_in: UseInView, cx: &mut Context<Self>) {
        match target {
            UseInTarget::Connection(id) => self.update_connection(
                id,
                ConnectionChanges {
                    use_in: Some(use_in),
                    ..ConnectionChanges::default()
                },
                cx,
            ),
            UseInTarget::Skill(id) => self.update_skill(
                id,
                SkillChanges {
                    use_in: Some(use_in),
                    ..SkillChanges::default()
                },
                cx,
            ),
            UseInTarget::ConnectDialog => self.ext.connect.use_in = use_in,
            UseInTarget::AddSkillDialog => self.ext.add_skill.use_in = use_in,
        }
        cx.notify();
    }

    /// The Use in that a target has now.
    pub fn use_in_of(&self, target: &UseInTarget) -> UseInView {
        match target {
            UseInTarget::Connection(id) => self
                .state
                .connection(id)
                .map(|c| c.use_in.clone())
                .unwrap_or_default(),
            UseInTarget::Skill(id) => self
                .state
                .skill(id)
                .map(|s| s.use_in.clone())
                .unwrap_or_default(),
            UseInTarget::ConnectDialog => self.ext.connect.use_in.clone(),
            UseInTarget::AddSkillDialog => self.ext.add_skill.use_in.clone(),
        }
    }

    /// Opens the Connect dialog for a service. `card` is the connect card
    /// that the dialog answers, with its settings.
    pub fn open_connect(
        &mut self,
        service: ServiceView,
        card: Option<(String, String, UseInView, ToolApprovalsView)>,
        from_catalog: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = consort_client::connections_of_service(&self.state, &service.slug);
        let (approval_id, name, use_in, approvals) = match card {
            Some((id, name, use_in, approvals)) => (Some(id), name, use_in, approvals),
            None => (
                None,
                consort_client::suggested_connection_name(&service.name, &existing),
                default_use_in(&self.state, &self.channel_id),
                ToolApprovalsView::default(),
            ),
        };
        let fields = self.credential_inputs(&service, window, cx);
        self.ext.connect = ConnectForm {
            service,
            use_in,
            approvals,
            approval_id,
            fields,
            from_catalog,
            ..ConnectForm::default()
        };
        self.open_dialog(Dialog::Connect, window, cx);
        self.ext_inputs.connect_name.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.focus(window, cx);
        });
    }

    /// The inputs of the credential form of a service whose scheme has no
    /// browser step. Secret fields are masked.
    fn credential_inputs(
        &self,
        service: &ServiceView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(CredentialFieldView, Entity<InputState>)> {
        let Some(scheme) = service.preferred_scheme() else {
            return Vec::new();
        };
        if scheme.kind.uses_browser() {
            return Vec::new();
        }
        let style = crate::ui::parts::editor_style(&Self::tokens(cx));
        scheme
            .fields
            .iter()
            .map(|field| {
                let placeholder = field
                    .description
                    .clone()
                    .unwrap_or_else(|| field.label.clone());
                let secret = field.secret;
                let style = style.clone();
                let input = cx.new(|cx| {
                    let mut state = InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(secret);
                    state.set_editor_style(style);
                    state
                });
                (field.clone(), input)
            })
            .collect()
    }

    /// Reconnect on a connection: a browser scheme opens its sign-in page at
    /// once. A scheme with a form opens the Connect dialog for the form.
    pub fn reconnect(
        &mut self,
        connection_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.state.connection(&connection_id).cloned() else {
            return;
        };
        let service = service_from(
            &connection.service,
            &connection.service_name,
            &connection.logo,
            &self.state,
        );
        let needs_form = service
            .preferred_scheme()
            .is_some_and(|s| !s.kind.uses_browser());
        if needs_form {
            let fields = self.credential_inputs(&service, window, cx);
            self.ext.connect = ConnectForm {
                service,
                use_in: connection.use_in.clone(),
                approvals: connection.approvals,
                reconnect: Some(connection_id),
                fields,
                ..ConnectForm::default()
            };
            self.open_dialog(Dialog::Connect, window, cx);
            let name = connection.name.clone();
            self.ext_inputs
                .connect_name
                .update(cx, |input, cx| input.set_value(name, window, cx));
            return;
        }
        let client = self.client.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = client
                .run_for(Command::ReconnectConnection {
                    connection_id,
                    credentials: SecretEnv::new(),
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(CommandOutput::ConnectStarted {
                    connection_id,
                    redirect_url: Some(url),
                }) => this.open_sign_in_page(&connection_id, &url, cx),
                Ok(_) => {}
                Err(err) => this.show_error(err.to_string(), cx),
            });
        })
        .detach();
    }

    /// Continue in browser (or Connect) in the Connect dialog.
    pub fn submit_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = &self.ext.connect;
        if form.attempt.is_some() {
            // The sign-in runs. The button opens the page again.
            if let (Some(id), Some(url)) = (form.attempt.clone(), form.redirect_url.clone()) {
                self.ext.opened.retain(|o| o != &id);
                self.open_sign_in_page(&id, &url, cx);
            }
            return;
        }
        let name = self
            .ext_inputs
            .connect_name
            .read(cx)
            .value()
            .trim()
            .to_owned();
        if name.is_empty() && form.reconnect.is_none() {
            self.dialog_error = Some("Enter a name for the account, such as Gmail Work.".into());
            cx.notify();
            return;
        }
        let mut credentials = SecretEnv::new();
        for (field, input) in &form.fields {
            let value = input.read(cx).value().trim().to_owned();
            if field.required && value.is_empty() {
                self.dialog_error = Some(format!("Enter the {}.", field.label.to_lowercase()));
                cx.notify();
                return;
            }
            if !value.is_empty() {
                credentials.insert(field.name.clone(), value);
            }
        }
        let command = match &form.reconnect {
            Some(connection_id) => Command::ReconnectConnection {
                connection_id: connection_id.clone(),
                credentials,
            },
            None => Command::Connect(NewConnection {
                service: form.service.slug.clone(),
                name,
                use_in: form.use_in.clone(),
                approvals: form.approvals,
                auth_scheme: form.scheme(),
                credentials,
                approval_id: form.approval_id.clone(),
            }),
        };
        if !self.in_flight.begin("connect") {
            return;
        }
        self.dialog_error = None;
        let client = self.client.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = client.run_for(command).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.in_flight.finish("connect");
                match result {
                    Ok(CommandOutput::ConnectStarted {
                        connection_id,
                        redirect_url,
                    }) => {
                        this.ext.connect.attempt = Some(connection_id.clone());
                        this.ext.connect.redirect_url = redirect_url.clone();
                        if let Some(url) = redirect_url {
                            this.open_sign_in_page(&connection_id, &url, cx);
                        }
                        this.follow_connect_dialog(window, cx);
                    }
                    Ok(_) => {}
                    Err(err) => this.dialog_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the sign-in page of a connection in the system browser, once.
    /// The mock has no real page, so it says so instead.
    pub fn open_sign_in_page(&mut self, connection_id: &str, url: &str, cx: &mut Context<Self>) {
        if self.ext.opened.iter().any(|o| o == connection_id) {
            return;
        }
        self.ext.opened.push(connection_id.to_owned());
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            self.show_error(
                "The hub sent a sign-in address that is not a web page. Try again, or check the hub."
                    .into(),
                cx,
            );
            return;
        }
        if self.demo {
            self.show_notice(
                "This is a demo, so the sign-in page does not open. The connector turns active in a moment.".into(),
                cx,
            );
        } else {
            cx.open_url(url);
        }
    }

    /// The Connect dialog follows its connection: active closes it, a
    /// failure shows in it.
    fn follow_connect_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog != Some(Dialog::Connect) {
            return;
        }
        let Some(id) = self.ext.connect.attempt.clone() else {
            return;
        };
        let Some(connection) = self.state.connection(&id).cloned() else {
            return;
        };
        match &connection.status {
            ConnectionStatusView::Active => {
                self.close_dialog(window, cx);
                self.show_notice(format!("{} is connected.", connection.name), cx);
                if self.page == Page::Settings {
                    self.settings_section = SettingsSection::Connectors;
                    self.select_connection(Some(id), Some(window), cx);
                }
            }
            ConnectionStatusView::NeedsSignIn { message } => {
                self.dialog_error = Some(match message {
                    Some(m) => format!("{m} Try again."),
                    None => "The sign-in did not finish. Try again.".into(),
                });
                self.ext.connect.attempt = None;
            }
            ConnectionStatusView::Failed { message } => {
                self.dialog_error = Some(message.clone());
                self.ext.connect.attempt = None;
            }
            ConnectionStatusView::Connecting { .. } => {}
        }
    }

    /// After each new state: the Connect dialog, the connect cards that
    /// wait for their sign-in page, and the sign-in card that waits for its
    /// account.
    pub fn follow_extensions(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        if let Some(window) = window.as_deref_mut() {
            self.follow_connect_dialog(window, cx);
            if let Some(name) = self.pending_connection_name.take() {
                self.ext_inputs
                    .connection_name
                    .update(cx, |input, cx| input.set_value(name, window, cx));
            }
        }
        let window_for_sign_in = window;
        let waiting = std::mem::take(&mut self.ext.browser_cards);
        for approval_id in waiting {
            let url = match find_card(&self.state, &approval_id) {
                Some(HubCardView::ConnectService(card)) => card.connection_id.and_then(|id| {
                    match self.state.connection(&id).map(|c| &c.status) {
                        Some(ConnectionStatusView::Connecting {
                            redirect_url: Some(url),
                            ..
                        }) => Some((id, url.clone())),
                        _ => None,
                    }
                }),
                // The card is gone: stop waiting.
                None => continue,
                _ => None,
            };
            match url {
                Some((id, url)) => self.open_sign_in_page(&id, &url, cx),
                None => self.ext.browser_cards.push(approval_id),
            }
        }
        if let Some(wait) = self.ext.sign_in_card.clone()
            && wait.hub_starts
        {
            let started = match find_card(&self.state, &wait.approval_id) {
                Some(HubCardView::SignIn(card)) => card.sign_in_id,
                _ => None,
            };
            match (started, window_for_sign_in) {
                (Some(sign_in_id), Some(window)) if !wait.opened => {
                    if let Some(w) = self.ext.sign_in_card.as_mut() {
                        w.opened = true;
                    }
                    self.open_running_sign_in(
                        wait.worker_id.clone(),
                        wait.account_id.clone(),
                        sign_in_id,
                        window,
                        cx,
                    );
                }
                _ => {}
            }
            let signed_in = self
                .state
                .worker(&wait.worker_id)
                .and_then(|w| w.account(&wait.account_id))
                .is_some_and(|a| a.status.works());
            if signed_in && wait.opened {
                self.ext.sign_in_card = None;
            }
        } else if let Some(wait) = self.ext.sign_in_card.clone() {
            let signed_in = self
                .state
                .worker(&wait.worker_id)
                .and_then(|w| w.account(&wait.account_id))
                .is_some_and(|a| a.status.works());
            if signed_in {
                self.ext.sign_in_card = None;
                let client = self.client.clone();
                self.spawn_command(cx, async move {
                    client
                        .answer_approval(wait.approval_id, ApprovalChoice::AllowOnce)
                        .await
                });
            }
        }
    }

    /// Continue in browser on a connect card: the hub connects with the
    /// card's settings, and the app opens the page when it has it.
    pub fn continue_connect_card(&mut self, approval_id: String, cx: &mut Context<Self>) {
        if !self.ext.browser_cards.contains(&approval_id) {
            self.ext.browser_cards.push(approval_id.clone());
        }
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .answer_approval(approval_id, ApprovalChoice::AllowOnce)
                .await
        });
    }

    /// Change settings on a connect card: the Connect dialog with the card's
    /// settings. Its connect answers the card.
    pub fn change_connect_card(
        &mut self,
        approval_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(HubCardView::ConnectService(card)) = find_card(&self.state, &approval_id) else {
            return;
        };
        let service = service_from(&card.service, &card.service_name, &card.logo, &self.state);
        self.open_connect(
            service,
            Some((approval_id, card.name, card.use_in, card.approvals)),
            false,
            window,
            cx,
        );
    }

    /// Sign in on a sign-in card. With a method on the card, Allow makes
    /// the hub start the sign-in (DECISIONS in docs/notes/step2-hub.md), and
    /// the sign-in dialog opens on it. Without one, the sign-in dialog lets
    /// the user choose, and the card is answered when the account signs in.
    pub fn sign_in_from_card(
        &mut self,
        approval_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(HubCardView::SignIn(card)) = find_card(&self.state, &approval_id) else {
            return;
        };
        if self.state.worker(&card.worker_id).is_none() {
            self.show_error(
                "That worker is not paired with this hub any more. Answer Not now, then ask the assistant again.".into(),
                cx,
            );
            return;
        }
        let hub_starts = card.method.is_some();
        self.ext.sign_in_card = Some(SignInCardWait {
            approval_id: approval_id.clone(),
            worker_id: card.worker_id.clone(),
            account_id: card.account_id.clone(),
            hub_starts,
            opened: false,
        });
        if hub_starts {
            // The hub starts the sign-in with the card's method. The app
            // opens the sign-in dialog on it when the card names it.
            self.answer_approval(approval_id, ApprovalChoice::AllowOnce, window, cx);
        } else {
            self.open_sign_in(card.worker_id, card.account_id, window, cx);
        }
    }

    /// Saves the Composio key from the key field.
    pub fn save_composio_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self
            .ext_inputs
            .composio_key
            .read(cx)
            .value()
            .trim()
            .to_owned();
        if key.is_empty() {
            self.dialog_error = Some("Paste your Composio project key.".into());
            cx.notify();
            return;
        }
        let client = self.client.clone();
        let in_dialog = self.dialog == Some(Dialog::ComposioKey);
        let input = self.ext_inputs.composio_key.clone();
        if in_dialog {
            self.submit_dialog(
                async move {
                    client
                        .run(Command::SetComposioKey {
                            api_key: consort_client::SecretText(key),
                        })
                        .await
                },
                window,
                cx,
            );
            input.update(cx, |input, cx| input.set_value("", window, cx));
            return;
        }
        if !self.in_flight.begin("composio-key") {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let result = client
                .run(Command::SetComposioKey {
                    api_key: consort_client::SecretText(key),
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.in_flight.finish("composio-key");
                match result {
                    Ok(()) => {
                        this.key_error = None;
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    Err(err) => this.key_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Adds the skill source from the URL field of the Add source dialog.
    pub fn add_skill_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self
            .ext_inputs
            .source_url
            .read(cx)
            .value()
            .trim()
            .to_owned();
        if url.is_empty() {
            self.dialog_error =
                Some("Paste the URL of a git repo or a marketplace.json file.".into());
            cx.notify();
            return;
        }
        let client = self.client.clone();
        self.submit_dialog(
            async move { client.run(Command::AddSkillDirectorySource { url }).await },
            window,
            cx,
        );
    }

    /// Removes a skill source that the user added. Skills that came from it
    /// stay installed.
    pub fn remove_skill_source(&mut self, id: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(Command::RemoveSkillDirectorySource { id }).await
        });
    }

    /// Loads a page of the activity of a connection. `more` loads the page
    /// after the calls that show now. Otherwise the list starts again.
    pub fn load_activity(&mut self, connection_id: String, more: bool, cx: &mut Context<Self>) {
        let activity = &mut self.ext.activity;
        let before = if more {
            if activity.loading_more || activity.connection_id != connection_id {
                return;
            }
            activity.loading_more = true;
            activity.calls.last().map(|c| c.id.clone())
        } else {
            let serial = activity.serial;
            *activity = ActivityForm {
                connection_id: connection_id.clone(),
                loading: true,
                serial,
                ..ActivityForm::default()
            };
            None
        };
        activity.error = None;
        activity.serial += 1;
        let serial = activity.serial;
        cx.notify();
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .run_for(Command::LoadConnectionCalls {
                    connection_id: connection_id.clone(),
                    before,
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let activity = &mut this.ext.activity;
                // The user may have closed the dialog or asked again.
                if activity.serial != serial {
                    return;
                }
                activity.loading = false;
                activity.loading_more = false;
                match result {
                    Ok(CommandOutput::ConnectionCalls(page)) => {
                        activity.calls.extend(page.calls);
                        activity.has_more = page.has_more;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        let reason = err.to_string();
                        activity.error = Some(format!(
                            "Consort could not load the activity: {}. Try again.",
                            reason.trim_end_matches('.')
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Checks the skill of the Git URL tab.
    pub fn check_git_skill(&mut self, cx: &mut Context<Self>) {
        let url = self.ext_inputs.git_url.read(cx).value().trim().to_owned();
        let path = self.ext_inputs.git_path.read(cx).value().trim().to_owned();
        self.preview_skill(
            SkillInstallInput::Git {
                url,
                path: Some(path).filter(|p| !p.is_empty()),
                git_ref: None,
            },
            "git".into(),
            cx,
        );
    }

    /// Asks the hub what an install would bring. The answer shows in the
    /// dialog, and Add skill installs it.
    pub fn preview_skill(
        &mut self,
        source: SkillInstallInput,
        key: String,
        cx: &mut Context<Self>,
    ) {
        if self.ext.add_skill.checking.is_some() {
            return;
        }
        self.ext.add_skill.checking = Some(key);
        self.dialog_error = None;
        let client = self.client.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client
                .run_for(Command::PreviewSkill {
                    source: source.clone(),
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.ext.add_skill.checking = None;
                match result {
                    Ok(CommandOutput::SkillPreview(preview)) => {
                        this.ext.add_skill.preview = Some((source, preview));
                    }
                    Ok(_) => {}
                    Err(err) => this.dialog_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Installs the checked skill.
    pub fn install_previewed_skill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((source, preview)) = self.ext.add_skill.preview.clone() else {
            return;
        };
        let use_in = self.ext.add_skill.use_in.clone();
        let client = self.client.clone();
        self.submit_dialog(
            async move {
                client
                    .run_for(Command::InstallSkill {
                        source,
                        use_in,
                        enabled: None,
                        approval_id: None,
                    })
                    .await
                    .map(|_| ())
            },
            window,
            cx,
        );
        let _ = preview;
    }

    /// Opens the files dialog of a skill and loads its file list.
    pub fn open_skill_files(
        &mut self,
        skill_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ext.files = SkillFilesForm {
            skill_id: skill_id.clone(),
            ..SkillFilesForm::default()
        };
        self.open_dialog(Dialog::SkillFiles(skill_id.clone()), window, cx);
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .run_for(Command::LoadSkillFiles {
                    skill_id: skill_id.clone(),
                    pending: false,
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(CommandOutput::SkillFiles(files)) => {
                        let first = files
                            .iter()
                            .find(|f| f.path == consort_proto_manifest())
                            .or(files.first())
                            .map(|f| f.path.clone());
                        this.ext.files.files = files;
                        if let Some(path) = first {
                            this.read_skill_file(path, cx);
                        }
                    }
                    Ok(_) => {}
                    Err(err) => this.dialog_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn read_skill_file(&mut self, path: String, cx: &mut Context<Self>) {
        let skill_id = self.ext.files.skill_id.clone();
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .run_for(Command::ReadSkillFile {
                    skill_id,
                    path,
                    pending: false,
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(CommandOutput::SkillFile(file)) => this.ext.files.open = Some(file),
                    Ok(_) => {}
                    Err(err) => this.dialog_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Turns a connection or a skill on or off in a channel.
    pub fn set_channel_item(
        &mut self,
        channel_id: String,
        id: String,
        connector: bool,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        let command = if connector {
            Command::SetChannelConnectionEnabled {
                channel_id,
                connection_id: id,
                enabled,
            }
        } else {
            Command::SetChannelSkillEnabled {
                channel_id,
                skill_id: id,
                enabled,
            }
        };
        self.spawn_command(cx, async move { client.run(command).await });
    }

    /// The Change button of an item that the channel's scope does not give:
    /// its page in Settings, where its Use in is.
    pub fn show_item_scope(
        &mut self,
        id: String,
        connector: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_dialog(window, cx);
        if connector {
            self.open_settings(SettingsSection::Connectors, cx);
            self.select_connection(Some(id), Some(window), cx);
        } else {
            self.open_settings(SettingsSection::Skills, cx);
            self.ext.skill = Some(id);
        }
        cx.notify();
    }

    /// The tools of a kind for the approval row.
    pub fn tool_count(&self, service: &str, kind: ToolKindView) -> Option<usize> {
        self.state
            .tools_of(service)
            .filter(|t| t.load == consort_client::LoadState::Loaded)
            .map(|t| t.count(kind))
    }
}

/// The manifest file of a skill, which the files dialog opens first.
fn consort_proto_manifest() -> &'static str {
    "SKILL.md"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_toggles_and_no_tag_means_every_channel() {
        let work = toggle_tag(&UseInView::EveryChannel, "work");
        assert_eq!(work, UseInView::Tags(vec!["work".into()]));
        let both = toggle_tag(&work, "gpu");
        assert_eq!(both, UseInView::Tags(vec!["work".into(), "gpu".into()]));
        assert_eq!(
            toggle_tag(&toggle_tag(&both, "gpu"), "work"),
            UseInView::EveryChannel
        );
    }

    #[test]
    fn a_new_item_starts_with_the_tags_of_the_open_channel() {
        let state = consort_client::mock::view_state();
        assert_eq!(
            default_use_in(&state, consort_client::mock_extensions::ACME),
            UseInView::Tags(vec!["work".into()])
        );
        assert_eq!(
            default_use_in(&state, consort_client::mock::tutti_id()),
            UseInView::EveryChannel
        );
        let tags = scope_tags(&state);
        assert!(tags.contains(&"work".to_owned()));
        assert_eq!(tags.iter().filter(|t| *t == "personal").count(), 1);
    }
}
