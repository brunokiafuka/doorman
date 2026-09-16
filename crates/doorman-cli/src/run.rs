//! `doorman run`: start a dev server on a free port behind a named route.

use std::{
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    sync::atomic::{AtomicI32, Ordering},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use doorman_core::{
    APP_PORT_RANGE, BUILTIN_DOMAIN, PROJECT_CONFIG_FILE, Request, Response, Route, sanitize_label,
    validate_name,
};
use serde::Deserialize;
use serde_json::Value;

use crate::client;

/// Project settings from `doorman.json`, or a settings block in package.json.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub name: Option<String>,
    /// package.json script to run when no command is given (default: "dev").
    pub script: Option<String>,
    /// Fixed port instead of a random one.
    pub app_port: Option<u16>,
}

pub struct Project {
    pub root: PathBuf,
    pub config: ProjectConfig,
    pub package: Option<Value>,
}

impl Project {
    pub fn discover(start: &Path) -> Self {
        let root = start
            .ancestors()
            .find(|directory| {
                directory.join(PROJECT_CONFIG_FILE).is_file()
                    || directory.join("package.json").is_file()
            })
            .unwrap_or(start)
            .to_path_buf();
        let package = read_json::<Value>(&root.join("package.json"));
        let config = read_json::<ProjectConfig>(&root.join(PROJECT_CONFIG_FILE))
            .or_else(|| package.as_ref().and_then(package_config))
            .unwrap_or_default();
        Self {
            root,
            config,
            package,
        }
    }

    fn script(&self, name: &str) -> Option<&str> {
        self.package.as_ref()?.get("scripts")?.get(name)?.as_str()
    }

    /// Project config name, package.json name, git root, then folder.
    fn name(&self, git: Option<&GitInfo>) -> String {
        let from_package = self
            .package
            .as_ref()
            .and_then(|package| package.get("name").and_then(Value::as_str));
        let candidates = [
            self.config.name.clone(),
            from_package.map(str::to_owned),
            git.and_then(|git| folder_name(&git.root)),
            folder_name(&self.root),
        ];
        candidates
            .into_iter()
            .flatten()
            .map(|candidate| {
                if validate_name(&candidate).is_ok() {
                    candidate
                } else {
                    sanitize_label(&candidate)
                }
            })
            .find(|candidate| !candidate.is_empty())
            .unwrap_or_else(|| "app".to_owned())
    }
}

/// Reads the `"doorman"` block from package.json: a bare name string, or an object with
/// `name`, `script`, and `appPort`.
fn package_config(package: &Value) -> Option<ProjectConfig> {
    match package.get("doorman")? {
        Value::String(name) => Some(ProjectConfig {
            name: Some(name.clone()),
            ..ProjectConfig::default()
        }),
        value @ Value::Object(_) => serde_json::from_value(value.clone()).ok(),
        _ => None,
    }
}

fn folder_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub struct GitInfo {
    root: PathBuf,
    /// Shared git directory, identical across worktrees.
    common_dir: PathBuf,
    /// `None` when HEAD is detached.
    branch: Option<String>,
    linked: bool,
}

impl GitInfo {
    /// DNS-safe branch label used to prefix routes started from a linked worktree.
    fn worktree_label(&self) -> Option<String> {
        self.branch
            .as_deref()
            .filter(|_| self.linked)
            .map(sanitize_label)
            .filter(|label| !label.is_empty())
    }

    fn context(&self, base_name: &str) -> doorman_core::GitContext {
        // The shared git dir lives inside the main checkout (`<repo>/.git`).
        let main_checkout = self.common_dir.parent().unwrap_or(&self.common_dir);
        doorman_core::GitContext {
            repo: self.common_dir.display().to_string(),
            repo_name: folder_name(main_checkout).unwrap_or_else(|| "repository".to_owned()),
            branch: self.branch.clone(),
            worktree: self.root.display().to_string(),
            linked: self.linked,
            base_name: base_name.to_owned(),
        }
    }
}

