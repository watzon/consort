//! `consort worker install`, `uninstall` and `status`: the commands around
//! the service text in [`crate::service`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use consort_worker::config::{KEY_FILE, WorkerToml};

use crate::service::{self, Platform, Runner, ServiceEnv, SystemRunner};

/// The options of `consort worker install`.
#[derive(Args)]
pub struct InstallArgs {
    /// The hub URL, for example https://hub.example.com.
    #[arg(long)]
    pub hub: String,
    /// The pairing code from a client, for example ABCD-EFGH. Without it,
    /// the code comes from CONSORT_PAIRING_CODE, or from a prompt. A machine
    /// that is paired with this hub already needs no code.
    #[arg(long, env = "CONSORT_PAIRING_CODE", hide_env_values = true)]
    pub code: Option<String>,
    /// The worker name. The default is the host name.
    #[arg(long)]
    pub name: Option<String>,
    /// A tag for placement, such as gpu. Repeat it for more tags.
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// The worker folder.
    #[arg(long, env = "CONSORT_WORKER_DIR")]
    pub config_dir: Option<PathBuf>,
    /// The consort program that the service runs. The default is this
    /// program.
    #[arg(long)]
    pub exe: Option<PathBuf>,
    /// Print what the install would write and run. Do not pair, write or run
    /// anything.
    #[arg(long)]
    pub dry_run: bool,
    /// On Linux, also run `loginctl enable-linger`, so the worker starts at
    /// boot and not only when you sign in. It may ask for a password.
    #[arg(long)]
    pub linger: bool,
}

