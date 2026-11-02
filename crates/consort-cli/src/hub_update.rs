//! `consort hub update`: replaces the `consort` program of a server install
//! with a release of the fixed repository, after a check of its SHA-256.
//!
//! The network, the file replacement and the service manager are seams, so
//! that tests need none of them.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use consort_link::update::{
    ProgramSwap, ReleaseHost, fetch_program, is_build_or_bundle, latest_version, remove_old_program,
};
use consort_proto::release::{InstallKind, Version};

/// The systemd services that run this program on a hub machine.
const SERVICES: [&str; 2] = ["consort-hub", "consort-session-host"];

/// What this command asks of the service manager.
pub trait ServiceManager {
    /// True when this process may restart services: it runs as root, and the
    /// system has systemd.
    fn can_restart(&self) -> bool;
    /// True when the service runs now.
    fn is_active(&self, service: &str) -> bool;
    /// Restarts the service.
    fn restart(&self, service: &str) -> Result<()>;
}

/// `systemctl`, and `id` to see whether this process is root.
pub struct Systemd;

impl ServiceManager for Systemd {
    fn can_restart(&self) -> bool {
        let root = std::process::Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0");
        root && std::process::Command::new("systemctl")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn is_active(&self, service: &str) -> bool {
        std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", service])
            .status()
            .is_ok_and(|status| status.success())
    }

    fn restart(&self, service: &str) -> Result<()> {
        let status = std::process::Command::new("systemctl")
            .args(["restart", service])
            .status()
            .context("could not run systemctl")?;
        if !status.success() {
            bail!(
                "systemctl could not restart {service}. Run `journalctl -u {service}` to see why"
            );
        }
        Ok(())
    }
}

/// What the command works on.
pub struct Target {
    /// The version that the user named. `None` means the latest release.
    pub named: Option<String>,
    /// The version of this program.
    pub current: Version,
    /// How this hub was installed.
    pub install: InstallKind,
    /// The program to replace.
    pub exe: PathBuf,
    /// The release file for this system.
    pub asset: String,
    /// A folder for the download. It must not exist yet.
    pub work: PathBuf,
}

/// The seams of the command.
pub struct Seams<'a> {
    pub host: &'a dyn ReleaseHost,
    pub swap: &'a dyn ProgramSwap,
    pub services: &'a dyn ServiceManager,
}

/// Refuses a program that this command must not replace.
fn check_install(install: InstallKind, exe: &Path) -> Result<()> {
    if install == InstallKind::Docker {
        bail!(
            "this hub runs in a container, and a new image replaces its program. \
             Run `git pull`, then `docker compose -f deploy/hub/docker-compose.yml up -d --build`"
        );
    }
    if install == InstallKind::Local {
        bail!(
            "this hub runs on this computer, and it updates with the app. \
             Restart the app to install an update"
        );
    }
    if is_build_or_bundle(exe) {
        bail!(
            "{} is in a build folder or an app bundle, and this command does not replace it. \
             Build it again, or update the app",
            exe.display()
        );
    }
    Ok(())
}

/// Finds the version to install. `None` means that this program is that
/// version already.
async fn choose_version(target: &Target, host: &dyn ReleaseHost) -> Result<Option<Version>> {
    let version = match &target.named {
        Some(named) => Version::parse(named)
            .with_context(|| format!("{named} is not a version. Name a version such as 0.8.0"))?,
        None => latest_version(host).await?,
    };
    if version < target.current {
        bail!(
            "this program is {} already, and {version} is older. An old program cannot read \
             a database that a newer one changed. To go back, restore a backup first",
            target.current
        );
    }
    Ok((version != target.current).then_some(version))
}