fn git_info(directory: &Path) -> Option<GitInfo> {
    let output = Command::new("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
            "--abbrev-ref",
            "HEAD",
        ])
        .current_dir(directory)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut lines = stdout.lines();
    let (root, git_dir, common_dir, branch) =
        (lines.next()?, lines.next()?, lines.next()?, lines.next()?);
    Some(GitInfo {
        root: PathBuf::from(root),
        common_dir: PathBuf::from(common_dir),
        branch: (branch != "HEAD").then(|| branch.to_owned()),
        // In a linked worktree the per-checkout git dir differs from the shared one.
        linked: git_dir != common_dir,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageManager {
    /// The manager that invoked us (e.g. `pnpm dev`), else the nearest lockfile, which in
    /// a monorepo lives above the package.
    fn detect(root: &Path) -> Self {
        let invoked_by = std::env::var("npm_config_user_agent")
            .ok()
            .and_then(|agent| {
                let program = agent.split('/').next()?.to_owned();
                Self::parse(&program)
            });
        invoked_by
            .or_else(|| root.ancestors().find_map(Self::from_lockfile))
            .unwrap_or(Self::Npm)
    }

    fn from_lockfile(directory: &Path) -> Option<Self> {
        [
            ("pnpm-lock.yaml", Self::Pnpm),
            ("yarn.lock", Self::Yarn),
            ("bun.lock", Self::Bun),
            ("bun.lockb", Self::Bun),
            ("package-lock.json", Self::Npm),
        ]
        .into_iter()
        .find(|(file, _)| directory.join(file).exists())
        .map(|(_, manager)| manager)
    }

    fn parse(program: &str) -> Option<Self> {
        match program {
            "npm" => Some(Self::Npm),
            "pnpm" => Some(Self::Pnpm),
            "yarn" => Some(Self::Yarn),
            "bun" => Some(Self::Bun),
            _ => None,
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }
}

pub struct RunOptions {
    pub name: Option<String>,
    pub command: Vec<String>,
}

pub fn run(options: RunOptions) -> Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let project = Project::discover(&cwd);
    let mut command = options.command;
    if command.is_empty() {
        let script = project.config.script.as_deref().unwrap_or("dev");
        match project.script(script) {
            None => bail!(
                "no command given and package.json has no \"{script}\" script.\n\
                 Try: doorman run <command>, e.g. doorman run npm run dev"
            ),
            // `"dev": "doorman"` would start itself forever.
            Some(body) if body.split_whitespace().next() == Some("doorman") => {
                bail!(
                    "the \"{script}\" script runs {body} itself. Point Doorman at the real server script, \
                 e.g. add \"doorman\": {{ \"script\": \"dev:app\" }} to package.json"
                )
            }
            Some(_) => {}
        }
        let manager = PackageManager::detect(&project.root);
        command = vec![
            manager.binary().to_owned(),
            "run".to_owned(),
            script.to_owned(),
        ];
    }

    // DOORMAN=0 runs the command untouched, e.g. in CI.
    if std::env::var("DOORMAN").as_deref() == Ok("0") {
        let status = Command::new(&command[0]).args(&command[1..]).status()?;
        return Ok(exit_code(status.code()));
    }

    let git = git_info(&cwd);
    let base = match options.name {
        Some(name) => {
            validate_name(&name)?;
            name
        }
        None => project.name(git.as_ref()),
    };
    let worktree_label = git.as_ref().and_then(GitInfo::worktree_label);
    let name = match worktree_label.as_deref() {
        Some(branch) if !base.starts_with(&format!("{branch}.")) => format!("{branch}.{base}"),
        _ => base.clone(),
    };
    validate_name(&name).with_context(|| format!("route name '{name}'"))?;

    client::ensure_running()?;
    let status = client::status()?;
    if let Response::Routes { routes, .. } = client::call(&Request::List)?
        && let Some(existing) = routes.iter().find(|route| route.name == name)
    {
        match existing.pid {
            Some(pid) => bail!(
                "{name} is already running (pid {pid}). Stop it first, or pass --name <other>"
            ),
            None => bail!(
                "{name} is a fixed route to port {}. Remove it with `doorman remove {name}`, or pass --name <other>",
                existing.port
            ),
        }
    }

    // A pinned port belongs to the main checkout; worktrees running alongside it would
    // collide on it, so they get a free port and are reached through their own URL.
    let pinned_port = project
        .config
        .app_port
        .filter(|_| !git.as_ref().is_some_and(|git| git.linked));
    let port = match pinned_port {
        Some(port) => port,
        None => free_port().context("no free port between 4000 and 4999")?,
    };
    let script_body = script_body(&command, &project);
    let command = with_port_flags(&command, script_body.as_deref(), port);

    let hosts: Vec<String> = std::iter::once(BUILTIN_DOMAIN.to_owned())
        .chain(status.domains.iter().map(|domain| domain.name.clone()))
        .map(|domain| format!("{name}.{domain}"))
        .collect();
    let primary_url = status.endpoints.url(&hosts[0]);
    let allowed_hosts = std::iter::once(BUILTIN_DOMAIN.to_owned())
        .chain(status.domains.iter().map(|domain| domain.name.clone()))
        .map(|domain| format!(".{domain}"))
        .collect::<Vec<_>>()
        .join(",");

    let mut child_command = Command::new(&command[0]);
    child_command
        .args(&command[1..])
        .env("PORT", port.to_string())
        .env("HOST", "127.0.0.1")
        .env("DOORMAN_URL", &primary_url)
        .env("DOORMAN_NAME", &name)
        // Vite rejects unfamiliar Host headers unless told about them.
        .env("__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS", allowed_hosts);
    if let Some(ca) = &status.ca_cert {
        child_command.env("NODE_EXTRA_CA_CERTS", ca);
    }

    let route = Route {
        command: Some(command.join(" ")),
        git: git.as_ref().map(|git| git.context(&base)),
        ..Route::new(
            name.clone(),
            port,
            Some(std::process::id()),
            Some(project.root.display().to_string()),
        )?
    };
    match client::call(&Request::Add { route })? {
        Response::Added { .. } => {}
        other => return Err(client::unexpected(other)),
    }

    eprintln!();
    eprintln!("  \x1b[1mdoorman\x1b[0m  {}", command.join(" "));
    for host in &hosts {
        eprintln!("  \x1b[32m→\x1b[0m {}", status.endpoints.url(host));
    }
    eprintln!("  \x1b[2m  app on 127.0.0.1:{port}\x1b[0m");
    if let (Some(pinned), None) = (project.config.app_port, pinned_port) {
        eprintln!("  \x1b[2m  worktree: free port instead of the pinned {pinned}\x1b[0m");
    }
    if status.ca_cert.is_some() && !status.ca_trusted {
        eprintln!("  \x1b[33m!\x1b[0m run `doorman trust` so browsers accept HTTPS");
    }
    eprintln!();

    // The server gets its own process group so signals reach every process it spawns
    // (npm → sh → node) without touching whoever launched `doorman run`.
    {
        use std::os::unix::process::CommandExt;
        child_command.process_group(0);
    }
    let result = child_command
        .spawn()
        .with_context(|| format!("start `{}`", command[0]));
    let code = match result {
        Ok(mut child) => supervise(&mut child),
        Err(error) => {
            remove_route(&name);
            return Err(error);
        }
    };
    remove_route(&name);
    Ok(exit_code(code))
}

fn remove_route(name: &str) {
    let _ = client::call(&Request::Remove {
        name: name.to_owned(),
    });
}

fn exit_code(code: Option<i32>) -> ExitCode {
    ExitCode::from(code.map_or(1, |code| code.clamp(0, 255) as u8))
}

fn free_port() -> Option<u16> {
    let span = u64::from(APP_PORT_RANGE.end() - APP_PORT_RANGE.start() + 1);
    let offset =
        (chrono::Utc::now().timestamp_subsec_nanos() as u64 ^ u64::from(std::process::id())) % span;
    (0..span)
        .map(|step| APP_PORT_RANGE.start() + ((offset + step) % span) as u16)
        .find(|port| {
            TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).is_ok()
                && TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, *port)).is_ok()
        })
}

