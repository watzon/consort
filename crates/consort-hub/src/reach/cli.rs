//! `consort hub reach status|tailscale|off`: for the owner of a server, on the
//! hub machine. These commands open the same store as the hub. The hub reads
//! the store for each signature, so a change counts with no restart. Its TLS
//! port follows within seconds.

use std::path::Path;

use consort_proto::client_api::{ReachMethod, TailscaleState};
use consort_proto::time::Timestamp;

use super::program::{SystemTailscale, TailscaleProgram};
use super::tailscale;
use crate::error::{HubError, Result};
use crate::store::reach::{self as reach_store, ReachRow, TARGET_KEY};
use crate::store::{meta_get, open_and_migrate};

/// What to turn off. No value means every way.
pub type OffScope = Option<ReachMethod>;

fn target_of(conn: &rusqlite::Connection) -> Result<String> {
    meta_get(conn, TARGET_KEY)?.ok_or_else(|| {
        HubError::invalid("This hub has not started yet. Start it one time, then run this again.")
    })
}

/// A line for a Tailscale state.
fn state_line(state: &TailscaleState) -> String {
    match state {
        TailscaleState::NotInstalled => {
            "Tailscale is not on this computer. Install it and sign in.".to_owned()
        }
        TailscaleState::NotConnected => {
            "Tailscale is not connected. Run `tailscale up` and sign in.".to_owned()
        }
        TailscaleState::HttpsOff { action_url, .. } => {
            format!("The tailnet needs HTTPS. An admin must turn it on, one time, at {action_url}")
        }
        TailscaleState::NeedsPermission { command, .. } => {
            format!("This user may not change Tailscale. Run `{command}` one time, then try again.")
        }
        TailscaleState::Available { address, .. } => {
            format!("Tailscale is off. The hub would get the address {address}.")
        }
        TailscaleState::NoFreePort { .. } => {
            "Other services use the ports 443 and 8443. Free one of them, then try again."
                .to_owned()
        }
        TailscaleState::On { address, .. } => format!("Tailscale is on at {address}."),
        TailscaleState::Removed { address } => format!(
            "The Tailscale entry for {address} is gone. Run `consort hub reach tailscale` to add it, or `consort hub reach off` to forget the address."
        ),
        TailscaleState::Failed { detail } => format!("Tailscale gave an error: {detail}"),
        TailscaleState::Unknown => {
            "Tailscale is in a state that this program does not know.".to_owned()
        }
    }
}

/// `consort hub reach status`, with a chosen program.
pub async fn status_with(data_dir: &Path, program: &dyn TailscaleProgram) -> Result<Vec<String>> {
    let conn = open_and_migrate(data_dir)?;
    let rows = reach_store::rows(&conn)?;
    let target = meta_get(&conn, TARGET_KEY)?.unwrap_or_default();
    let saved = rows.iter().find(|r| r.method == ReachMethod::Tailscale);
    let state = tailscale::read_state(program, saved, &target).await;
    let mut lines = vec![state_line(&state)];
    lines.push(
        match rows.iter().find(|r| r.method == ReachMethod::ThisNetwork) {
            Some(row) => format!("This network is on at {}.", row.address),
            None => "This network is off.".to_owned(),
        },
    );
    Ok(lines)
}

/// `consort hub reach tailscale`, with a chosen program.
pub async fn tailscale_with(
    data_dir: &Path,
    program: &dyn TailscaleProgram,
) -> Result<Vec<String>> {
    let conn = open_and_migrate(data_dir)?;
    let target = target_of(&conn)?;
    let previous = reach_store::row(&conn, ReachMethod::Tailscale)?;
    match tailscale::turn_on(program, previous.as_ref(), &target).await {
        Ok(on) => {
            let line = format!("The hub answers at {}.", on.address);
            reach_store::save(
                &conn,
                &ReachRow {
                    method: ReachMethod::Tailscale,
                    address: on.address,
                    port: on.port,
                    target: Some(target),
                    turned_on_at: Timestamp::now(),
                },
            )?;
            Ok(vec![
                line,
                "The hub accepts this address now. It needs no restart.".to_owned(),
            ])
        }
        Err(state) => Err(HubError::invalid(state_line(&state))),
    }
}

/// `consort hub reach off`, with a chosen program.
pub async fn off_with(
    data_dir: &Path,
    scope: OffScope,
    program: &dyn TailscaleProgram,
) -> Result<Vec<String>> {
    let conn = open_and_migrate(data_dir)?;
    let mut lines = Vec::new();
    if scope != Some(ReachMethod::ThisNetwork) {
        if let Some(row) = reach_store::row(&conn, ReachMethod::Tailscale)? {
            tailscale::turn_off(program, &row)
                .await
                .map_err(|state| HubError::invalid(state_line(&state)))?;
            reach_store::remove(&conn, ReachMethod::Tailscale)?;
            lines.push(format!(
                "Tailscale is off. {} no longer works.",
                row.address
            ));
        } else {
            lines.push("Tailscale was off.".to_owned());
        }
    }
    if scope != Some(ReachMethod::Tailscale) {
        if let Some(row) = reach_store::row(&conn, ReachMethod::ThisNetwork)? {
            reach_store::remove(&conn, ReachMethod::ThisNetwork)?;
            lines.push(format!(
                "This network is off. {} closes in a few seconds.",
                row.address
            ));
        } else {
            lines.push("This network was off.".to_owned());
        }
    }
    Ok(lines)
}

/// `consort hub reach status`.
pub async fn status(data_dir: &Path) -> Result<Vec<String>> {
    status_with(data_dir, &SystemTailscale::default()).await
}

/// `consort hub reach tailscale`.
pub async fn turn_on_tailscale(data_dir: &Path) -> Result<Vec<String>> {
    tailscale_with(data_dir, &SystemTailscale::default()).await
}

/// `consort hub reach off`.
pub async fn off(data_dir: &Path, scope: OffScope) -> Result<Vec<String>> {
    off_with(data_dir, scope, &SystemTailscale::default()).await
}