/// Runs the command, and returns the lines to show.
pub async fn update(target: Target, seams: Seams<'_>) -> Result<Vec<String>> {
    check_install(target.install, &target.exe)?;
    let Some(version) = choose_version(&target, seams.host).await? else {
        return Ok(vec![format!(
            "This hub program is {} already.",
            target.current
        )]);
    };
    let program = fetch_program(seams.host, &version, &target.asset, &target.work).await;
    let result = match program {
        Ok(program) => seams
            .swap
            .swap(&program, &target.exe)
            .map_err(anyhow::Error::from),
        Err(err) => Err(err.into()),
    };
    let _ = std::fs::remove_dir_all(&target.work);
    result.with_context(|| format!("{} did not change", target.exe.display()))?;
    remove_old_program(&target.exe);

    let mut lines = vec![format!(
        "{} is now version {version}. The SHA-256 matches the release.",
        target.exe.display()
    )];
    let running: Vec<&str> = if seams.services.can_restart() {
        SERVICES
            .into_iter()
            .filter(|service| seams.services.is_active(service))
            .collect()
    } else {
        Vec::new()
    };
    if running.is_empty() {
        lines.push(format!(
            "Restart the hub to use the new version. With systemd, run `sudo systemctl restart {}`.",
            SERVICES[0]
        ));
        lines.push(format!(
            "If a session host runs on this machine, also run `sudo systemctl restart {}`.",
            SERVICES[1]
        ));
    }
    for service in running {
        seams.services.restart(service)?;
        lines.push(format!("Restarted {service}."));
    }
    Ok(lines)
}