/// The package.json script a `<manager> run <script>` command delegates to.
fn script_body(command: &[String], project: &Project) -> Option<String> {
    let manager = PackageManager::parse(command.first()?)?;
    let script = match command.get(1).map(String::as_str) {
        Some("run" | "run-script") => command.get(2)?,
        Some("start") => "start",
        Some(other) if manager != PackageManager::Npm => other,
        _ => return None,
    };
    project.script(script).map(str::to_owned)
}

/// Adds port flags for dev servers that ignore `PORT`. Leaves anything it doesn't
/// fully understand untouched.
pub fn with_port_flags(command: &[String], script_body: Option<&str>, port: u16) -> Vec<String> {
    let mut command = command.to_vec();
    let has_port = |tokens: &[String]| {
        tokens
            .iter()
            .any(|token| token == "-p" || token.starts_with("--port"))
    };
    if has_port(&command) {
        return command;
    }

    if let Some(body) = script_body {
        let tokens: Vec<String> = body.split_whitespace().map(str::to_owned).collect();
        if has_port(&tokens) || is_compound(&tokens) {
            return command;
        }
        if let Some(flags) = port_flags(&tokens, port) {
            // npm needs `--` to forward arguments to the script.
            if command[0] == "npm" {
                command.push("--".to_owned());
            }
            command.extend(flags);
        }
        return command;
    }

    if !is_compound(&command)
        && let Some(flags) = port_flags(&command, port)
    {
        command.extend(flags);
    }
    command
}

