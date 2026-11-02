//! The `tailscale` program. A trait over it lets the tests run with no
//! Tailscale.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt as _;

/// What a run of the program gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramOutput {
    /// It ended with status 0.
    pub success: bool,
    /// It did not end in the time limit, and the hub stopped it.
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

impl ProgramOutput {
    /// Both streams as one text.
    pub(crate) fn text(&self) -> String {
        [self.stdout.trim(), self.stderr.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Why the program did not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProgramError {
    #[error("the tailscale program is not on this computer")]
    NotFound,
    #[error("the tailscale program did not start: {0}")]
    Start(String),
}

/// Runs `tailscale` with arguments.
#[async_trait::async_trait]
pub trait TailscaleProgram: Send + Sync {
    async fn run(&self, args: &[String]) -> Result<ProgramOutput, ProgramError>;
}

/// The real program. Each run has its input closed and a time limit, because
/// Serve can wait on a consent prompt when HTTPS is off for the tailnet.
pub struct SystemTailscale {
    limit: Duration,
}

/// The most bytes of output that the hub keeps from one stream.
const OUTPUT_LIMIT: u64 = 64 * 1024;
/// How long one run may take.
const RUN_LIMIT: Duration = Duration::from_secs(20);

impl Default for SystemTailscale {
    fn default() -> Self {
        Self { limit: RUN_LIMIT }
    }
}

/// The folders to look in after `PATH`, then the program in the macOS app.
const KNOWN_PATHS: [&str; 3] = [
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

/// Finds the program: first in `path`, then in the known places.
fn find_program(
    path: Option<&std::ffi::OsStr>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "tailscale.exe"
    } else {
        "tailscale"
    };
    path.into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(name))
        .chain(KNOWN_PATHS.iter().map(PathBuf::from))
        .find(|candidate| is_file(candidate))
}

/// Reads a stream into `into`, up to the limit, and then drains it. What was
/// read stays in `into` when the hub stops the program early.
async fn collect(stream: Option<impl tokio::io::AsyncRead + Unpin>, into: Arc<Mutex<Vec<u8>>>) {
    let Some(mut stream) = stream else { return };
    let mut chunk = [0_u8; 4096];
    while let Ok(read) = stream.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        let Ok(mut held) = into.lock() else { break };
        if (held.len() as u64) < OUTPUT_LIMIT {
            held.extend_from_slice(chunk.get(..read).unwrap_or_default());
        }
    }
}

fn taken(buffer: &Mutex<Vec<u8>>) -> String {
    buffer
        .lock()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl TailscaleProgram for SystemTailscale {
    async fn run(&self, args: &[String]) -> Result<ProgramOutput, ProgramError> {
        let path = std::env::var_os("PATH");
        let program = find_program(path.as_deref(), Path::is_file).ok_or(ProgramError::NotFound)?;
        run_with_limit(&program, args, self.limit).await
    }
}

/// Runs a program with its input closed. When the limit ends, it stops the
/// program and returns the output so far.
async fn run_with_limit(
    program: &Path,
    args: &[String],
    limit: Duration,
) -> Result<ProgramOutput, ProgramError> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| ProgramError::Start(err.to_string()))?;
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let readers = tokio::spawn({
        let (out, err) = (stdout.clone(), stderr.clone());
        let (out_stream, err_stream) = (child.stdout.take(), child.stderr.take());
        async move {
            tokio::join!(collect(out_stream, out), collect(err_stream, err));
        }
    });
    let finished = tokio::time::timeout(limit, child.wait()).await;
    let (success, timed_out) = match finished {
        Ok(Ok(status)) => (status.success(), false),
        Ok(Err(err)) => return Err(ProgramError::Start(err.to_string())),
        Err(_) => {
            // Best effort: the process may have ended in this moment.
            let _ = child.kill().await;
            (false, true)
        }
    };
    // A helper that keeps the pipe open must not hold up the answer.
    if tokio::time::timeout(Duration::from_secs(1), readers)
        .await
        .is_err()
    {
        tracing::debug!("the output of tailscale stayed open");
    }
    Ok(ProgramOutput {
        success,
        timed_out,
        stdout: taken(&stdout),
        stderr: taken(&stderr),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_comes_first_and_then_the_known_places() {
        let path = std::ffi::OsString::from("/one:/two");
        let found = find_program(Some(&path), |p| p == Path::new("/two/tailscale"));
        assert_eq!(found, Some(PathBuf::from("/two/tailscale")));
        let found = find_program(Some(&path), |p| {
            p == Path::new("/Applications/Tailscale.app/Contents/MacOS/Tailscale")
        });
        assert_eq!(
            found,
            Some(PathBuf::from(
                "/Applications/Tailscale.app/Contents/MacOS/Tailscale"
            ))
        );
        assert_eq!(find_program(Some(&path), |_| false), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_that_waits_on_its_input_ends_at_once() {
        // The input is closed, so `cat` sees its end and stops.
        let out = run_with_limit(Path::new("/bin/cat"), &[], Duration::from_secs(10))
            .await
            .unwrap();
        assert!(out.success && !out.timed_out);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_that_blocks_stops_at_the_limit_and_keeps_its_output() {
        let args: Vec<String> = ["-c", "echo https://login.tailscale.com/f/serve; sleep 30"]
            .map(str::to_owned)
            .to_vec();
        let started = std::time::Instant::now();
        let out = run_with_limit(Path::new("/bin/sh"), &args, Duration::from_millis(500))
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(out.timed_out && !out.success);
        assert!(out.stdout.contains("https://login.tailscale.com/f/serve"));
    }
}
