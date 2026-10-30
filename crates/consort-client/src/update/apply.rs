//! Replaces the installed app with a staged update, and starts the new copy
//! after this process ends.
//!
//! macOS: the app is a bundle (`Consort.app`). The update unpacks next to
//! the installed bundle, and then two renames swap the bundles. If the second
//! rename fails, the first one is undone, so the user keeps a working app.
//!
//! Windows and Linux: a running program cannot be replaced safely from
//! inside. The app writes a small script, starts it, and quits. The script
//! waits until the app has ended, copies the new files over the installed
//! ones, and starts the app again. This is a best effort: it needs write
//! access to the install folder, and it logs what went wrong to
//! `update.log` in the staging folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::net::UpdateError;
use super::{Os, Platform};

/// Where the app is installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    pub os: Os,
    /// The `.app` bundle on macOS. The folder with the program elsewhere.
    pub root: PathBuf,
    /// The file name of the program.
    pub exe_name: String,
}

impl Install {
    /// The install of the running program, or `None` when it runs from a
    /// build folder, which cannot replace itself.
    pub fn detect(exe: &Path) -> Option<Install> {
        Install::detect_for(Platform::current()?.os, exe)
    }

    /// The same for a given system, so the tests can check every system.
    pub fn detect_for(os: Os, exe: &Path) -> Option<Install> {
        let exe_name = exe.file_name()?.to_string_lossy().into_owned();
        match os {
            Os::MacOs => {
                let bundle = exe
                    .ancestors()
                    .find(|p| p.extension().is_some_and(|e| e == "app"))?;
                Some(Install {
                    os,
                    root: bundle.to_path_buf(),
                    exe_name,
                })
            }
            Os::Windows | Os::Linux => {
                // `cargo run` and `cargo build` put the program in
                // `target/debug` or `target/release`.
                if exe.components().any(|c| c.as_os_str() == "target") {
                    return None;
                }
                Some(Install {
                    os,
                    root: exe.parent()?.to_path_buf(),
                    exe_name,
                })
            }
        }
    }
}

fn install_error(message: impl Into<String>) -> UpdateError {
    UpdateError::Install(message.into())
}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<(), UpdateError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| install_error(format!("Consort could not run {program}: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(install_error(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Replaces the bundle `app` with the one in the zip `archive`. With
/// `verify`, the new bundle must pass `codesign --verify` first.
pub fn replace_macos_app(archive: &Path, app: &Path, verify: bool) -> Result<(), UpdateError> {
    let name = app
        .file_name()
        .ok_or_else(|| install_error("The app has no name"))?
        .to_os_string();
    let parent = app
        .parent()
        .ok_or_else(|| install_error("The app has no folder"))?;
    let work = archive
        .parent()
        .ok_or_else(|| install_error("The update has no folder"))?
        .join("unpacked");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)
        .map_err(|e| install_error(format!("Consort could not make {}: {e}", work.display())))?;
    run(
        "ditto",
        &[
            "-x".as_ref(),
            "-k".as_ref(),
            archive.as_os_str(),
            work.as_os_str(),
        ],
    )?;
    let fresh = work.join(&name);
    if !fresh.is_dir() {
        return Err(install_error(format!(
            "The update file has no {}",
            name.to_string_lossy()
        )));
    }
    if verify {
        run(
            "codesign",
            &["--verify".as_ref(), "--strict".as_ref(), fresh.as_os_str()],
        )
        .map_err(|e| {
            install_error(format!(
                "The new app has a bad signature, so Consort did not install it. {e}"
            ))
        })?;
    }
    // Copy next to the installed app, so the swap is a rename on one volume.
    let mut staged_name = std::ffi::OsString::from(".");
    staged_name.push(&name);
    staged_name.push(".update");
    let mut old_name = std::ffi::OsString::from(".");
    old_name.push(&name);
    old_name.push(".old");
    let staged = parent.join(staged_name);
    let old = parent.join(old_name);
    let _ = std::fs::remove_dir_all(&staged);
    let _ = std::fs::remove_dir_all(&old);
    run("ditto", &[fresh.as_os_str(), staged.as_os_str()]).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staged);
        install_error(format!(
            "Consort could not copy the new app to {}. Move Consort to a folder that you can write to, then try again. {e}",
            parent.display()
        ))
    })?;
    if let Err(err) = std::fs::rename(app, &old) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(install_error(format!(
            "Consort could not replace {}: {err}. Move Consort to a folder that you can write to, then try again",
            app.display()
        )));
    }
    if let Err(err) = std::fs::rename(&staged, app) {
        // Put the old app back.
        let _ = std::fs::rename(&old, app);
        let _ = std::fs::remove_dir_all(&staged);
        return Err(install_error(format!(
            "Consort could not put the new app in place: {err}. The old app is still there"
        )));
    }
    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}

