//! Connections: connect, update, disconnect, reconnect, the audit log, the
//! channel exceptions, and the task that follows a browser sign-in.

use consort_proto::client_api::{
    CONNECTOR_CALLS_PAGE_MAX, ChannelResult, ChannelSetConnectionEnabled, ChannelSetSkillEnabled,
    ConnectAttempt, ConnectionCalls, ConnectionCallsResult, ConnectionConnect,
    ConnectionDisconnect, ConnectionReconnect, ConnectionResult, ConnectionUpdate,
};
use consort_proto::frame::Empty;
use consort_proto::ids::{ApprovalId, ConnectionId};
use consort_proto::model::{
    ApprovalDetails, ApprovalStatus, AuthSchemeKind, Connection, ConnectionStatus, SecretEnv,
    ToolApprovals, UseIn, unique_connection_handle, validate_connection_name,
};
use consort_proto::time::Timestamp;

use super::composio::{self, AccountState, Client};
use super::{scheme_kind, scheme_wire};
use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::{connectors as con_store, conversation, skills as skill_store, work};

/// The default page of `connection.calls`.
const CALLS_PAGE_DEFAULT: u32 = 50;

/// What a new connection asks for.
#[derive(Debug, Clone)]
pub(crate) struct ConnectSpec {
    pub service: String,
    pub name: String,
    pub use_in: UseIn,
    pub approvals: ToolApprovals,
    pub auth_scheme: Option<AuthSchemeKind>,
    pub credentials: SecretEnv,
}

/// The Composio side of a new connected account.
struct Linked {
    auth_config_id: String,
    account_id: String,
    redirect_url: Option<String>,
    /// The account, for a scheme with no browser step.
    account: Option<composio::ConnectedAccount>,
}

impl Hub {
    /// Checks a "Use in" scope: its shape, and that its channels exist.
    pub(crate) async fn check_use_in(&self, use_in: UseIn) -> Result<UseIn> {
        let use_in = use_in.normalized()?;
        if let UseIn::Channels { channel_ids } = &use_in {
            let ids = channel_ids.clone();
            self.store
                .read(move |conn| {
                    for id in ids {
                        conversation::channel_or_404(conn, id)?;
                    }
                    Ok(())
                })
                .await?;
        }
        Ok(use_in)
    }

    /// Finds or makes the auth config for a new connected account. A
    /// Composio-managed config comes first, then one that the user made in
    /// Composio, then a new one.
    async fn auth_config_for(
        &self,
        client: &Client,
        service: &str,
        scheme: AuthSchemeKind,
        managed_scheme: bool,
    ) -> Result<String> {
        let configs = client.auth_configs(service).await?;
        let same = |c: &&composio::AuthConfig| {
            c.is_enabled() && c.auth_scheme.as_deref().map(scheme_kind) == Some(scheme)
        };
        if let Some(found) = configs.iter().filter(same).find(|c| c.is_composio_managed) {
            return Ok(found.id.clone());
        }
        if managed_scheme && scheme.uses_browser() {
            return client.create_auth_config(service, None).await;
        }
        if let Some(found) = configs.iter().find(same) {
            return Ok(found.id.clone());
        }
        if scheme.uses_browser() {
            return Err(HubError::invalid(format!(
                "Composio has no managed sign-in for {service}. Make an auth config for it in the Composio dashboard, then connect again."
            )));
        }
        let wire = scheme_wire(scheme).ok_or_else(|| {
            HubError::invalid("Consort cannot connect this kind of sign-in yet. Choose another.")
        })?;
        client.create_auth_config(service, Some(wire)).await
    }