/// The folder for a download, in the temporary folder of the system.
pub fn work_folder() -> PathBuf {
    std::env::temp_dir().join(format!("consort-hub-update-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use consort_link::update::UpdateError;
    use consort_proto::release::{CHECKSUMS_ASSET, latest_release_url, release_file_url};
    use sha2::{Digest as _, Sha256};

    use super::*;

    const ASSET: &str = "consort-linux-x86_64.tar.gz";

    #[derive(Default)]
    struct FakeHost(HashMap<String, Vec<u8>>);

    #[async_trait]
    impl ReleaseHost for FakeHost {
        async fn fetch(&self, url: &str, _limit: usize) -> Result<Vec<u8>, UpdateError> {
            self.0.get(url).cloned().ok_or(UpdateError::Status {
                status: 404,
                url: url.to_owned(),
            })
        }
        async fn download(&self, url: &str, dest: &Path, limit: u64) -> Result<(), UpdateError> {
            let bytes = self.fetch(url, limit as usize).await?;
            std::fs::write(dest, bytes).map_err(|e| UpdateError::Storage {
                path: dest.display().to_string(),
                message: e.to_string(),
            })
        }
    }

    #[derive(Default)]
    struct FakeSwap(Mutex<Option<Vec<u8>>>);

    impl ProgramSwap for FakeSwap {
        fn swap(&self, new: &Path, _target: &Path) -> Result<(), UpdateError> {
            *self.0.lock().unwrap() = Some(std::fs::read(new).unwrap());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeServices {
        root: bool,
        active: Vec<&'static str>,
        restarted: Mutex<Vec<String>>,
    }

    impl ServiceManager for FakeServices {
        fn can_restart(&self) -> bool {
            self.root
        }
        fn is_active(&self, service: &str) -> bool {
            self.active.contains(&service)
        }
        fn restart(&self, service: &str) -> Result<()> {
            self.restarted.lock().unwrap().push(service.to_owned());
            Ok(())
        }
    }

    fn release(version: &str) -> FakeHost {
        let v = Version::parse(version).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut tar = tar::Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(11);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, "consort", &b"new program"[..])
                .unwrap();
            tar.finish().unwrap();
        }
        let archive = gz.finish().unwrap();
        let hash: String = Sha256::digest(&archive)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut files = HashMap::new();
        files.insert(
            release_file_url(&v, CHECKSUMS_ASSET),
            format!("{hash}  {ASSET}\n").into_bytes(),
        );
        files.insert(release_file_url(&v, ASSET), archive);
        files.insert(
            latest_release_url(),
            format!(r#"{{"tag_name":"v{version}"}}"#).into_bytes(),
        );
        FakeHost(files)
    }

    fn target(named: Option<&str>, dir: &Path) -> Target {
        Target {
            named: named.map(str::to_owned),
            current: Version::parse("0.7.0").unwrap(),
            install: InstallKind::Service,
            exe: PathBuf::from("/usr/local/bin/consort"),
            asset: ASSET.to_owned(),
            work: dir.join("work"),
        }
    }

    async fn run(
        target: Target,
        host: &FakeHost,
        swap: &FakeSwap,
        services: &FakeServices,
    ) -> Result<Vec<String>> {
        update(
            target,
            Seams {
                host,
                swap,
                services,
            },
        )
        .await
    }

    #[tokio::test]
    async fn a_named_version_replaces_the_program_and_says_to_restart() {
        let dir = tempfile::tempdir().unwrap();
        let swap = FakeSwap::default();
        let services = FakeServices::default();
        let lines = run(
            target(Some("0.8.0"), dir.path()),
            &release("0.8.0"),
            &swap,
            &services,
        )
        .await
        .unwrap();
        assert_eq!(swap.0.lock().unwrap().as_deref(), Some(&b"new program"[..]));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("is now version 0.8.0"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("systemctl restart consort-hub"))
        );
        assert!(services.restarted.lock().unwrap().is_empty());
        assert!(!dir.path().join("work").exists(), "the download is gone");
    }

    #[tokio::test]
    async fn with_no_version_it_takes_the_latest_release() {
        let dir = tempfile::tempdir().unwrap();
        let swap = FakeSwap::default();
        let lines = run(
            target(None, dir.path()),
            &release("0.9.1"),
            &swap,
            &FakeServices::default(),
        )
        .await
        .unwrap();
        assert!(lines[0].contains("is now version 0.9.1"), "{lines:?}");
        assert!(swap.0.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn as_root_with_systemd_it_restarts_the_services_that_run() {
        let dir = tempfile::tempdir().unwrap();
        let services = FakeServices {
            root: true,
            active: vec!["consort-hub"],
            ..FakeServices::default()
        };
        let lines = run(
            target(Some("0.8.0"), dir.path()),
            &release("0.8.0"),
            &FakeSwap::default(),
            &services,
        )
        .await
        .unwrap();
        assert_eq!(*services.restarted.lock().unwrap(), vec!["consort-hub"]);
        assert!(!lines.iter().any(|line| line.contains("sudo systemctl")));
    }

    #[tokio::test]
    async fn a_wrong_checksum_leaves_the_program_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = release("0.8.0");
        let v = Version::parse("0.8.0").unwrap();
        host.0.insert(
            release_file_url(&v, CHECKSUMS_ASSET),
            format!("{}  {ASSET}\n", "0".repeat(64)).into_bytes(),
        );
        let swap = FakeSwap::default();
        let err = run(
            target(Some("0.8.0"), dir.path()),
            &host,
            &swap,
            &FakeServices::default(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("checksum"), "{err:#}");
        assert!(swap.0.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn it_refuses_an_older_version_and_stops_at_the_same_version() {
        let dir = tempfile::tempdir().unwrap();
        let swap = FakeSwap::default();
        let err = run(
            target(Some("0.6.0"), dir.path()),
            &release("0.6.0"),
            &swap,
            &FakeServices::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("older"), "{err}");
        let lines = run(
            target(Some("0.7.0"), dir.path()),
            &release("0.7.0"),
            &swap,
            &FakeServices::default(),
        )
        .await
        .unwrap();
        assert!(lines[0].contains("0.7.0 already"), "{lines:?}");
        assert!(swap.0.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_container_and_a_build_folder_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let swap = FakeSwap::default();
        let services = FakeServices::default();
        let mut docker = target(Some("0.8.0"), dir.path());
        docker.install = InstallKind::Docker;
        let err = run(docker, &release("0.8.0"), &swap, &services)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("docker compose"), "{err}");
        let mut local = target(Some("0.8.0"), dir.path());
        local.install = InstallKind::Local;
        let err = run(local, &release("0.8.0"), &swap, &services)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("updates with the app"), "{err}");
        let mut build = target(Some("0.8.0"), dir.path());
        build.exe = PathBuf::from("/src/consort/target/release/consort");
        let err = run(build, &release("0.8.0"), &swap, &services)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("build folder"), "{err}");
        assert!(swap.0.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_name_that_is_not_a_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(
            target(Some("latest"), dir.path()),
            &release("0.8.0"),
            &FakeSwap::default(),
            &FakeServices::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not a version"), "{err}");
    }
}