fn is_compound(tokens: &[String]) -> bool {
    tokens.iter().any(|token| {
        ["&&", "||", "|", ";", "&"].contains(&token.as_str())
            || token.contains('=') && !token.starts_with('-')
    })
}

fn port_flags(tokens: &[String], port: u16) -> Option<Vec<String>> {
    // Skip package runners so `npx vite` and `pnpm exec astro dev` are recognized.
    let mut rest = tokens;
    loop {
        match rest.first().map(String::as_str) {
            Some("npx" | "bunx" | "pnpx") => rest = &rest[1..],
            Some("pnpm" | "yarn" | "npm" | "bun")
                if matches!(rest.get(1).map(String::as_str), Some("exec" | "x")) =>
            {
                rest = &rest[2..]
            }
            _ => break,
        }
    }
    let tool = Path::new(rest.first()?).file_name()?.to_str()?;
    let subcommand = rest
        .get(1)
        .map(String::as_str)
        .filter(|value| !value.starts_with('-'));
    let port = port.to_string();
    let host = || vec!["--host".to_owned(), "127.0.0.1".to_owned()];
    let with = |mut flags: Vec<String>| {
        flags.splice(0..0, ["--port".to_owned(), port.clone()]);
        Some(flags)
    };
    match (tool, subcommand) {
        ("vite", None | Some("dev" | "serve" | "preview")) => {
            with([vec!["--strictPort".to_owned()], host()].concat())
        }
        ("astro", Some("dev" | "preview")) => with(host()),
        ("ng", Some("serve" | "s")) => with(host()),
        ("react-router", Some("dev")) => with(host()),
        ("wrangler", Some("dev")) => with(Vec::new()),
        ("webpack", Some("serve")) | ("webpack-dev-server", _) => with(Vec::new()),
        ("expo", Some("start")) | ("react-native", Some("start")) => with(Vec::new()),
        ("storybook", Some("dev")) | ("start-storybook", _) => Some(vec!["-p".to_owned(), port]),
        _ => None,
    }
}

static PENDING_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn remember_signal(signal: libc::c_int) {
    PENDING_SIGNAL.store(signal, Ordering::SeqCst);
}