    /// Asks Composio for a new connected account: a sign-in link, or an
    /// account made with the credentials. The credentials go to Composio
    /// once and the hub keeps no copy.
    async fn link_account(
        &self,
        client: &Client,
        service: &str,
        scheme: AuthSchemeKind,
        managed_scheme: bool,
        alias: &str,
        credentials: &SecretEnv,
    ) -> Result<Linked> {
        let user_id = self.composio_user_id().await?;
        let auth_config_id = self
            .auth_config_for(client, service, scheme, managed_scheme)
            .await?;
        if scheme.uses_browser() {
            let link = client.link(&auth_config_id, &user_id, alias).await?;
            return Ok(Linked {
                auth_config_id,
                account_id: link.connected_account_id,
                redirect_url: Some(link.redirect_url),
                account: None,
            });
        }
        let wire = scheme_wire(scheme).ok_or_else(|| {
            HubError::invalid("Consort cannot connect this kind of sign-in yet. Choose another.")
        })?;
        let account = client
            .create_connected_account(&auth_config_id, &user_id, alias, wire, credentials)
            .await?;
        Ok(Linked {
            auth_config_id,
            account_id: account.id.clone(),
            redirect_url: None,
            account: Some(account),
        })
    }

    /// Picks the auth scheme and checks the credentials against the fields
    /// that Composio asks for. Returns the scheme and whether Composio
    /// manages it.
    async fn choose_scheme(
        &self,
        client: &Client,
        service: &str,
        requested: Option<AuthSchemeKind>,
        credentials: &SecretEnv,
    ) -> Result<(composio::ToolkitDetail, AuthSchemeKind, bool)> {
        let detail = client.toolkit(service).await.map_err(|err| {
            if composio::is_not_found(&err) {
                HubError::not_found(format!(
                    "Composio has no service {service}. Search the catalog for its name."
                ))
            } else {
                err
            }
        })?;
        let managed: Vec<AuthSchemeKind> = detail
            .composio_managed_auth_schemes
            .iter()
            .map(|s| scheme_kind(s))
            .collect();
        let mut available: Vec<AuthSchemeKind> = managed.clone();
        for mode in &detail.auth_config_details {
            let kind = scheme_kind(&mode.mode);
            if !available.contains(&kind) {
                available.push(kind);
            }
        }
        if available.is_empty() && detail.no_auth {
            available.push(AuthSchemeKind::NoAuth);
        }
        let scheme = match requested {
            Some(kind) if available.contains(&kind) => kind,
            Some(kind) => {
                return Err(HubError::invalid(format!(
                    "{} does not sign in with {}. Choose one of the schemes in the catalog.",
                    detail.name,
                    serde_json::to_value(kind)?
                        .as_str()
                        .unwrap_or("this scheme")
                )));
            }
            None => managed
                .iter()
                .copied()
                .find(|k| k.uses_browser())
                .or_else(|| managed.first().copied())
                .or_else(|| available.iter().copied().find(|k| k.uses_browser()))
                .or_else(|| available.first().copied())
                .ok_or_else(|| {
                    HubError::invalid(format!(
                        "Composio has no sign-in for {}. Choose another service.",
                        detail.name
                    ))
                })?,
        };
        if scheme.uses_browser() {
            if !credentials.is_empty() {
                return Err(HubError::invalid(
                    "This service signs in in the browser, so it takes no credentials. Leave them out.",
                ));
            }
        } else if let Some(mode) = detail
            .auth_config_details
            .iter()
            .find(|d| scheme_kind(&d.mode) == scheme)
        {
            let missing: Vec<String> = mode
                .fields
                .connected_account_initiation
                .required
                .iter()
                .filter(|f| credentials.get(&f.name).is_none_or(|v| v.trim().is_empty()))
                .map(|f| {
                    if f.display_name.trim().is_empty() {
                        f.name.clone()
                    } else {
                        f.display_name.clone()
                    }
                })
                .collect();
            if !missing.is_empty() {
                return Err(HubError::invalid(format!(
                    "Fill in {} to connect {}.",
                    missing.join(", "),
                    detail.name
                )));
            }
        }
        let is_managed = managed.contains(&scheme);
        Ok((detail, scheme, is_managed))
    }

