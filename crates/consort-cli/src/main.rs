//! The `consort` program: the hub, the worker, a dev mode that runs both,
//! and the stdio MCP bridge.

use std::io::BufRead;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use consort_worker::WorkerConfig;
use consort_worker::config::WorkerToml;

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
    /// Run a hub and a worker in one process, for development. The worker
    /// keeps its profile folders and managed tools in <data>/home, unless
    /// CONSORT_HOME is set.
    Dev(DevArgs),
    /// Bridge stdio to a session's local MCP endpoint. The token comes from
    /// CONSORT_MCP_TOKEN.
    McpBridge {
        /// The endpoint URL, `http://127.0.0.1:<port>/mcp/<session_id>`.
        #[arg(long)]
        url: String,
    },
}

#[derive(Args)]
struct HubArgs {
    #[command(subcommand)]
    command: Option<HubCommand>,
    /// The data folder, with hub.db and blobs/.
    #[arg(long)]
    data: Option<PathBuf>,
    /// The address to listen on.
    #[arg(long, default_value = DEFAULT_LISTEN)]
    listen: SocketAddr,
    /// The URL that clients and workers use, when it is not http://<listen>.
    #[arg(long)]
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

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // Logs go to stderr, so stdout stays free for mcp-bridge.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn default_hub_dir() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "", "consort")
        .map(|d| d.data_dir().join("hub"))
        .context("could not find a data folder; pass --data")
}

fn worker_dir(given: Option<PathBuf>) -> Result<PathBuf> {
    match given {
        Some(dir) => Ok(dir),
        None => {
            WorkerConfig::default_dir().context("could not find a config folder; pass --config-dir")
        }
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
        Some(HubCommand::Client { command }) => return run_paired(true, command, args.data),
        Some(HubCommand::Worker { command }) => return run_paired(false, command, args.data),
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

async fn run_worker(config_dir: PathBuf) -> Result<()> {
    let worker = consort_worker::run(WorkerConfig::new(config_dir.clone()))
        .await
        .with_context(|| format!("could not start the worker in {}", config_dir.display()))?;
    println!("Worker {} runs. Press Ctrl-C to stop.", worker.worker_id());
    wait_for_stop().await;
    worker.shutdown().await?;
    Ok(())
}

/// Pairs the dev worker with the dev hub, unless it is paired with it
/// already.
async fn pair_dev_worker(hub: &consort_hub::HubHandle, dir: &Path) -> Result<()> {
    let toml = WorkerToml::load(dir)?;
    let url = consort_worker::http::base_url(&hub.public_url());
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

/// The Consort home of `consort dev`: `CONSORT_HOME` when it is set, else
/// `<data>/home`, so a dev run keeps its profiles and tools apart from the
/// user's own `~/.consort`.
fn dev_consort_home(data: &Path, env: Option<std::ffi::OsString>) -> PathBuf {
    env.filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("home"))
}

async fn run_dev(args: DevArgs) -> Result<()> {
    let hub = consort_hub::serve(consort_hub::HubConfig {
        data_dir: args.data.clone(),
        listen: args.listen,
        public_url: None,
    })
    .await
    .context("could not start the hub")?;
    let worker_dir = args.data.join("worker");
    pair_dev_worker(&hub, &worker_dir).await?;
    let consort_home = dev_consort_home(&args.data, std::env::var_os("CONSORT_HOME"));
    let worker = consort_worker::run(WorkerConfig {
        config_dir: worker_dir.clone(),
        consort_home: Some(consort_home.clone()),
        check_tool_updates: true,
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
        "Profile folders and managed tools are in {}.",
        consort_home.display()
    );
    println!("Press Ctrl-C to stop.");
    wait_for_stop().await;
    worker.shutdown().await?;
    hub.shutdown()
        .await
        .context("the hub did not stop cleanly")?;
    Ok(())
}

async fn run_bridge(url: &str) -> Result<()> {
    let Ok(token) = std::env::var(consort_worker::bridge::TOKEN_VAR) else {
        bail!(
            "set {} to the session's MCP token",
            consort_worker::bridge::TOKEN_VAR
        );
    };
    consort_worker::bridge::run_stdio(url, &token).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging();
    match cli.command {
        Command::Hub(args) => run_hub(args).await,
        Command::Worker { command } => match command {
            WorkerCommand::Pair {
                hub_url,
                code,
                name,
                tags,
                config_dir,
            } => {
                let dir = worker_dir(config_dir)?;
                if let Some(warning) = consort_worker::http::cleartext_warning(&hub_url) {
                    eprintln!("Warning: {warning}");
                }
                let code = pairing_code(code, &mut std::io::stdin().lock())?;
                let id = consort_worker::pair(&hub_url, &code, &dir, name, tags).await?;
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
            WorkerCommand::Run { config_dir } => run_worker(worker_dir(config_dir)?).await,
        },
        Command::Dev(args) => run_dev(args).await,
        Command::McpBridge { url } => run_bridge(&url).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

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