/// Waits for the server while forwarding signals to its process group. In an
/// interactive terminal the server's group becomes the foreground job, so Ctrl-C and
/// keyboard shortcuts (like Vite's `h`) reach it directly.
fn supervise(child: &mut std::process::Child) -> Option<i32> {
    let group = libc::pid_t::try_from(child.id()).ok()?;
    // SAFETY: plain libc calls on our own terminal and our own child's process group;
    // the handler only stores into an atomic, which is async-signal-safe.
    let owns_terminal = unsafe {
        libc::isatty(libc::STDIN_FILENO) == 1
            && libc::tcgetpgrp(libc::STDIN_FILENO) == libc::getpgrp()
    };
    unsafe {
        let handler = remember_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::signal(signal, handler);
        }
        if owns_terminal {
            // Handing the terminal back later would otherwise stop this process.
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(libc::STDIN_FILENO, group);
            // The server may have touched the terminal before it was foreground.
            libc::kill(-group, libc::SIGCONT);
        }
    }

    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                use std::os::unix::process::ExitStatusExt;
                break status
                    .code()
                    .or_else(|| status.signal().map(|signal| 128 + signal));
            }
            Ok(None) => {}
            Err(_) => break None,
        }
        let signal = PENDING_SIGNAL.swap(0, Ordering::SeqCst);
        if signal != 0 {
            // SAFETY: signalling our child's process group.
            unsafe { libc::kill(-group, signal) };
        }
        thread::sleep(Duration::from_millis(100));
    };

    // SAFETY: as above.
    unsafe {
        // Don't leave grandchildren behind if the top process exited first.
        libc::kill(-group, libc::SIGTERM);
        if owns_terminal {
            libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn injects_flags_for_servers_that_ignore_port() {
        assert_eq!(
            with_port_flags(&args("vite"), None, 4100),
            args("vite --port 4100 --strictPort --host 127.0.0.1")
        );
        assert_eq!(
            with_port_flags(&args("npx astro dev"), None, 4100),
            args("npx astro dev --port 4100 --host 127.0.0.1")
        );
        assert_eq!(
            with_port_flags(&args("ng serve"), None, 4100),
            args("ng serve --port 4100 --host 127.0.0.1")
        );
    }

    #[test]
    fn leaves_port_aware_or_unknown_commands_alone() {
        for line in [
            "next dev",
            "vite build",
            "vite --port 3000",
            "node server.js",
            "a && vite",
        ] {
            assert_eq!(with_port_flags(&args(line), None, 4100), args(line));
        }
    }

    #[test]
    fn forwards_flags_through_package_scripts() {
        assert_eq!(
            with_port_flags(&args("npm run dev"), Some("vite"), 4100),
            args("npm run dev -- --port 4100 --strictPort --host 127.0.0.1")
        );
        assert_eq!(
            with_port_flags(&args("pnpm dev"), Some("astro dev"), 4100),
            args("pnpm dev --port 4100 --host 127.0.0.1")
        );
        assert_eq!(
            with_port_flags(&args("pnpm dev"), Some("next dev"), 4100),
            args("pnpm dev")
        );
        assert_eq!(
            with_port_flags(&args("npm run dev"), Some("NODE_ENV=dev vite"), 4100),
            args("npm run dev")
        );
    }

    #[test]
    fn finds_lockfiles_above_the_package() {
        let root = std::env::temp_dir().join(format!("doorman-lock-{}", std::process::id()));
        let package = root.join("apps/site");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(
            package.ancestors().find_map(PackageManager::from_lockfile),
            Some(PackageManager::Pnpm)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_the_doorman_package_block() {
        let block = serde_json::json!({
            "doorman": { "name": "site.uselucerna", "script": "dev:app", "appPort": 5177 }
        });
        let config = package_config(&block).unwrap();
        assert_eq!(config.name.as_deref(), Some("site.uselucerna"));
        assert_eq!(config.script.as_deref(), Some("dev:app"));
        assert_eq!(config.app_port, Some(5177));

        let name_only = serde_json::json!({ "doorman": "shop" });
        assert_eq!(
            package_config(&name_only).unwrap().name.as_deref(),
            Some("shop")
        );
        assert!(package_config(&serde_json::json!({ "name": "x" })).is_none());
    }

    #[test]
    fn infers_names() {
        let project = |name: &str| Project {
            root: PathBuf::from("/tmp/My Folder"),
            config: ProjectConfig::default(),
            package: Some(serde_json::json!({ "name": name })),
        };
        assert_eq!(project("@acme/web").name(None), "web");
        assert_eq!(project("shop").name(None), "shop");
        let without_package = Project {
            root: PathBuf::from("/tmp/My Folder"),
            config: ProjectConfig::default(),
            package: None,
        };
        assert_eq!(without_package.name(None), "my-folder");
    }
}