    /// Makes a connection. A browser scheme stores it in `connecting` with
    /// the sign-in URL and follows it until it is active.
    pub(crate) async fn connect_service(&self, spec: ConnectSpec) -> Result<ConnectAttempt> {
        let client = self.connectors.client()?;
        validate_connection_name(&spec.name)?;
        let name = spec.name.trim().to_owned();
        let use_in = self.check_use_in(spec.use_in).await?;
        let service = spec.service.trim().to_ascii_lowercase();
        let (detail, scheme, managed) = self
            .choose_scheme(&client, &service, spec.auth_scheme, &spec.credentials)
            .await?;
        let timeout = self.connectors.timings().connect_timeout_ms;
        let (service_name, logo_url) = (
            detail.name.clone(),
            self.hub_logo_url(detail.meta.logo.as_deref(), true),
        );
        let reserved = self
            .store
            .write(move |tx| {
                let handles = con_store::handles(tx.db())?;
                let now = tx.now();
                let connection = Connection {
                    id: ConnectionId::new(),
                    handle: unique_connection_handle(&name, handles.iter().map(String::as_str)),
                    name,
                    service: detail.slug.clone(),
                    service_name,
                    logo_url,
                    identity: None,
                    use_in,
                    approvals: spec.approvals,
                    status: ConnectionStatus::Connecting {
                        redirect_url: None,
                        expires_at: now.plus_millis(timeout),
                    },
                    last_used_at: None,
                    created_at: now,
                    updated_at: now,
                };
                tx.save_connection(&connection)?;
                Ok(connection)
            })
            .await?;
        let linked = self
            .link_account(
                &client,
                &service,
                scheme,
                managed,
                &reserved.handle,
                &spec.credentials,
            )
            .await;
        let linked = match linked {
            Ok(linked) => linked,
            Err(err) => {
                let id = reserved.id;
                self.store.write(move |tx| tx.delete_connection(id)).await?;
                return Err(err);
            }
        };
        self.finish_link(reserved, scheme, linked).await
    }

    /// Stores the Composio side of a connect attempt and starts to follow a
    /// browser sign-in.
    async fn finish_link(
        &self,
        connection: Connection,
        scheme: AuthSchemeKind,
        linked: Linked,
    ) -> Result<ConnectAttempt> {
        let timeout = self.connectors.timings().connect_timeout_ms;
        let scheme_name = serde_json::to_value(scheme)?
            .as_str()
            .unwrap_or("other")
            .to_owned();
        let connection = self
            .store
            .write(move |tx| {
                let mut connection = connection;
                let now = tx.now();
                connection.status = match &linked.account {
                    None => ConnectionStatus::Connecting {
                        redirect_url: linked.redirect_url.clone(),
                        expires_at: now.plus_millis(timeout),
                    },
                    Some(account) => match account.state() {
                        AccountState::Active => {
                            connection.identity = account.identity.clone();
                            ConnectionStatus::Active
                        }
                        AccountState::Waiting => ConnectionStatus::Connecting {
                            redirect_url: None,
                            expires_at: now.plus_millis(timeout),
                        },
                        AccountState::Expired | AccountState::Failed => ConnectionStatus::Failed {
                            message: failed_message(account.status_reason.as_deref()),
                        },
                    },
                };
                connection.updated_at = now;
                tx.save_connection(&connection)?;
                tx.set_connection_composio(
                    connection.id,
                    Some(&linked.account_id),
                    Some(&linked.auth_config_id),
                    Some(&scheme_name),
                )?;
                Ok(connection)
            })
            .await?;
        let (redirect_url, expires_at) = match &connection.status {
            ConnectionStatus::Connecting {
                redirect_url,
                expires_at,
            } => {
                self.spawn_connect_poll(connection.id);
                (redirect_url.clone(), Some(*expires_at))
            }
            _ => (None, None),
        };
        tracing::info!(connection_id = %connection.id, service = %connection.service, "a connect attempt started");
        Ok(ConnectAttempt {
            connection,
            redirect_url,
            expires_at,
        })
    }

    pub(crate) async fn connection_connect(
        &self,
        _caller: &Caller,
        request: ConnectionConnect,
    ) -> Result<ConnectAttempt> {
        // A connect card must wait for the user before the hub connects.
        if let Some(approval_id) = request.approval_id {
            self.pending_connect_card(approval_id).await?;
        }
        let attempt = self
            .connect_service(ConnectSpec {
                service: request.service,
                name: request.name,
                use_in: request.use_in,
                approvals: request.approvals,
                auth_scheme: request.auth_scheme,
                credentials: request.credentials,
            })
            .await?;
        if let Some(approval_id) = request.approval_id {
            self.close_connect_card(approval_id, &attempt.connection)
                .await?;
        }
        Ok(attempt)
    }