/// The program and arguments that start the app again, one second after
/// this process asks. The shell gets the path as an argument, so no quoting
/// can go wrong.
pub fn macos_relaunch_command(app: &Path) -> (String, Vec<String>) {
    (
        "/bin/sh".to_owned(),
        vec![
            "-c".to_owned(),
            "sleep 1; exec /usr/bin/open -n \"$1\"".to_owned(),
            "sh".to_owned(),
            app.to_string_lossy().into_owned(),
        ],
    )
}

fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The PowerShell script for Windows. It waits for the process `pid`, unpacks
/// the zip, copies the `Consort` folder of the zip over the install folder,
/// and starts the program.
pub fn windows_script(pid: u32, archive: &Path, install: &Install, log: &Path) -> String {
    // The paths are joined as text with backslashes, so the script is the
    // same when a test builds it on another system.
    let archive_text = archive.to_string_lossy();
    let folder = archive_text
        .rsplit_once('\\')
        .map_or("", |(folder, _)| folder);
    let work = format!("{folder}\\unpacked");
    let root = install.root.to_string_lossy();
    let exe = format!("{root}\\{}", install.exe_name);
    format!(
        "$ErrorActionPreference = 'Stop'\r\n\
         $log = {log}\r\n\
         try {{\r\n\
         \x20 Wait-Process -Id {pid} -Timeout 120 -ErrorAction SilentlyContinue\r\n\
         \x20 if (Test-Path {work}) {{ Remove-Item -Recurse -Force {work} }}\r\n\
         \x20 Expand-Archive -Force -Path {archive} -DestinationPath {work}\r\n\
         \x20 Copy-Item -Recurse -Force (Join-Path {work} 'Consort\\*') {root}\r\n\
         \x20 Remove-Item -Recurse -Force {work}\r\n\
         }} catch {{\r\n\
         \x20 Add-Content -Path $log -Value (\"Update failed: \" + $_.Exception.Message)\r\n\
         }}\r\n\
         Start-Process -FilePath {exe}\r\n",
        log = powershell_quote(&log.to_string_lossy()),
        work = powershell_quote(&work),
        archive = powershell_quote(&archive_text),
        root = powershell_quote(&root),
        exe = powershell_quote(&exe),
    )
}

fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The shell script for Linux. It waits for the process `pid`, unpacks the
/// tarball, copies the files over the install folder, and starts the program.
pub fn unix_script(pid: u32, archive: &Path, install: &Install, log: &Path) -> String {
    let work = archive
        .parent()
        .map(|p| p.join("unpacked"))
        .unwrap_or_default();
    let exe = install.root.join(&install.exe_name);
    let new_exe = install.root.join(format!("{}.new", install.exe_name));
    format!(
        "#!/bin/sh\n\
         log={log}\n\
         n=0\n\
         while kill -0 {pid} 2>/dev/null && [ \"$n\" -lt 600 ]; do sleep 0.2; n=$((n + 1)); done\n\
         rm -rf {work}\n\
         mkdir -p {work}\n\
         if tar -xzf {archive} -C {work} \\\n\
         \x20 && cp -f {work}/Consort/{name} {new_exe} \\\n\
         \x20 && mv -f {new_exe} {exe}; then\n\
         \x20 cp -f {work}/Consort/consort.png {root}/ 2>/dev/null || true\n\
         else\n\
         \x20 echo \"Update failed. Check that you can write to {root_text}\" >> \"$log\"\n\
         fi\n\
         rm -rf {work}\n\
         nohup {exe} >/dev/null 2>&1 &\n",
        log = sh_quote(&log.to_string_lossy()),
        work = sh_quote(&work.to_string_lossy()),
        archive = sh_quote(&archive.to_string_lossy()),
        name = sh_quote(&install.exe_name),
        new_exe = sh_quote(&new_exe.to_string_lossy()),
        exe = sh_quote(&exe.to_string_lossy()),
        root = sh_quote(&install.root.to_string_lossy()),
        root_text = install
            .root
            .to_string_lossy()
            .replace(['"', '`', '$', '\\'], ""),
    )
}

