//! Talking to the daemon, and starting it when it isn't running.

use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use doorman_core::{Request, Response, Status, socket_path, state_dir};

pub fn call(request: &Request) -> Result<Response> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "Doorman is not running. Start it with `doorman start` (socket: {})",
            path.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line).context("decode daemon response")
}

pub fn unexpected(response: Response) -> anyhow::Error {
    match response {
        Response::Error { message } => anyhow!(message),
        other => anyhow!("unexpected daemon response: {other:?}"),
    }
}

pub fn is_running() -> bool {
    matches!(call(&Request::Ping), Ok(Response::Pong { .. }))
}

pub fn status() -> Result<Status> {
    match call(&Request::Status)? {
        Response::Status { status } => Ok(status),
        other => Err(unexpected(other)),
    }
}

/// Starts the daemon in the background unless it's already up.
pub fn ensure_running() -> Result<()> {
    if is_running() {
        return Ok(());
    }
    let daemon = daemon_path()
        .context("can't find doorman-daemon; set DOORMAN_DAEMON_PATH or install the Doorman app")?;
    std::fs::create_dir_all(state_dir())?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir().join("daemon.log"))?;
    Command::new(&daemon)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .with_context(|| format!("start {}", daemon.display()))?;
    for _ in 0..60 {
        thread::sleep(Duration::from_millis(50));
        if is_running() {
            return Ok(());
        }
    }
    bail!(
        "doorman-daemon didn't come up; see {}",
        state_dir().join("daemon.log").display()
    )
}

/// Finds the daemon binary: explicit override, next to this CLI, inside a nearby or
/// installed Doorman.app, then PATH.
pub fn daemon_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("DOORMAN_DAEMON_PATH") {
        return Some(PathBuf::from(path));
    }
    let mut candidates = Vec::new();
    if let Ok(executable) = env::current_exe().and_then(|path| path.canonicalize())
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join("doorman-daemon"));
        candidates.push(directory.join("../Doorman.app/Contents/MacOS/doorman-daemon"));
    }
    candidates.push(PathBuf::from(
        "/Applications/Doorman.app/Contents/MacOS/doorman-daemon",
    ));
    if let Some(home) = env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home).join("Applications/Doorman.app/Contents/MacOS/doorman-daemon"),
        );
    }
    if let Some(path) = env::var_os("PATH") {
        candidates
            .extend(env::split_paths(&path).map(|directory| directory.join("doorman-daemon")));
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
}