    /// The connect card with this ID, while it waits.
    async fn pending_connect_card(&self, approval_id: ApprovalId) -> Result<()> {
        let approval = self
            .store
            .read(move |conn| work::approval(conn, approval_id))
            .await?
            .ok_or_else(|| HubError::not_found("The connect card does not exist."))?;
        if !approval.is_hub_card()
            || !matches!(approval.details, ApprovalDetails::ConnectService(_))
        {
            return Err(HubError::invalid("This approval is not a connect card."));
        }
        if approval.status != ApprovalStatus::Pending {
            return Err(HubError::conflict(
                "This connect card is already answered or closed.",
            ));
        }
        Ok(())
    }

    pub(crate) async fn connection_update(
        &self,
        _caller: &Caller,
        request: ConnectionUpdate,
    ) -> Result<ConnectionResult> {
        let connection = self
            .update_connection_fields(
                request.connection_id,
                request.name,
                request.use_in,
                request.approvals,
            )
            .await?;
        Ok(ConnectionResult { connection })
    }

    /// Changes the name, the scope or the approvals of a connection. A new
    /// name gives a new handle.
    pub(crate) async fn update_connection_fields(
        &self,
        id: ConnectionId,
        name: Option<String>,
        use_in: Option<UseIn>,
        approvals: Option<ToolApprovals>,
    ) -> Result<Connection> {
        if let Some(name) = &name {
            validate_connection_name(name)?;
        }
        let use_in = match use_in {
            Some(u) => Some(self.check_use_in(u).await?),
            None => None,
        };
        self.store
            .write(move |tx| {
                let before = con_store::connection_or_404(tx.db(), id)?;
                let mut connection = before.clone();
                if let Some(name) = name {
                    let name = name.trim().to_owned();
                    if name != connection.name {
                        let handles = con_store::handles(tx.db())?;
                        connection.handle = unique_connection_handle(
                            &name,
                            handles
                                .iter()
                                .map(String::as_str)
                                .filter(|h| *h != before.handle),
                        );
                        connection.name = name;
                    }
                }
                if let Some(use_in) = use_in {
                    connection.use_in = use_in;
                }
                if let Some(approvals) = approvals {
                    connection.approvals = approvals;
                }
                if connection != before {
                    connection.updated_at = tx.now();
                    tx.save_connection(&connection)?;
                }
                Ok(connection)
            })
            .await
    }

    pub(crate) async fn connection_disconnect(
        &self,
        _caller: &Caller,
        request: ConnectionDisconnect,
    ) -> Result<Empty> {
        self.disconnect(request.connection_id).await?;
        Ok(Empty {})
    }

    /// Deletes the connected account at Composio, then the connection. With
    /// no key, only the connection goes.
    pub(crate) async fn disconnect(&self, id: ConnectionId) -> Result<Connection> {
        let row = self
            .store
            .read(move |conn| con_store::connection_row(conn, id))
            .await?
            .ok_or_else(|| HubError::not_found("The connection does not exist."))?;
        if let (Some(account), Ok(client)) = (&row.composio_account_id, self.connectors.client()) {
            client.delete_connected_account(account).await?;
        }
        self.store.write(move |tx| tx.delete_connection(id)).await?;
        tracing::info!(connection_id = %id, "a connection was removed");
        Ok(row.connection)
    }

