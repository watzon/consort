//! The `consort` program: the hub, the worker, a dev mode that runs both,
//! and the stdio MCP bridge.

use std::io::BufRead;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use consort_session_host::HostConfig;
use consort_session_host::config::HostToml;

mod hub_update;
mod local;
mod service;
mod service_cmd;

/// The default hub address.
const DEFAULT_LISTEN: &str = "127.0.0.1:7600";

#[derive(Parser)]
#[command(
    name = "consort",
    version,
    about = "Consort: a personal assistant that runs on your own machines."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the hub, or manage it.
    Hub(HubArgs),
    /// Pair or run a worker.
    Worker {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    /// Pair or run the session host: the process on the hub machine that
    /// holds the accounts and runs the harnesses.
    SessionHost {
        #[command(subcommand)]
        command: SessionHostCommand,
    },
    /// Run a hub, a session host and a worker on this computer, in one service
    /// for your user. The app runs these commands.
    Local {
        #[command(subcommand)]
        command: local::LocalCommand,
    },
    /// Run a hub and a worker in one process, for development. The worker
    /// keeps its profile folders and managed tools in <data>/home, unless
    /// CONSORT_HOME is set.
    Dev(DevArgs),
    /// Runs a harness inside the Linux boundary. The session host starts this
    /// inside `bwrap`: it opens the ports of the egress proxy and the MCP
    /// endpoints in the empty network, then runs the harness.
    #[cfg(unix)]
    #[command(name = "boundary-run", hide = true)]
    BoundaryRun {
        /// `<port>=<socket>`: a local port and the Unix socket behind it.
        #[arg(long = "bridge")]
        bridges: Vec<String>,
        /// The program and its arguments.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Bridge stdio to a session's local MCP endpoint. The token comes from
    /// CONSORT_MCP_TOKEN.
    McpBridge {
        /// The endpoint URL, `http://127.0.0.1:<port>/mcp/<session_id>`.
        #[arg(long)]
        url: String,
    },
    /// Prints the `release.json` of this program. The release workflow runs
    /// this.
    #[command(name = "release-info", hide = true)]
    ReleaseInfo,
}

#[derive(Args)]
struct HubArgs {
    #[command(subcommand)]
    command: Option<HubCommand>,
    /// The data folder, with hub.db and blobs/. Can also come from
    /// CONSORT_HUB_DATA.
    #[arg(long, env = "CONSORT_HUB_DATA")]
    data: Option<PathBuf>,
    /// The address to listen on. Can also come from CONSORT_HUB_LISTEN.
    #[arg(long, env = "CONSORT_HUB_LISTEN", default_value = DEFAULT_LISTEN)]
    listen: SocketAddr,
    /// The URL that clients and workers use, when it is not http://<listen>.
    /// Behind a reverse proxy, it is the HTTPS URL of the proxy. Can also
    /// come from CONSORT_PUBLIC_URL.
    #[arg(long, env = "CONSORT_PUBLIC_URL")]
    public_url: Option<String>,
}

#[derive(Subcommand)]
enum HubCommand {
    /// Make a new client setup code. Works while the hub runs.
    SetupCode {
        /// The data folder of the hub.
        #[arg(long)]
        data: Option<PathBuf>,
    },
    /// Make a code that pairs the session host. Works while the hub runs.
    SessionHostCode {
        /// The data folder of the hub.
        #[arg(long)]
        data: Option<PathBuf>,
    },
    /// List or remove the clients of the hub. Works while the hub runs.
    ///
    /// Use it when a client key is lost or stolen: remove that client
    /// first, then make a new setup code. Removing a client also cancels
    /// the pairing codes that it made.
    Client {
        #[command(subcommand)]
        command: PairedCommand,
    },
    /// List or remove the workers of the hub. Works while the hub runs.
    Worker {
        #[command(subcommand)]
        command: PairedCommand,
    },
    /// Show or change how other devices reach this hub, on the hub machine.
    ///
    /// Run it as the owner of the hub, or as root on a server. It works while
    /// the hub runs, and the hub needs no restart. A hub on a server cannot
    /// change Tailscale itself, so this command does.
    Reach {
        #[command(subcommand)]
        command: ReachCommand,
        /// The data folder of the hub.
        #[arg(long, global = true)]
        data: Option<PathBuf>,
    },
    /// Replace this program with a release, after a check of its SHA-256.
    ///
    /// Back up first. Run it as root with systemd, and it also restarts the
    /// services. Otherwise it says what to restart. A container gets a new
    /// image in place of this command.
    Update {
        /// The version to install, such as 0.8.0. The default is the latest
        /// release. An older version is refused.
        #[arg(long)]
        version: Option<String>,
    },
    /// Restore a backup into a data folder. Stop the hub first.
    ///
    /// The backup is a consort-<date>.tar file from the backup folder that
    /// is set in Settings > Hub. A data folder that is not empty needs
    /// --force: its database, blobs and keys then move to a
    /// before-restore-<time> folder inside it first.
    Restore {
        /// The backup file, for example
        /// /srv/backups/consort/consort-2026-10-29-0300.tar.
        backup: PathBuf,
        /// The data folder of the hub.
        #[arg(long)]
        data: Option<PathBuf>,
        /// Restore into a data folder that is not empty.
        #[arg(long)]
        force: bool,
    },
}

/// What to do with the paired clients or workers.
#[derive(Subcommand)]
enum PairedCommand {
    /// Show each one with its ID, who paired it and whether it was removed.
    List {
        /// The data folder of the hub.
        #[arg(long)]
        data: Option<PathBuf>,
    },
    /// Remove one by its ID. It can no longer connect. A running hub
    /// closes its connection within a few seconds.
    Revoke {
        /// The ID from `list`, for example cli_0192f0c4-... or
        /// wrk_0192f0c4-....
        id: String,
        /// The data folder of the hub.
        #[arg(long)]
        data: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ReachCommand {
    /// Show the state of Tailscale and This network.
    Status,
    /// Give the hub an address on the tailnet with `tailscale serve`. It uses
    /// the first free port of 443 and 8443, and it leaves other entries alone.
    Tailscale,
    /// Turn off a way to reach the hub. The default is every way. Its address
    /// stops working at once.
    Off {
        /// Only this way.
        #[arg(long, value_enum)]
        only: Option<ReachOffScope>,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ReachOffScope {
    Tailscale,
    ThisNetwork,
}

#[derive(Subcommand)]
enum WorkerCommand {
    /// Pair this machine with a hub.
    Pair {
        /// The hub URL, for example https://hub.example.com.
        hub_url: String,
        /// The pairing code from a client, for example ABCD-EFGH. Without
        /// it, the code comes from CONSORT_PAIRING_CODE or from a prompt.
        /// Other users of this machine can see a code on the command line.
        #[arg(long, env = "CONSORT_PAIRING_CODE", hide_env_values = true)]
        code: Option<String>,
        /// The SHA-256 of the certificate of a hub on This network. The worker
        /// accepts this certificate and no other for this hub.
        #[arg(long = "tls-sha256")]
        tls_sha256: Option<consort_proto::model::Sha256Hash>,
        /// The worker name. The default is the host name.
        #[arg(long)]
        name: Option<String>,
        /// A tag for placement, such as gpu. Repeat it for more tags.
        /// `--label` is an older name for it.
        #[arg(long = "tag", alias = "label")]
        tags: Vec<String>,
        /// The worker folder.
        #[arg(long, env = "CONSORT_WORKER_DIR")]
        config_dir: Option<PathBuf>,
    },
    /// Run the worker until Ctrl-C, SIGTERM or SIGHUP.
    ///
    /// When the worker stops, it closes each harness and keeps the open
    /// sessions, so the next run resumes them. Only one worker can use a
    /// worker folder at a time.
    ///
    /// In a systemd unit, set `KillMode=mixed`. Then SIGTERM goes to the
    /// worker only, and the worker stops the harnesses itself. With the
    /// default `KillMode=control-group`, SIGTERM reaches every harness at
    /// the same time, and a harness can stop in the middle of its work.
    Run {
        /// The worker folder.
        #[arg(long, env = "CONSORT_WORKER_DIR")]
        config_dir: Option<PathBuf>,
        /// Write the log to this file, not to the terminal. A log larger
        /// than 10 MiB moves to `<file>.1` when the worker starts.
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    /// Pair this machine with a hub, then install a service that starts the
    /// worker when you sign in and starts it again after a failure.
    ///
    /// The service is a launchd agent on macOS, a systemd user unit on
    /// Linux and a Task Scheduler task on Windows. It needs no admin rights.
    Install(service_cmd::InstallArgs),
    /// Stop the worker service and remove it. The worker folder stays.
    Uninstall(service_cmd::ServiceArgs),
    /// Show whether the worker service is installed and running.
    Status(service_cmd::ServiceArgs),
}

#[derive(Subcommand)]
enum SessionHostCommand {
    /// Pair the session host with the hub on this machine.
    Pair {
        /// The hub URL. The session host runs on the hub machine, so this is
        /// usually http://127.0.0.1:7600.
        hub_url: String,
        /// The pairing code that `consort hub session-host-code` prints.
        #[arg(long, env = "CONSORT_PAIRING_CODE", hide_env_values = true)]
        code: Option<String>,
        /// The session host folder.
        #[arg(long, env = "CONSORT_SESSION_HOST_DIR")]
        config_dir: PathBuf,
    },
    /// Run the session host until Ctrl-C, SIGTERM or SIGHUP.
    Run {
        /// The session host folder.
        #[arg(long, env = "CONSORT_SESSION_HOST_DIR")]
        config_dir: PathBuf,
        /// The Consort home, with the account profiles and the managed tools.
        #[arg(long, env = "CONSORT_HOME")]
        consort_home: Option<PathBuf>,
        /// Write the log to this file, not to the terminal.
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
}

#[derive(Args)]
struct DevArgs {
    /// The data folder. The hub uses it, and the worker uses <data>/worker.
    #[arg(long)]
    data: PathBuf,
    /// The address to listen on.
    #[arg(long, default_value = DEFAULT_LISTEN)]
    listen: SocketAddr,
}

/// The size above which a log file moves aside when the worker starts.
const LOG_LIMIT: u64 = 10 * 1024 * 1024;

/// The log filter. At its trace level, the WebSocket library logs whole frames,
/// and a frame can hold a key that a client sends to the hub. So this library
/// never logs below `info`, whatever `RUST_LOG` says.
fn log_filter(
    from_env: std::result::Result<
        tracing_subscriber::EnvFilter,
        tracing_subscriber::filter::FromEnvError,
    >,
) -> Result<tracing_subscriber::EnvFilter> {
    let mut filter = from_env.unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    for directive in consort_proto::logs::WEBSOCKET_LOG_CAP {
        filter = filter.add_directive(directive.parse()?);
    }
    Ok(filter)
}

fn init_logging(log_file: Option<&Path>) -> Result<()> {
    let filter = log_filter(tracing_subscriber::EnvFilter::try_from_default_env())?;
    if let Some(path) = log_file {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not make the folder {}", parent.display()))?;
        }
        service::rotate_log(path, LOG_LIMIT)
            .with_context(|| format!("could not move the old log {}", path.display()))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("could not open the log file {}", path.display()))?;
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init();
        return Ok(());
    }
    // Logs go to stderr, so stdout stays free for mcp-bridge.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    Ok(())
}

fn default_hub_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "", "consort")
        .map(|d| d.data_dir().join("hub"))
        .context("could not find a data folder; pass --data")
}

fn worker_dir(given: Option<PathBuf>) -> Result<PathBuf> {
    match given {
        Some(dir) => Ok(dir),
        None => directories::ProjectDirs::from("", "", "consort")
            .map(|d| d.config_dir().join("worker"))
            .context("could not find a config folder; pass --config-dir"),
    }
}

/// The signals that stop the hub and the worker: Ctrl-C, and on Unix also
/// SIGTERM (from launchd or systemd) and SIGHUP (when the terminal closes).
/// On Windows, closing the console and a system shutdown stop them too.
struct StopSignals {
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    hangup: Option<tokio::signal::unix::Signal>,
    #[cfg(windows)]
    close: Option<tokio::signal::windows::CtrlClose>,
    #[cfg(windows)]
    shutdown: Option<tokio::signal::windows::CtrlShutdown>,
}

impl StopSignals {
    /// Starts to listen. From now on, these signals no longer end the
    /// process at once.
    fn listen() -> Self {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let listen = |kind: SignalKind, name: &str| match signal(kind) {
                Ok(s) => Some(s),
                Err(err) => {
                    tracing::warn!(error = %err, "could not listen for {name}");
                    None
                }
            };
            Self {
                terminate: listen(SignalKind::terminate(), "SIGTERM"),
                hangup: listen(SignalKind::hangup(), "SIGHUP"),
            }
        }
        #[cfg(windows)]
        {
            Self {
                close: tokio::signal::windows::ctrl_close().ok(),
                shutdown: tokio::signal::windows::ctrl_shutdown().ok(),
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            Self {}
        }
    }

    /// Waits for the first stop signal.
    async fn wait(mut self) {
        async fn next<F: std::future::Future>(f: Option<F>) {
            match f {
                Some(f) => {
                    f.await;
                }
                None => std::future::pending::<()>().await,
            }
        }
        let ctrl_c = async {
            if let Err(err) = tokio::signal::ctrl_c().await {
                tracing::warn!(error = %err, "could not listen for Ctrl-C");
                std::future::pending::<()>().await;
            }
        };
        #[cfg(unix)]
        {
            tokio::select! {
                () = ctrl_c => {}
                () = next(self.terminate.as_mut().map(|s| s.recv())) => {
                    tracing::info!("SIGTERM: stopping");
                }
                () = next(self.hangup.as_mut().map(|s| s.recv())) => {
                    tracing::info!("SIGHUP: stopping");
                }
            }
        }
        #[cfg(windows)]
        {
            tokio::select! {
                () = ctrl_c => {}
                () = next(self.close.as_mut().map(|s| s.recv())) => {}
                () = next(self.shutdown.as_mut().map(|s| s.recv())) => {}
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            ctrl_c.await;
        }
    }
}

async fn wait_for_stop() {
    StopSignals::listen().wait().await;
}

/// The pairing code: from `--code` or CONSORT_PAIRING_CODE (`given`), or
/// else one line from `input`, after a prompt on stderr.
fn pairing_code(given: Option<String>, input: &mut dyn BufRead) -> Result<String> {
    if let Some(code) = given.filter(|c| !c.trim().is_empty()) {
        return Ok(code.trim().to_owned());
    }
    eprint!("Pairing code from a client (for example ABCD-EFGH): ");
    let mut line = String::new();
    input
        .read_line(&mut line)
        .context("could not read the pairing code")?;
    let code = line.trim();
    if code.is_empty() {
        bail!("no pairing code. Pass --code, set CONSORT_PAIRING_CODE, or type the code");
    }
    Ok(code.to_owned())
}

/// The text for who paired or removed a client or a worker.
fn origin_text(origin: Option<consort_hub::admin::Origin>) -> String {
    origin.map_or_else(|| "unknown".to_owned(), |o| o.to_string())
}

/// One line per client, with what the user needs to choose one to remove.
fn client_lines(records: &[consort_hub::admin::ClientRecord]) -> Vec<String> {
    records
        .iter()
        .map(|r| {
            let c = &r.client;
            let kind = match c.kind {
                consort_proto::model::ClientKind::Desktop => "desktop",
                consort_proto::model::ClientKind::Ios => "iOS",
                consort_proto::model::ClientKind::Android => "Android",
            };
            let mut line = format!(
                "{}  {}  ({kind})  paired {} by {}",
                c.id,
                c.name,
                c.paired_at,
                origin_text(r.paired_by)
            );
            match c.last_seen_at {
                Some(at) => line.push_str(&format!(", last seen {at}")),
                None => line.push_str(", never connected"),
            }
            if let Some(at) = c.revoked_at {
                line.push_str(&format!(", removed {at} by {}", origin_text(r.revoked_by)));
            }
            line
        })
        .collect()
}

/// One line per worker, with what the user needs to choose one to remove.
fn worker_lines(records: &[consort_hub::admin::WorkerRecord]) -> Vec<String> {
    records
        .iter()
        .map(|r| {
            let w = &r.worker;
            let mut line = format!(
                "{}  {}  (host {})  paired {} by {}",
                w.id,
                w.name,
                w.hostname,
                w.paired_at,
                origin_text(r.paired_by)
            );
            match w.last_seen_at {
                Some(at) => line.push_str(&format!(", last seen {at}")),
                None => line.push_str(", never connected"),
            }
            if let Some(at) = w.revoked_at {
                line.push_str(&format!(", removed {at} by {}", origin_text(r.revoked_by)));
            }
            line
        })
        .collect()
}

/// A hub error as one line. Its source repeats the same text.
fn one_line(err: consort_hub::HubError) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

fn hub_data(given: Option<PathBuf>, fallback: Option<PathBuf>) -> Result<PathBuf> {
    match given.or(fallback) {
        Some(d) => Ok(d),
        None => default_hub_dir(),
    }
}

/// Runs `consort hub restore` and returns the lines to print.
fn run_restore(backup: &Path, data: &Path, force: bool) -> Result<Vec<String>> {
    let report =
        consort_hub::admin::restore(backup, data, force).map_err(|err| anyhow::anyhow!("{err}"))?;
    let mut lines = vec![format!(
        "Restored the backup from {} (hub {}, {} blobs) into {}.",
        report.created_at,
        report.hub_version,
        report.blobs,
        data.display()
    )];
    if let Some(aside) = report.moved_aside {
        lines.push(format!(
            "The files that were in the data folder are now in {}. Delete that folder when the hub works.",
            aside.display()
        ));
    }
    lines.push(format!(
        "Start the hub with `consort hub --data {}`.",
        data.display()
    ));
    Ok(lines)
}

/// Runs `consort hub client ...` or `consort hub worker ...`.
fn run_paired(clients: bool, command: PairedCommand, fallback: Option<PathBuf>) -> Result<()> {
    let what = if clients { "client" } else { "worker" };
    match command {
        PairedCommand::List { data } => {
            let data = hub_data(data, fallback)?;
            let lines = if clients {
                client_lines(
                    &consort_hub::admin::list_clients(&data)
                        .map_err(one_line)
                        .context("could not read the clients")?,
                )
            } else {
                worker_lines(
                    &consort_hub::admin::list_workers(&data)
                        .map_err(one_line)
                        .context("could not read the workers")?,
                )
            };
            if lines.is_empty() {
                println!("The hub has no {what}s.");
            }
            for line in lines {
                println!("{line}");
            }
        }
        PairedCommand::Revoke { id, data } => {
            let data = hub_data(data, fallback)?;
            let removed = if clients {
                let id = id.trim().parse().with_context(|| {
                    format!("{id} is not a client ID. Copy an ID from `consort hub client list`")
                })?;
                consort_hub::admin::revoke_client(&data, id)
                    .map_err(one_line)
                    .context("could not remove the client")?
            } else {
                let id = id.trim().parse().with_context(|| {
                    format!("{id} is not a worker ID. Copy an ID from `consort hub worker list`")
                })?;
                consort_hub::admin::revoke_worker(&data, id)
                    .map_err(one_line)
                    .context("could not remove the worker")?
            };
            if removed.already {
                println!("The {what} {} was removed already.", removed.name);
            } else {
                println!("Removed the {what} {}.", removed.name);
                println!("A running hub closes its connection within a few seconds.");
            }
            if clients {
                println!("To pair a client again, run `consort hub setup-code`.");
            }
        }
    }
    Ok(())
}

/// How this program was installed. The systemd unit, the Dockerfile and the
/// service of a local hub set the variable.
fn this_install_kind() -> consort_proto::release::InstallKind {
    use consort_proto::release::InstallKind;
    InstallKind::from_env_value(std::env::var(InstallKind::ENV_VAR).ok().as_deref())
}

async fn run_hub_update(named: Option<String>) -> Result<()> {
    let current = consort_proto::release::Version::parse(env!("CARGO_PKG_VERSION"))
        .context("this program has no version that can be read")?;
    let asset = consort_proto::release::program_asset(std::env::consts::OS, std::env::consts::ARCH)
        .with_context(|| {
            format!(
                "no release has a program for {} on {}. Build Consort from source",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })?;
    let exe = std::env::current_exe().context("could not find this program")?;
    let host = consort_link::update::GithubHost::new(&format!("consort-hub/{current}"))?;
    let lines = hub_update::update(
        hub_update::Target {
            named,
            current,
            install: this_install_kind(),
            exe,
            asset,
            work: hub_update::work_folder(),
        },
        hub_update::Seams {
            host: &host,
            swap: &consort_link::update::RenameSwap,
            services: &hub_update::Systemd,
        },
    )
    .await?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

async fn run_hub(args: HubArgs) -> Result<()> {
    match args.command {
        Some(HubCommand::SetupCode { data }) => {
            let data = hub_data(data, args.data)?;
            let code =
                consort_hub::create_setup_code(&data).context("could not make a setup code")?;
            println!("Client setup code: {code}");
            println!("It works once and is valid for 10 minutes.");
            return Ok(());
        }
        Some(HubCommand::SessionHostCode { data }) => {
            let data = hub_data(data, args.data)?;
            let code = consort_hub::create_session_host_code(&data)
                .context("could not make a session host code")?;
            println!("Session host code: {code}");
            println!("It works once and is valid for 10 minutes.");
            return Ok(());
        }
        Some(HubCommand::Client { command }) => return run_paired(true, command, args.data),
        Some(HubCommand::Worker { command }) => return run_paired(false, command, args.data),
        Some(HubCommand::Update { version }) => return run_hub_update(version).await,
        Some(HubCommand::Reach { command, data }) => {
            let data = hub_data(data, args.data)?;
            let lines = match command {
                ReachCommand::Status => consort_hub::reach::cli::status(&data).await,
                ReachCommand::Tailscale => consort_hub::reach::cli::turn_on_tailscale(&data).await,
                ReachCommand::Off { only } => {
                    use consort_proto::client_api::ReachMethod;
                    let scope = only.map(|only| match only {
                        ReachOffScope::Tailscale => ReachMethod::Tailscale,
                        ReachOffScope::ThisNetwork => ReachMethod::ThisNetwork,
                    });
                    consort_hub::reach::cli::off(&data, scope).await
                }
            }?;
            for line in lines {
                println!("{line}");
            }
            return Ok(());
        }
        Some(HubCommand::Restore {
            backup,
            data,
            force,
        }) => {
            let data = hub_data(data, args.data)?;
            for line in run_restore(&backup, &data, force)? {
                println!("{line}");
            }
            return Ok(());
        }
        None => {}
    }
    let data = match args.data {
        Some(d) => d,
        None => default_hub_dir()?,
    };
    let hub = consort_hub::serve(consort_hub::HubConfig {
        data_dir: data.clone(),
        listen: args.listen,
        public_url: args.public_url,
        install_kind: this_install_kind(),
        dev: false,
        first_greeting: true,
    })
    .await
    .context("could not start the hub")?;
    println!(
        "The hub runs at {} (data in {}).",
        hub.public_url(),
        data.display()
    );
    if let Some(code) = hub.setup_code() {
        println!("No client is paired yet. Client setup code: {code}");
    }
    println!("Press Ctrl-C to stop.");
    wait_for_stop().await;
    hub.shutdown()
        .await
        .context("the hub did not stop cleanly")?;
    Ok(())
}

/// The exit code that says "start me again". The service plans restart a
/// worker that ends with an error.
const EXIT_RESTART: i32 = 75;

/// Starts the new program in place of this one. It uses the path from before
/// the swap: on Linux, the running program now has another name.
fn restart_program(exe: &Path) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let err = std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .exec();
        tracing::error!(error = %err, "could not start the new program");
    }
    #[cfg(not(unix))]
    let _ = exe;
    std::process::exit(EXIT_RESTART);
}

async fn run_worker(config_dir: PathBuf) -> Result<()> {
    let self_update = consort_worker::self_update::SelfUpdate::for_this_program();
    let exe = self_update.as_ref().map(|update| update.exe.clone());
    let worker = consort_worker::run(consort_worker::WorkerConfig {
        config_dir: config_dir.clone(),
        self_update,
    })
    .await
    .with_context(|| format!("could not start the worker in {}", config_dir.display()))?;
    println!("Worker {} runs. Press Ctrl-C to stop.", worker.worker_id());
    let restart = tokio::select! {
        () = wait_for_stop() => false,
        () = worker.restart_requested() => true,
    };
    worker.shutdown().await?;
    if restart && let Some(exe) = exe {
        restart_program(&exe);
    }
    Ok(())
}

async fn run_session_host(config_dir: PathBuf, consort_home: Option<PathBuf>) -> Result<()> {
    let host = consort_session_host::run(HostConfig {
        consort_home,
        ..HostConfig::new(config_dir.clone())
    })
    .await
    .with_context(|| {
        format!(
            "could not start the session host in {}",
            config_dir.display()
        )
    })?;
    println!("The session host runs. Press Ctrl-C to stop.");
    tracing::info!(id = %host.worker_id(), "the session host runs");
    wait_for_stop().await;
    host.shutdown().await?;
    Ok(())
}

/// Pairs the dev worker with the dev hub, unless it is paired with it
/// already.
async fn pair_dev_worker(hub: &consort_hub::HubHandle, dir: &Path) -> Result<()> {
    let toml = consort_worker::config::WorkerToml::load(dir)?;
    let url = consort_link::http::base_url(&hub.public_url());
    if toml.hub_url.as_deref() == Some(url.as_str())
        && toml.worker_id.is_some()
        && dir.join(consort_worker::config::KEY_FILE).exists()
    {
        return Ok(());
    }
    let code = hub
        .new_worker_pairing_code()
        .await
        .context("the hub could not make a pairing code")?;
    let id = consort_worker::pair(
        &url,
        &code,
        dir,
        Some("hub".to_owned()),
        vec!["hub".to_owned()],
    )
    .await
    .context("could not pair the dev worker")?;
    tracing::info!(worker = %id, "paired the dev worker");
    Ok(())
}

/// Pairs the dev session host with the dev hub, unless it is paired with it
/// already.
async fn pair_dev_session_host(hub: &consort_hub::HubHandle, dir: &Path) -> Result<()> {
    let toml = HostToml::load(dir)?;
    let url = consort_link::http::base_url(&hub.public_url());
    if toml.hub_url.as_deref() == Some(url.as_str())
        && toml.worker_id.is_some()
        && dir.join(consort_session_host::config::KEY_FILE).exists()
    {
        return Ok(());
    }
    let code = hub
        .new_session_host_pairing_code()
        .await
        .context("the hub could not make a session host code")?;
    consort_session_host::pair(&url, &code, dir, Some("hub".to_owned()), Vec::new())
        .await
        .context("could not pair the session host")?;
    Ok(())
}

/// Starts the session host as its own process, as a real hub machine runs it.
/// A hub restart then does not stop a turn.
fn spawn_session_host(dir: &Path, consort_home: &Path) -> Result<tokio::process::Child> {
    let program = std::env::current_exe().context("could not find this program")?;
    tokio::process::Command::new(program)
        .args(["session-host", "run", "--config-dir"])
        .arg(dir)
        .arg("--consort-home")
        .arg(consort_home)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("could not start the session host")
}

/// Stops the session host, which closes its harness processes. After 10
/// seconds, it kills the process.
async fn stop_session_host(mut child: tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("kill")
            .args(["-s", "TERM", &pid.to_string()])
            .status()
            .await;
    }
    if tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

/// The accounts of an older dev hub were on its worker. There is no migration.
fn refuse_old_dev_folder(data: &Path) -> Result<()> {
    if data.join("hub.db").exists() && !data.join("session-host").exists() {
        bail!(
            "{} is from before the hub owned the accounts. Delete this folder to start again with a new hub",
            data.display()
        );
    }
    Ok(())
}

/// The Consort home of `consort dev`: `CONSORT_HOME` when it is set, else
/// `<data>/home`, so a dev run keeps its profiles and tools apart from the
/// user's own `~/.consort`.
fn dev_consort_home(data: &Path, env: Option<std::ffi::OsString>) -> PathBuf {
    env.filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("home"))
}

async fn run_dev(args: DevArgs) -> Result<()> {
    refuse_old_dev_folder(&args.data)?;
    let hub = consort_hub::serve(consort_hub::HubConfig {
        data_dir: args.data.clone(),
        listen: args.listen,
        public_url: None,
        install_kind: consort_proto::release::InstallKind::Manual,
        dev: true,
        first_greeting: true,
    })
    .await
    .context("could not start the hub")?;
    let host_dir = args.data.join("session-host");
    pair_dev_session_host(&hub, &host_dir).await?;
    let consort_home = dev_consort_home(&args.data, std::env::var_os("CONSORT_HOME"));
    let session_host = spawn_session_host(&host_dir, &consort_home)?;
    let worker_dir = args.data.join("worker");
    pair_dev_worker(&hub, &worker_dir).await?;
    let worker = consort_worker::run(consort_worker::WorkerConfig {
        config_dir: worker_dir.clone(),
        // A development worker keeps its program.
        self_update: None,
    })
    .await?;
    if !worker.wait_connected(Duration::from_secs(10)).await {
        tracing::warn!("the dev worker did not connect within 10 seconds; it keeps trying");
    }
    let setup = match hub.setup_code() {
        Some(code) => code,
        None => {
            consort_hub::create_setup_code(&args.data).context("could not make a setup code")?
        }
    };
    println!("Hub URL: {}", hub.public_url());
    println!("Client setup code: {setup}");
    println!(
        "Worker {} (tag `hub`) uses {}.",
        worker.worker_id(),
        worker_dir.display()
    );
    println!(
        "The session host uses {}. Profile folders and managed tools are in {}.",
        host_dir.display(),
        consort_home.display()
    );
    println!("Press Ctrl-C to stop.");
    wait_for_stop().await;
    stop_session_host(session_host).await;
    worker.shutdown().await?;
    hub.shutdown()
        .await
        .context("the hub did not stop cleanly")?;
    Ok(())
}

async fn run_local(command: local::LocalCommand) -> Result<()> {
    use local::LocalCommand as C;
    match command {
        C::Install { dry_run, progress } => local::install(dry_run, progress),
        C::Status => local::status(),
        C::Uninstall {
            delete_data,
            dry_run,
        } => local::uninstall(delete_data, dry_run).await,
        C::Run { .. } => local::run().await,
        C::SetupCode => local::setup_code(),
        C::Check => {
            println!("{}", serde_json::to_string_pretty(&local::check())?);
            Ok(())
        }
        C::Start => local::start(),
        C::Stop => local::stop(),
        C::Login { state } => local::login(state),
        C::Upgrade { program } => local::upgrade(&program),
    }
}

async fn run_bridge(url: &str) -> Result<()> {
    let Ok(token) = std::env::var(consort_session_host::bridge::TOKEN_VAR) else {
        bail!(
            "set {} to the session's MCP token",
            consort_session_host::bridge::TOKEN_VAR
        );
    };
    consort_session_host::bridge::run_stdio(url, &token).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let log_file = match &cli.command {
        Command::Worker {
            command: WorkerCommand::Run { log_file, .. },
        }
        | Command::SessionHost {
            command: SessionHostCommand::Run { log_file, .. },
        } => log_file.as_deref(),
        Command::Local { command } => local::log_file_of(command),
        _ => None,
    };
    init_logging(log_file)?;
    match cli.command {
        Command::Hub(args) => run_hub(args).await,
        Command::Worker { command } => match command {
            WorkerCommand::Pair {
                hub_url,
                code,
                tls_sha256,
                name,
                tags,
                config_dir,
            } => {
                let dir = worker_dir(config_dir)?;
                if let Some(warning) = consort_link::http::cleartext_warning(&hub_url) {
                    eprintln!("Warning: {warning}");
                }
                let code = pairing_code(code, &mut std::io::stdin().lock())?;
                let id = consort_worker::pair_pinned(&hub_url, tls_sha256, &code, &dir, name, tags)
                    .await?;
                println!(
                    "Paired as worker {id}. The configuration is in {}.",
                    dir.display()
                );
                println!(
                    "Start it with: consort worker run --config-dir {}",
                    dir.display()
                );
                Ok(())
            }
            WorkerCommand::Run { config_dir, .. } => run_worker(worker_dir(config_dir)?).await,
            WorkerCommand::Install(args) => {
                let dir = worker_dir(args.config_dir.clone())?;
                service_cmd::install(args, dir, &mut std::io::stdin().lock()).await
            }
            WorkerCommand::Uninstall(args) => {
                let dir = worker_dir(args.config_dir.clone())?;
                service_cmd::uninstall(args, &dir)
            }
            WorkerCommand::Status(args) => {
                let dir = worker_dir(args.config_dir.clone())?;
                service_cmd::status(args, &dir)
            }
        },
        Command::SessionHost { command } => match command {
            SessionHostCommand::Pair {
                hub_url,
                code,
                config_dir,
            } => {
                let code = pairing_code(code, &mut std::io::stdin().lock())?;
                let id = consort_session_host::pair(&hub_url, &code, &config_dir, None, Vec::new())
                    .await?;
                println!(
                    "Paired the session host as {id}. The configuration is in {}.",
                    config_dir.display()
                );
                Ok(())
            }
            SessionHostCommand::Run {
                config_dir,
                consort_home,
                ..
            } => run_session_host(config_dir, consort_home).await,
        },
        Command::Local { command } => run_local(command).await,
        Command::Dev(args) => run_dev(args).await,
        #[cfg(unix)]
        Command::BoundaryRun { bridges, command } => run_boundary(&bridges, &command).await,
        Command::McpBridge { url } => run_bridge(&url).await,
        Command::ReleaseInfo => {
            let manifest =
                consort_proto::release::ReleaseManifest::of_this_build(env!("CARGO_PKG_VERSION"));
            println!("{}", serde_json::to_string_pretty(&manifest)?);
            Ok(())
        }
    }
}

/// Runs the harness inside the Linux boundary. This process ends with its code.
#[cfg(unix)]
async fn run_boundary(bridges: &[String], command: &[String]) -> Result<()> {
    let mut parsed = Vec::with_capacity(bridges.len());
    for bridge in bridges {
        let (port, socket) = bridge
            .split_once('=')
            .with_context(|| format!("`{bridge}` is not <port>=<socket>"))?;
        let port: u16 = port
            .parse()
            .with_context(|| format!("`{port}` is not a port"))?;
        parsed.push((port, std::path::PathBuf::from(socket)));
    }
    let (program, args) = command.split_first().context("no program to run")?;
    let code = consort_harness::boundary::run_inside(&parsed, program, args).await;
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_log_filter_never_lets_the_websocket_library_log_frames() {
        let filter = log_filter(Ok(tracing_subscriber::EnvFilter::new("trace"))).unwrap();
        let text = filter.to_string();
        assert!(text.contains("tungstenite=info"), "{text}");
        assert!(text.contains("tokio_tungstenite=info"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn hub_update_takes_a_named_version_or_none() {
        let cli = Cli::try_parse_from(["consort", "hub", "update", "--version", "0.8.0"]).unwrap();
        let Command::Hub(HubArgs {
            command: Some(HubCommand::Update { version }),
            ..
        }) = cli.command
        else {
            panic!("not hub update");
        };
        assert_eq!(version.as_deref(), Some("0.8.0"));
        let cli = Cli::try_parse_from(["consort", "hub", "update"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hub(HubArgs {
                command: Some(HubCommand::Update { version: None }),
                ..
            })
        ));
    }

    #[test]
    fn the_boundary_helper_takes_its_bridges_and_its_command() {
        let cli = Cli::try_parse_from([
            "consort",
            consort_harness::boundary::INSIDE_SUBCOMMAND,
            "--bridge",
            "5001=/run/consort/5001.sock",
            "--",
            "/usr/bin/node",
            "--flag",
        ])
        .unwrap();
        let Command::BoundaryRun { bridges, command } = cli.command else {
            panic!("not boundary-run");
        };
        assert_eq!(bridges, ["5001=/run/consort/5001.sock"]);
        assert_eq!(command, ["/usr/bin/node", "--flag"]);
    }

    #[test]
    fn cli_parses() {
        Cli::command().debug_assert();
        let cli = Cli::try_parse_from([
            "consort",
            "worker",
            "pair",
            "http://h:7600",
            "--code",
            "ABCD-EFGH",
            "--tag",
            "a",
            "--label",
            "b",
        ])
        .unwrap();
        let Command::Worker {
            command: WorkerCommand::Pair { tags, code, .. },
        } = cli.command
        else {
            panic!("expected worker pair")
        };
        assert_eq!(tags, vec!["a", "b"]);
        assert_eq!(code.as_deref(), Some("ABCD-EFGH"));
        // The code can come from the environment or a prompt instead.
        let cli = Cli::try_parse_from(["consort", "worker", "pair", "http://h:7600"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Worker {
                command: WorkerCommand::Pair { .. }
            }
        ));
        let cli =
            Cli::try_parse_from(["consort", "hub", "setup-code", "--data", "/tmp/x"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hub(HubArgs {
                command: Some(HubCommand::SetupCode { .. }),
                ..
            })
        ));
        let cli =
            Cli::try_parse_from(["consort", "hub", "client", "list", "--data", "/tmp/x"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hub(HubArgs {
                command: Some(HubCommand::Client {
                    command: PairedCommand::List { .. }
                }),
                ..
            })
        ));
        let cli = Cli::try_parse_from(["consort", "hub", "worker", "revoke", "wrk_1"]).unwrap();
        let Command::Hub(HubArgs {
            command:
                Some(HubCommand::Worker {
                    command: PairedCommand::Revoke { id, .. },
                }),
            ..
        }) = cli.command
        else {
            panic!("expected hub worker revoke")
        };
        assert_eq!(id, "wrk_1");
        assert!(
            Cli::try_parse_from(["consort", "hub", "client", "revoke"]).is_err(),
            "revoke needs an ID"
        );
        let cli = Cli::try_parse_from(["consort", "hub"]).unwrap();
        let Command::Hub(args) = cli.command else {
            panic!("expected hub")
        };
        assert_eq!(args.listen.to_string(), DEFAULT_LISTEN);
        assert!(
            Cli::try_parse_from(["consort", "dev"]).is_err(),
            "dev needs --data"
        );
    }

    #[test]
    fn the_service_commands_parse() {
        let cli = Cli::try_parse_from([
            "consort",
            "worker",
            "install",
            "--hub",
            "https://h.example.com",
            "--code",
            "ABCD-EFGH",
            "--tag",
            "gpu",
            "--dry-run",
        ])
        .unwrap();
        let Command::Worker {
            command: WorkerCommand::Install(args),
        } = cli.command
        else {
            panic!("expected worker install")
        };
        assert_eq!(args.hub, "https://h.example.com");
        assert_eq!(args.code.as_deref(), Some("ABCD-EFGH"));
        assert_eq!(args.tags, vec!["gpu"]);
        assert!(args.dry_run && !args.linger);
        assert!(
            Cli::try_parse_from(["consort", "worker", "install"]).is_err(),
            "install needs --hub"
        );
        for name in ["uninstall", "status"] {
            assert!(Cli::try_parse_from(["consort", "worker", name, "--dry-run"]).is_ok());
        }
        let cli =
            Cli::try_parse_from(["consort", "worker", "run", "--log-file", "/tmp/w.log"]).unwrap();
        let Command::Worker {
            command: WorkerCommand::Run { log_file, .. },
        } = cli.command
        else {
            panic!("expected worker run")
        };
        assert_eq!(log_file, Some(PathBuf::from("/tmp/w.log")));
    }

    #[test]
    fn the_local_commands_parse() {
        let cli = Cli::try_parse_from(["consort", "local", "install", "--dry-run"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Local {
                command: local::LocalCommand::Install {
                    dry_run: true,
                    progress: false
                }
            }
        ));
        let cli = Cli::try_parse_from(["consort", "local", "uninstall", "--delete-data"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Local {
                command: local::LocalCommand::Uninstall {
                    delete_data: true,
                    dry_run: false
                }
            }
        ));
        for args in [
            vec!["status"],
            vec!["setup-code"],
            vec!["check"],
            vec!["start"],
            vec!["stop"],
            vec!["login", "off"],
            vec!["run", "--log-file", "/tmp/l.log"],
            vec!["upgrade", "--program", "/tmp/consort"],
        ] {
            let mut line = vec!["consort", "local"];
            line.extend(args.iter());
            assert!(Cli::try_parse_from(&line).is_ok(), "{line:?}");
        }
        assert!(Cli::try_parse_from(["consort", "local", "upgrade"]).is_err());
    }

    #[test]
    fn restore_takes_a_file_a_folder_and_force() {
        let cli = Cli::try_parse_from([
            "consort",
            "hub",
            "restore",
            "/srv/backups/consort-2026-10-29-0300.tar",
            "--data",
            "/var/lib/consort",
            "--force",
        ])
        .unwrap();
        let Command::Hub(HubArgs {
            command:
                Some(HubCommand::Restore {
                    backup,
                    data,
                    force,
                }),
            ..
        }) = cli.command
        else {
            panic!("expected hub restore")
        };
        assert_eq!(
            backup,
            PathBuf::from("/srv/backups/consort-2026-10-29-0300.tar")
        );
        assert_eq!(data, Some(PathBuf::from("/var/lib/consort")));
        assert!(force);
        assert!(
            Cli::try_parse_from(["consort", "hub", "restore"]).is_err(),
            "restore needs a file"
        );
    }

    #[test]
    fn restore_says_what_is_wrong_with_a_file_that_is_not_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.tar");
        std::fs::write(&file, b"plain text").unwrap();
        let err = run_restore(&file, &dir.path().join("data"), false).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("is not a Consort backup"), "{text}");
        assert!(text.contains("consort-<date>.tar"), "{text}");
    }

    #[test]
    fn dev_uses_a_home_inside_the_data_folder() {
        let data = Path::new("/tmp/dev-data");
        assert_eq!(dev_consort_home(data, None), data.join("home"));
        assert_eq!(
            dev_consort_home(data, Some("".into())),
            data.join("home"),
            "an empty variable counts as not set"
        );
        assert_eq!(
            dev_consort_home(data, Some("/custom".into())),
            PathBuf::from("/custom")
        );
    }

    #[test]
    fn the_pairing_code_comes_from_the_flag_or_a_prompt() {
        let mut empty = std::io::Cursor::new(Vec::new());
        assert_eq!(
            pairing_code(Some(" ABCD-EFGH ".into()), &mut empty).unwrap(),
            "ABCD-EFGH"
        );
        let mut typed = std::io::Cursor::new(b"wxyz-1234\n".to_vec());
        assert_eq!(pairing_code(None, &mut typed).unwrap(), "wxyz-1234");
        let mut nothing = std::io::Cursor::new(b"\n".to_vec());
        assert!(pairing_code(None, &mut nothing).is_err());
    }

    /// SIGHUP and SIGTERM end the wait, so the worker runs its shutdown.
    #[cfg(unix)]
    #[tokio::test]
    async fn stop_signals_end_the_wait() {
        for signal in ["HUP", "TERM"] {
            let signals = StopSignals::listen();
            let wait = tokio::spawn(signals.wait());
            tokio::time::sleep(Duration::from_millis(50)).await;
            let status = std::process::Command::new("kill")
                .args(["-s", signal, &std::process::id().to_string()])
                .status()
                .unwrap();
            assert!(status.success());
            tokio::time::timeout(Duration::from_secs(5), wait)
                .await
                .unwrap_or_else(|_| panic!("SIG{signal} did not end the wait"))
                .unwrap();
        }
    }
}
