//! The `consort` program: the hub, the worker, a dev mode that runs both,
//! and the stdio MCP bridge.

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
    /// Run a hub and a worker in one process, for development.
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
}

#[derive(Subcommand)]
enum WorkerCommand {
    /// Pair this machine with a hub.
    Pair {
        /// The hub URL, for example https://hub.example.com.
        hub_url: String,
        /// The pairing code from a client, for example ABCD-EFGH.
        #[arg(long)]
        code: String,
        /// The worker name. The default is the host name.
        #[arg(long)]
        name: Option<String>,
        /// A label for placement. Repeat it for more labels.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// The worker folder.
        #[arg(long, env = "CONSORT_WORKER_DIR")]
        config_dir: Option<PathBuf>,
    },
    /// Run the worker until Ctrl-C.
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

async fn wait_for_ctrl_c() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %err, "could not listen for Ctrl-C");
        std::future::pending::<()>().await;
    }
}

async fn run_hub(args: HubArgs) -> Result<()> {
    if let Some(HubCommand::SetupCode { data }) = args.command {
        let data = match data.or(args.data) {
            Some(d) => d,
            None => default_hub_dir()?,
        };
        let code = consort_hub::create_setup_code(&data).context("could not make a setup code")?;
        println!("Client setup code: {code}");
        println!("It works once and is valid for 10 minutes.");
        return Ok(());
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
    wait_for_ctrl_c().await;
    hub.shutdown()
        .await
        .context("the hub did not stop cleanly")?;
    Ok(())
}

async fn run_worker(config_dir: PathBuf) -> Result<()> {
    let worker = consort_worker::run(WorkerConfig {
        config_dir: config_dir.clone(),
    })
    .await
    .with_context(|| format!("could not start the worker in {}", config_dir.display()))?;
    println!("Worker {} runs. Press Ctrl-C to stop.", worker.worker_id());
    wait_for_ctrl_c().await;
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
    let worker = consort_worker::run(WorkerConfig {
        config_dir: worker_dir.clone(),
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
        "Worker {} (label `hub`) uses {}.",
        worker.worker_id(),
        worker_dir.display()
    );
    println!("Press Ctrl-C to stop.");
    wait_for_ctrl_c().await;
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
                labels,
                config_dir,
            } => {
                let dir = worker_dir(config_dir)?;
                let id = consort_worker::pair(&hub_url, &code, &dir, name, labels).await?;
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
            "--label",
            "a",
            "--label",
            "b",
        ])
        .unwrap();
        let Command::Worker {
            command: WorkerCommand::Pair { labels, code, .. },
        } = cli.command
        else {
            panic!("expected worker pair")
        };
        assert_eq!(labels, vec!["a", "b"]);
        assert_eq!(code, "ABCD-EFGH");
        let cli =
            Cli::try_parse_from(["consort", "hub", "setup-code", "--data", "/tmp/x"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hub(HubArgs {
                command: Some(HubCommand::SetupCode { .. }),
                ..
            })
        ));
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
}