    pub(crate) async fn connection_reconnect(
        &self,
        _caller: &Caller,
        request: ConnectionReconnect,
    ) -> Result<ConnectAttempt> {
        let client = self.connectors.client()?;
        let id = request.connection_id;
        let row = self
            .store
            .read(move |conn| con_store::connection_row(conn, id))
            .await?
            .ok_or_else(|| HubError::not_found("The connection does not exist."))?;
        let requested = row.auth_scheme.as_deref().map(scheme_kind);
        let (_, scheme, managed) = self
            .choose_scheme(
                &client,
                &row.connection.service,
                requested,
                &request.credentials,
            )
            .await?;
        // The alias must be unique, so the old account goes first. A
        // reconnect is for an account that does not work anymore.
        if let Some(account) = &row.composio_account_id {
            client.delete_connected_account(account).await?;
        }
        let timeout = self.connectors.timings().connect_timeout_ms;
        let connection = self
            .store
            .write(move |tx| {
                let mut connection = con_store::connection_or_404(tx.db(), id)?;
                let now = tx.now();
                connection.status = ConnectionStatus::Connecting {
                    redirect_url: None,
                    expires_at: now.plus_millis(timeout),
                };
                connection.updated_at = now;
                tx.save_connection(&connection)?;
                tx.set_connection_composio(id, None, None, None)?;
                Ok(connection)
            })
            .await?;
        let linked = self
            .link_account(
                &client,
                &connection.service,
                scheme,
                managed,
                &connection.handle,
                &request.credentials,
            )
            .await;
        match linked {
            Ok(linked) => self.finish_link(connection, scheme, linked).await,
            Err(err) => {
                let message = err.to_error_body().message;
                self.set_connection_status(id, ConnectionStatus::Failed { message }, None, false)
                    .await?;
                Err(err)
            }
        }
    }

    pub(crate) async fn connection_calls(
        &self,
        _caller: &Caller,
        request: ConnectionCalls,
    ) -> Result<ConnectionCallsResult> {
        let limit = request
            .limit
            .unwrap_or(CALLS_PAGE_DEFAULT)
            .clamp(1, CONNECTOR_CALLS_PAGE_MAX);
        let (calls, has_more) = self
            .store
            .read(move |conn| {
                con_store::connector_calls(conn, request.connection_id, request.before, limit)
            })
            .await?;
        Ok(ConnectionCallsResult { calls, has_more })
    }

    /// Sets the status of a connection. With `only_if_connecting`, a
    /// connection that left `connecting` keeps its status, so a late poll
    /// does not undo a newer change. Returns the connection when it
    /// changed.
    pub(crate) async fn set_connection_status(
        &self,
        id: ConnectionId,
        status: ConnectionStatus,
        identity: Option<String>,
        only_if_connecting: bool,
    ) -> Result<Option<Connection>> {
        self.store
            .write(move |tx| {
                let Some(mut connection) = con_store::connection(tx.db(), id)? else {
                    return Ok(None);
                };
                if only_if_connecting
                    && !matches!(connection.status, ConnectionStatus::Connecting { .. })
                {
                    return Ok(None);
                }
                if connection.status == status && identity.is_none() {
                    return Ok(None);
                }
                connection.status = status;
                if identity.is_some() {
                    connection.identity = identity;
                }
                connection.updated_at = tx.now();
                tx.save_connection(&connection)?;
                Ok(Some(connection))
            })
            .await
    }

    /// Follows a browser sign-in until the account is active, fails or the
    /// time runs out. One task per connection.
    pub(crate) fn spawn_connect_poll(&self, id: ConnectionId) {
        if !self.connectors.start_polling(id) {
            return;
        }
        let hub = self.clone();
        tokio::spawn(async move {
            if let Err(err) = hub.poll_connection(id).await {
                tracing::warn!(error = %err, connection_id = %id, "could not follow a connect attempt");
            }
            hub.connectors.stop_polling(id);
        });
    }