/// The options of `consort worker uninstall` and `consort worker status`.
#[derive(Args)]
pub struct ServiceArgs {
    /// The worker folder.
    #[arg(long, env = "CONSORT_WORKER_DIR")]
    pub config_dir: Option<PathBuf>,
    /// Print what the command would write and run. Do not run anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Whether this machine is paired already.
#[derive(Debug, PartialEq, Eq)]
pub enum PairedWith {
    /// With the hub that the user named.
    ThisHub,
    /// With another hub.
    OtherHub(String),
    /// With no hub.
    Nobody,
}

/// Reads the pairing from the worker folder. The key file must exist too.
pub fn paired_with(toml: &WorkerToml, key_exists: bool, hub_url: &str) -> PairedWith {
    match (&toml.hub_url, &toml.worker_id) {
        (Some(url), Some(_)) if key_exists => {
            if consort_worker::http::base_url(url) == consort_worker::http::base_url(hub_url) {
                PairedWith::ThisHub
            } else {
                PairedWith::OtherHub(url.clone())
            }
        }
        _ => PairedWith::Nobody,
    }
}

fn platform() -> Result<Platform> {
    Platform::current().with_context(|| {
        format!(
            "{} has no worker service. Run `consort worker run` yourself",
            std::env::consts::OS
        )
    })
}

fn program_path(given: Option<PathBuf>) -> Result<PathBuf> {
    let exe = match given {
        Some(exe) => exe,
        None => std::env::current_exe().context("could not find the consort program")?,
    };
    // The service starts the same file after an update, so it needs a stable
    // absolute path.
    std::fs::canonicalize(&exe)
        .or_else(|_| {
            if exe.is_absolute() {
                Ok(exe.clone())
            } else {
                std::env::current_dir().map(|cwd| cwd.join(&exe))
            }
        })
        .with_context(|| format!("could not find {}", exe.display()))
}

/// Takes `\\?\` off a Windows path. Task Scheduler and PowerShell do not
/// take the prefix.
fn plain_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

pub async fn install(
    args: InstallArgs,
    dir: PathBuf,
    input: &mut dyn std::io::BufRead,
) -> Result<()> {
    let platform = platform()?;
    let exe = plain_path(program_path(args.exe)?);
    let env = ServiceEnv::detect(&exe, args.config_dir.as_deref())?;
    let plan = service::plan_install(platform, &env)?;
    let toml = WorkerToml::load(&dir)?;
    let state = paired_with(&toml, dir.join(KEY_FILE).exists(), &args.hub);
    if args.dry_run {
        match state {
            PairedWith::ThisHub if args.code.is_none() => println!(
                "Dry run. This machine is paired with {} already, so the install would not pair it again.",
                args.hub
            ),
            _ => println!(
                "Dry run. The install would pair this machine with {} and use up the pairing code. This dry run does not.",
                args.hub
            ),
        }
        print!("{}", service::describe(&plan, "install"));
        return Ok(());
    }
    if let Some(warning) = consort_worker::http::cleartext_warning(&args.hub) {
        eprintln!("Warning: {warning}");
    }
    match state {
        PairedWith::ThisHub if args.code.is_none() => {
            println!("This machine is paired with {} already.", args.hub);
        }
        PairedWith::OtherHub(other) if args.code.is_none() => bail!(
            "this machine is paired with {other}. Pass --code from the hub {} to pair it again",
            args.hub
        ),
        _ => {
            let code = crate::pairing_code(args.code, input)?;
            let id = consort_worker::pair(&args.hub, &code, &dir, args.name, args.tags).await?;
            println!(
                "Paired as worker {id}. The configuration is in {}.",
                dir.display()
            );
        }
    }
    service::apply(&plan, &mut SystemRunner)?;
    if platform == Platform::Linux && args.linger {
        let user = std::env::var("USER").unwrap_or_default();
        let linger = service::PlannedCommand {
            program: "loginctl".to_owned(),
            args: vec!["enable-linger".to_owned(), user],
            may_fail: false,
        };
        let outcome = SystemRunner.run(&linger)?;
        if !outcome.success {
            eprintln!(
                "Warning: `loginctl enable-linger` failed: {}. The worker starts when you sign in.",
                outcome.output.trim()
            );
        }
    }
    println!(
        "Installed {}. The worker starts when you sign in, and again after a failure.",
        platform.manager()
    );
    println!("The worker writes its log to {}.", plan.log_file);
    println!("Check it with `consort worker status`. Remove it with `consort worker uninstall`.");
    if platform == Platform::Linux && !args.linger {
        println!(
            "On a server, run `consort worker install --linger` too, so the worker starts at boot without a sign-in."
        );
    }
    Ok(())
}

pub fn uninstall(args: ServiceArgs, dir: &Path) -> Result<()> {
    let platform = platform()?;
    let exe = plain_path(program_path(None)?);
    let env = ServiceEnv::detect(&exe, args.config_dir.as_deref())?;
    let plan = service::plan_uninstall(platform, &env)?;
    if args.dry_run {
        print!("{}", service::describe(&plan, "remove"));
        return Ok(());
    }
    service::apply(&plan, &mut SystemRunner)?;
    println!("Removed {}.", platform.manager());
    println!(
        "The worker folder {} and the log {} stay. Delete them by hand to remove all traces.",
        dir.display(),
        plan.log_file
    );
    Ok(())
}

pub fn status(args: ServiceArgs, dir: &Path) -> Result<()> {
    let platform = platform()?;
    let exe = plain_path(program_path(None)?);
    let env = ServiceEnv::detect(&exe, args.config_dir.as_deref())?;
    let plan = service::plan_status(platform, &env)?;
    if args.dry_run {
        print!("{}", service::describe(&plan, "check"));
        return Ok(());
    }
    let mut results = Vec::new();
    for command in &plan.commands {
        let outcome = SystemRunner.run(command)?;
        results.push((command.clone(), outcome));
    }
    println!("{}", service::status_summary(platform, &results));
    let toml = WorkerToml::load(dir)?;
    match (&toml.hub_url, &toml.worker_id) {
        (Some(url), Some(id)) => println!("The worker {id} is paired with {url}."),
        _ => println!("This machine is not paired with a hub."),
    }
    let installed = Path::new(&service::definition_path(platform, &env)?).exists();
    if !installed {
        println!("The service is not installed. Run `consort worker install`.");
    }
    println!("The log is {}.", plan.log_file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::WorkerId;

    fn toml(hub: Option<&str>, id: bool) -> WorkerToml {
        WorkerToml {
            hub_url: hub.map(str::to_owned),
            worker_id: id.then(WorkerId::new),
            ..WorkerToml::default()
        }
    }

    #[test]
    fn pairing_state_needs_the_hub_the_id_and_the_key() {
        let t = toml(Some("https://hub.example.com"), true);
        assert_eq!(
            paired_with(&t, true, "https://hub.example.com/"),
            PairedWith::ThisHub
        );
        assert_eq!(
            paired_with(&t, true, "https://other.example.com"),
            PairedWith::OtherHub("https://hub.example.com".into())
        );
        assert_eq!(
            paired_with(&t, false, "https://hub.example.com"),
            PairedWith::Nobody
        );
        assert_eq!(
            paired_with(
                &toml(Some("https://hub.example.com"), false),
                true,
                "https://hub.example.com"
            ),
            PairedWith::Nobody
        );
        assert_eq!(
            paired_with(&toml(None, false), true, "x"),
            PairedWith::Nobody
        );
    }

    #[test]
    fn the_windows_extended_prefix_goes_away() {
        assert_eq!(
            plain_path(PathBuf::from(r"\\?\C:\Users\ann\consort.exe")),
            PathBuf::from(r"C:\Users\ann\consort.exe")
        );
        assert_eq!(
            plain_path(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
        assert_eq!(
            plain_path(PathBuf::from("/usr/bin/consort")),
            PathBuf::from("/usr/bin/consort")
        );
    }
}