fn spawn_detached(mut command: Command) -> Result<(), UpdateError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A new process group, so the helper outlives the app.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS and CREATE_NO_WINDOW.
        command.creation_flags(0x0800_0008);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| install_error(format!("Consort could not start the update helper: {e}")))
}

/// Installs the staged update. The caller quits the app when this returns
/// `Ok`; the helper then starts the new copy.
pub fn install_and_relaunch(
    platform: Platform,
    install: &Install,
    archive: &Path,
) -> Result<(), UpdateError> {
    match platform.os {
        Os::MacOs => {
            replace_macos_app(archive, &install.root, true)?;
            let (program, args) = macos_relaunch_command(&install.root);
            let mut command = Command::new(program);
            command.args(args);
            spawn_detached(command)
        }
        Os::Windows | Os::Linux => {
            let folder = archive
                .parent()
                .ok_or_else(|| install_error("The update has no folder"))?;
            let log = folder.join("update.log");
            let pid = std::process::id();
            if platform.os == Os::Windows {
                let script = folder.join("update.ps1");
                std::fs::write(&script, windows_script(pid, archive, install, &log)).map_err(
                    |e| install_error(format!("Consort could not write the update script: {e}")),
                )?;
                let mut command = Command::new("powershell.exe");
                command.args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-WindowStyle",
                    "Hidden",
                    "-File",
                ]);
                command.arg(&script);
                spawn_detached(command)
            } else {
                let script = folder.join("update.sh");
                std::fs::write(&script, unix_script(pid, archive, install, &log)).map_err(|e| {
                    install_error(format!("Consort could not write the update script: {e}"))
                })?;
                let mut command = Command::new("/bin/sh");
                command.arg(&script);
                spawn_detached(command)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mac_app_is_found_by_its_bundle() {
        let install = Install::detect_for(
            Os::MacOs,
            Path::new("/Applications/Consort.app/Contents/MacOS/Consort"),
        )
        .unwrap();
        assert_eq!(install.root, PathBuf::from("/Applications/Consort.app"));
        assert_eq!(install.exe_name, "Consort");
        assert!(
            Install::detect_for(Os::MacOs, Path::new("/repo/target/debug/consort-desktop"))
                .is_none(),
            "a program outside a bundle cannot replace itself"
        );
    }

    #[test]
    fn a_build_folder_program_cannot_replace_itself_elsewhere() {
        for os in [Os::Windows, Os::Linux] {
            assert!(
                Install::detect_for(os, Path::new("/repo/target/release/consort-desktop"))
                    .is_none()
            );
        }
        let install = Install::detect_for(
            Os::Linux,
            Path::new("/home/ann/.local/share/Consort/consort-desktop"),
        )
        .unwrap();
        assert_eq!(
            install.root,
            PathBuf::from("/home/ann/.local/share/Consort")
        );
        assert_eq!(install.exe_name, "consort-desktop");
    }

    #[test]
    fn the_mac_relaunch_passes_the_path_as_an_argument() {
        let (program, args) =
            macos_relaunch_command(Path::new("/Applications/My Apps/Consort.app"));
        assert_eq!(program, "/bin/sh");
        assert_eq!(args[1], "sleep 1; exec /usr/bin/open -n \"$1\"");
        assert_eq!(args[3], "/Applications/My Apps/Consort.app");
    }

    fn install(os: Os, root: &str, exe: &str) -> Install {
        Install {
            os,
            root: PathBuf::from(root),
            exe_name: exe.to_owned(),
        }
    }

    #[test]
    fn the_windows_script_waits_unpacks_copies_and_starts_the_app() {
        let script = windows_script(
            4242,
            Path::new(
                "C:\\Users\\Ann\\AppData\\Local\\consort\\updates\\0.2.0\\Consort-windows-x86_64.zip",
            ),
            &install(
                Os::Windows,
                "C:\\Users\\O'Neil\\Consort",
                "consort-desktop.exe",
            ),
            Path::new("C:\\Users\\Ann\\AppData\\Local\\consort\\updates\\0.2.0\\update.log"),
        );
        assert!(script.contains("Wait-Process -Id 4242"));
        assert!(script.contains("Expand-Archive -Force -Path 'C:\\Users\\Ann\\AppData\\Local\\consort\\updates\\0.2.0\\Consort-windows-x86_64.zip'"));
        assert!(script.contains("Copy-Item -Recurse -Force (Join-Path "));
        assert!(script.contains("'Consort\\*') 'C:\\Users\\O''Neil\\Consort'"));
        assert!(script.contains("Add-Content -Path $log"));
        assert!(script.trim_end().ends_with(
            "Start-Process -FilePath 'C:\\Users\\O''Neil\\Consort\\consort-desktop.exe'"
        ));
    }

    #[test]
    fn the_linux_script_waits_unpacks_replaces_and_starts_the_app() {
        let script = unix_script(
            77,
            Path::new("/home/ann/.cache/consort/updates/0.2.0/Consort-linux-x86_64.tar.gz"),
            &install(Os::Linux, "/home/ann/it's here/Consort", "consort-desktop"),
            Path::new("/home/ann/.cache/consort/updates/0.2.0/update.log"),
        );
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("kill -0 77"));
        assert!(script.contains(
            "tar -xzf '/home/ann/.cache/consort/updates/0.2.0/Consort-linux-x86_64.tar.gz' -C "
        ));
        assert!(script.contains("mv -f '/home/ann/it'\\''s here/Consort/consort-desktop.new' '/home/ann/it'\\''s here/Consort/consort-desktop'"));
        assert!(script.trim_end().ends_with(
            "nohup '/home/ann/it'\\''s here/Consort/consort-desktop' >/dev/null 2>&1 &"
        ));
    }

    /// Makes `Consort.app` with one file, as a zip made by `ditto`.
    #[cfg(target_os = "macos")]
    fn make_app_zip(dir: &Path, marker: &str) -> PathBuf {
        let app = dir.join("build").join("Consort.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(app.join("Contents").join("marker.txt"), marker).unwrap();
        let zip = dir
            .join("staging")
            .join("0.2.0")
            .join("Consort-macos-arm64.zip");
        std::fs::create_dir_all(zip.parent().unwrap()).unwrap();
        run(
            "ditto",
            &[
                "-c".as_ref(),
                "-k".as_ref(),
                "--keepParent".as_ref(),
                app.as_os_str(),
                zip.as_os_str(),
            ],
        )
        .unwrap();
        zip
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_new_mac_app_takes_the_place_of_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let zip = make_app_zip(dir.path(), "new");
        let apps = dir.path().join("Applications");
        let app = apps.join("Consort.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(app.join("Contents").join("marker.txt"), "old").unwrap();
        replace_macos_app(&zip, &app, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(app.join("Contents").join("marker.txt")).unwrap(),
            "new"
        );
        let left: Vec<_> = std::fs::read_dir(&apps)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            left,
            vec![std::ffi::OsString::from("Consort.app")],
            "no helper folder stays"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_unsigned_update_is_refused_and_the_old_app_stays() {
        let dir = tempfile::tempdir().unwrap();
        let zip = make_app_zip(dir.path(), "new");
        let apps = dir.path().join("Applications");
        let app = apps.join("Consort.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(app.join("Contents").join("marker.txt"), "old").unwrap();
        let err = replace_macos_app(&zip, &app, true).unwrap_err();
        assert!(err.to_string().contains("bad signature"), "{err}");
        assert_eq!(
            std::fs::read_to_string(app.join("Contents").join("marker.txt")).unwrap(),
            "old"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_zip_without_the_app_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("Other.app");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("x"), "x").unwrap();
        let zip = dir.path().join("staging").join("a.zip");
        std::fs::create_dir_all(zip.parent().unwrap()).unwrap();
        run(
            "ditto",
            &[
                "-c".as_ref(),
                "-k".as_ref(),
                "--keepParent".as_ref(),
                other.as_os_str(),
                zip.as_os_str(),
            ],
        )
        .unwrap();
        let app = dir.path().join("Consort.app");
        std::fs::create_dir_all(&app).unwrap();
        let err = replace_macos_app(&zip, &app, false).unwrap_err();
        assert!(err.to_string().contains("has no Consort.app"), "{err}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_folder_that_cannot_be_written_keeps_the_old_app() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let zip = make_app_zip(dir.path(), "new");
        let apps = dir.path().join("Applications");
        let app = apps.join("Consort.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(app.join("Contents").join("marker.txt"), "old").unwrap();
        std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = replace_macos_app(&zip, &app, false);
        std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("Move Consort to a folder that you can write to"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(app.join("Contents").join("marker.txt")).unwrap(),
            "old"
        );
    }
}