    async fn poll_connection(&self, id: ConnectionId) -> Result<()> {
        loop {
            let interval = self.connectors.timings().poll_interval;
            tokio::select! {
                () = self.shutdown.cancelled() => return Ok(()),
                () = tokio::time::sleep(interval) => {}
            }
            let Some(row) = self
                .store
                .read(move |conn| con_store::connection_row(conn, id))
                .await?
            else {
                return Ok(());
            };
            let ConnectionStatus::Connecting { expires_at, .. } = row.connection.status else {
                return Ok(());
            };
            if Timestamp::now() >= expires_at {
                self.set_connection_status(
                    id,
                    ConnectionStatus::Failed {
                        message: "The sign-in did not finish in 10 minutes. Connect again."
                            .to_owned(),
                    },
                    None,
                    true,
                )
                .await?;
                return Ok(());
            }
            let Some(account_id) = row.composio_account_id else {
                continue;
            };
            let client = match self.connectors.client() {
                Ok(client) => client,
                Err(_) => {
                    self.set_connection_status(
                        id,
                        ConnectionStatus::Failed {
                            message:
                                "The Composio key was removed. Add it again, then connect again."
                                    .to_owned(),
                        },
                        None,
                        true,
                    )
                    .await?;
                    return Ok(());
                }
            };
            match client.connected_account(&account_id).await {
                Ok(account) => match account.state() {
                    AccountState::Waiting => {}
                    AccountState::Active => {
                        self.set_connection_status(
                            id,
                            ConnectionStatus::Active,
                            account.identity,
                            true,
                        )
                        .await?;
                        return Ok(());
                    }
                    AccountState::Expired | AccountState::Failed => {
                        self.set_connection_status(
                            id,
                            ConnectionStatus::Failed {
                                message: failed_message(account.status_reason.as_deref()),
                            },
                            None,
                            true,
                        )
                        .await?;
                        return Ok(());
                    }
                },
                Err(err) if composio::is_not_found(&err) => {
                    self.set_connection_status(
                        id,
                        ConnectionStatus::Failed {
                            message: "Composio no longer has this sign-in. Connect again."
                                .to_owned(),
                        },
                        None,
                        true,
                    )
                    .await?;
                    return Ok(());
                }
                // A short outage: ask again at the next tick.
                Err(_) => {}
            }
        }
    }

    /// Starts to follow the connect attempts that were open when the hub
    /// stopped.
    pub(crate) async fn resume_connect_polls(&self) -> Result<()> {
        for connection in self.store.read(con_store::connecting).await? {
            self.spawn_connect_poll(connection.id);
        }
        Ok(())
    }

    /// Asks Composio about each active connection. One that Composio
    /// reports as expired becomes "needs a new sign-in".
    pub(crate) async fn sweep_connection_status(&self) -> Result<()> {
        let Ok(client) = self.connectors.client() else {
            return Ok(());
        };
        // Also ask whether Composio still accepts the key, so a key that was
        // revoked shows in `composio.status`.
        self.refresh_composio_status().await?;
        let connections = self.store.read(con_store::connections).await?;
        for connection in connections.into_iter().filter(|c| c.status.is_active()) {
            let id = connection.id;
            let Some(row) = self
                .store
                .read(move |conn| con_store::connection_row(conn, id))
                .await?
            else {
                continue;
            };
            let Some(account_id) = row.composio_account_id else {
                continue;
            };
            self.check_account(&client, id, &account_id).await;
        }
        Ok(())
    }

    /// Reads one connected account and marks the connection when Composio
    /// says it expired or is gone. Returns true when it needs a new
    /// sign-in.
    pub(crate) async fn check_account(
        &self,
        client: &Client,
        id: ConnectionId,
        account_id: &str,
    ) -> bool {
        let message = match client.connected_account(account_id).await {
            Ok(account)
                if matches!(
                    account.state(),
                    AccountState::Expired | AccountState::Failed
                ) =>
            {
                Some(EXPIRED_MESSAGE)
            }
            Err(err) if composio::is_not_found(&err) => Some(OTHER_PROJECT_MESSAGE),
            _ => None,
        };
        let Some(message) = message else {
            return false;
        };
        let status = ConnectionStatus::NeedsSignIn {
            message: Some(message.to_owned()),
        };
        if let Err(err) = self.set_connection_status(id, status, None, false).await {
            tracing::warn!(error = %err, "could not mark a connection as needing a new sign-in");
        }
        true
    }

    /// Checks every connection against the key that the hub has now. The key
    /// can belong to another Composio project, and that project has none of
    /// the old connected accounts. A connection whose account Composio does
    /// not know is marked "needs a new sign-in", with a message that names
    /// the cause, and it stays in the list, so the user can reconnect it.
    /// When the old key comes back, the accounts are found again and the
    /// connections work again. Returns how many connections it marked.
    pub(crate) async fn reconcile_connections_with_key(&self) -> Result<usize> {
        let Ok(client) = self.connectors.client() else {
            return Ok(0);
        };
        let rows = self.store.read(con_store::connection_rows).await?;
        let mut marked = 0;
        for row in rows {
            let Some(account_id) = row.composio_account_id.clone() else {
                continue;
            };
            let id = row.connection.id;
            match row.connection.status {
                ConnectionStatus::Active => {
                    if self.check_account(&client, id, &account_id).await {
                        marked += 1;
                    }
                }
                // Back in the project that knows the account: it works again.
                ConnectionStatus::NeedsSignIn { ref message }
                    if message.as_deref() == Some(OTHER_PROJECT_MESSAGE) =>
                {
                    if let Ok(account) = client.connected_account(&account_id).await
                        && account.state() == AccountState::Active
                    {
                        self.set_connection_status(
                            id,
                            ConnectionStatus::Active,
                            account.identity,
                            false,
                        )
                        .await?;
                    }
                }
                _ => {}
            }
        }
        if marked > 0 {
            tracing::warn!(
                marked,
                "connections belong to another Composio project and need a new sign-in"
            );
        }
        Ok(marked)
    }

    // Channel exceptions -------------------------------------------------------

    pub(crate) async fn channel_set_connection_enabled(
        &self,
        _caller: &Caller,
        request: ChannelSetConnectionEnabled,
    ) -> Result<ChannelResult> {
        let channel = self
            .store
            .write(move |tx| {
                let mut channel = conversation::channel_or_404(tx.db(), request.channel_id)?;
                let connection = con_store::connection_or_404(tx.db(), request.connection_id)?;
                let id = connection.id;
                if request.enabled {
                    if !connection.use_in.matches_channel(&channel) {
                        return Err(HubError::conflict(format!(
                            "{} is not available in this channel. Change its Use in first.",
                            connection.name
                        )));
                    }
                    if !channel.disabled_connections.contains(&id) {
                        return Ok(channel);
                    }
                    channel.disabled_connections.retain(|c| *c != id);
                } else {
                    if channel.disabled_connections.contains(&id) {
                        return Ok(channel);
                    }
                    channel.disabled_connections.push(id);
                }
                channel.updated_at = tx.now();
                tx.save_channel(&channel)?;
                Ok(channel)
            })
            .await?;
        Ok(ChannelResult { channel })
    }

    pub(crate) async fn channel_set_skill_enabled(
        &self,
        _caller: &Caller,
        request: ChannelSetSkillEnabled,
    ) -> Result<ChannelResult> {
        let channel = self
            .store
            .write(move |tx| {
                let mut channel = conversation::channel_or_404(tx.db(), request.channel_id)?;
                let skill = skill_store::skill_or_404(tx.db(), request.skill_id)?;
                let id = skill.id;
                if request.enabled {
                    if !skill.use_in.matches_channel(&channel) {
                        return Err(HubError::conflict(format!(
                            "The skill {} is not available in this channel. Change its Use in first.",
                            skill.name
                        )));
                    }
                    if !channel.disabled_skills.contains(&id) {
                        return Ok(channel);
                    }
                    channel.disabled_skills.retain(|s| *s != id);
                } else {
                    if channel.disabled_skills.contains(&id) {
                        return Ok(channel);
                    }
                    channel.disabled_skills.push(id);
                }
                channel.updated_at = tx.now();
                tx.save_channel(&channel)?;
                Ok(channel)
            })
            .await?;
        Ok(ChannelResult { channel })
    }
}

/// The message of a connection whose sign-in Composio reports as expired.
const EXPIRED_MESSAGE: &str = "The sign-in expired. Reconnect it in Settings > Connectors.";
/// The message of a connection whose account Composio does not know. This
/// is what a key of another Composio project gives.
pub(crate) const OTHER_PROJECT_MESSAGE: &str = "Composio does not know this account with the current key. It may belong to another Composio project. Reconnect it in Settings > Connectors, or add the old key again.";

fn failed_message(reason: Option<&str>) -> String {
    match reason.map(str::trim).filter(|r| !r.is_empty()) {
        Some(reason) => format!(
            "The sign-in failed: {}. Connect again.",
            crate::util::truncate_chars(reason.trim_end_matches('.'), 300)
        ),
        None => "The sign-in failed. Connect again.".to_owned(),
    }
}
