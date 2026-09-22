//! Foreground-owned providers: no shell, Terminal, daemon state, or stored credentials.
use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use doorman_core::{
    Route,
    tunnel::{self, Provider},
};
use slint::{Model, ModelRc, SharedString, VecModel};

const MAX_LOG_LINES: usize = 100;
const MAX_LINE_BYTES: u64 = 2048;
const START_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Default)]
pub struct Manager {
    sessions: HashMap<String, Session>,
}

struct Session {
    route: Route,
    provider: Provider,
    child: Option<Child>,
    output: Receiver<String>,
    started: Instant,
    status: String,
    url: String,
    registered: bool,
    error: String,
    logs: VecDeque<LogLine>,
}

struct LogLine {
    received: Instant,
    time: String,
    raw: String,
}

impl LogLine {
    fn new(raw: String) -> Self {
        Self {
            received: Instant::now(),
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            raw,
        }
    }

    fn row(&self, session: &Session) -> crate::TunnelLogItem {
        let json = serde_json::from_str::<serde_json::Value>(&self.raw).ok();
        let level = json
            .as_ref()
            .and_then(|v| v.get("level").or_else(|| v.get("lvl")))
            .and_then(|v| v.as_str())
            .unwrap_or("info")
            .to_uppercase();
        let message = json
            .as_ref()
            .and_then(|v| v.get("message").or_else(|| v.get("msg")))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.raw);
        crate::TunnelLogItem {
            time: self.time.clone().into(),
            level: level.into(),
            route: session.route.name.clone().into(),
            provider: session.provider.binary().into(),
            message: message.into(),
            raw: self.raw.clone().into(),
        }
    }
}

impl Manager {
    pub fn start(&mut self, route: &Route, requested: &str) {
        if self
            .sessions
            .get(&route.name)
            .is_some_and(|s| s.child.is_some())
        {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(100);
        let mut session = Session {
            route: route.clone(),
            provider: Provider::Auto,
            child: None,
            output: rx,
            started: Instant::now(),
            status: "Starting".into(),
            url: String::new(),
            registered: false,
            error: String::new(),
            logs: VecDeque::new(),
        };
        if let Err(error) = session.launch(requested, tx) {
            session.fail(format!("{error:#}"));
        }
        self.sessions.insert(route.name.clone(), session);
    }

    pub fn stop(&mut self, name: &str) {
        if let Some(session) = self.sessions.get_mut(name) {
            session.stop();
        }
    }

    pub fn stop_all(&mut self) {
        for session in self.sessions.values_mut() {
            session.stop();
        }
    }

    pub fn clear_logs(&mut self, route: &str) {
        for session in self.sessions.values_mut() {
            if route.is_empty() || session.route.name == route {
                session.logs.clear();
            }
        }
    }

    fn connected_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .sessions
            .values()
            .filter(|s| s.child.is_some() && s.status == "Connected")
            .map(|s| s.route.name.clone())
            .collect();
        names.sort();
        names
    }

    fn log_rows(&self, route: &str) -> Vec<crate::TunnelLogItem> {
        let mut lines: Vec<_> = self
            .sessions
            .values()
            .filter(|s| route.is_empty() || s.route.name == route)
            .flat_map(|s| s.logs.iter().map(move |line| (s, line)))
            .collect();
        lines.sort_by_key(|(_, line)| line.received);
        lines.into_iter().map(|(s, line)| line.row(s)).collect()
    }

    /// Replacing/removing a route must not leave its former port public.
    pub fn reconcile(&mut self, routes: &[Route]) {
        self.sessions.retain(|_, session| {
            let current = routes.iter().find(|r| r.name == session.route.name);
            match current {
                None => false, // Drop kills and reaps.
                Some(route) => {
                    if route.port != session.route.port
                        || route.pid != session.route.pid
                        || route.created_at != session.route.created_at
                    {
                        session.stop();
                    }
                    true
                }
            }
        });
    }

    pub fn poll(&mut self) {
        for session in self.sessions.values_mut() {
            session.poll();
        }
    }

    pub fn render(&self, app: &crate::MainWindow) {
        let names = self.connected_names();
        app.set_log_route_connected(names.iter().any(|n| n == app.get_log_route().as_str()));
        // Replacing the model destroys the chips' TouchAreas. A log refresh
        // between pointer-down and pointer-up must not swallow the click.
        if connected_names_changed(&app.get_connected_tunnels(), &names) {
            app.set_connected_tunnels(ModelRc::new(VecModel::from(
                names
                    .into_iter()
                    .map(Into::into)
                    .collect::<Vec<SharedString>>(),
            )));
        }
        let log_route = app.get_log_route();
        let log_session = self.sessions.get(log_route.as_str());
        app.set_log_view_status(log_session.map_or("Stopped", |s| &s.status).into());
        app.set_log_view_error(log_session.map_or("", |s| &s.error).into());
        if !app.get_log_view_paused() {
            let rows = self.log_rows(&log_route);
            let text = rows
                .iter()
                .map(|row| {
                    format!(
                        "{} [{}] {} · {} {}",
                        row.time, row.level, row.route, row.provider, row.raw
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            if app.get_log_view_text().as_str() != text {
                app.set_log_view_rows(ModelRc::new(VecModel::from(rows)));
                app.set_log_view_text(text.into());
            }
        }
        let name = app.get_selected_route().name;
        let session = self.sessions.get(name.as_str());
        app.set_tunnel_active(session.is_some_and(|s| s.child.is_some()));
        app.set_tunnel_status(session.map_or("Stopped", |s| &s.status).into());
        app.set_tunnel_url(session.map_or("", |s| &s.url).into());
        app.set_tunnel_error(session.map_or("", |s| &s.error).into());
        app.set_tunnel_logs(
            session
                .map(|s| {
                    s.logs
                        .iter()
                        .map(|l| l.raw.clone())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
                .into(),
        );
        app.set_tunnel_actual_provider(
            session
                .filter(|s| s.provider != Provider::Auto)
                .map_or("", |s| s.provider.binary())
                .into(),
        );
    }
}

fn connected_names_changed(model: &ModelRc<SharedString>, names: &[String]) -> bool {
    model.row_count() != names.len()
        || model
            .iter()
            .zip(names)
            .any(|(old, new)| old.as_str() != new)
}

impl Session {
    fn launch(&mut self, requested: &str, tx: SyncSender<String>) -> Result<()> {
        let path = tunnel::desktop_path();
        let (provider, executable) = tunnel::select(
            Provider::parse(requested).map_err(anyhow::Error::msg)?,
            |binary| tunnel::find_executable(binary, &path),
        )
        .map_err(anyhow::Error::msg)?;
        self.provider = provider;
        let mut child = Command::new(&executable)
            .args(provider.managed_args(&self.route))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if cfg!(target_os = "macos") && error.raw_os_error() == Some(86) {
                    anyhow::anyhow!(
                        "{} cannot run on this Mac (incompatible CPU architecture). \
                         Install the macOS {} build from {} and retry. \
                         Doorman has not changed your installation.",
                        executable.display(),
                        if std::env::consts::ARCH == "aarch64" { "Apple Silicon / arm64" } else { "Intel / amd64" },
                        if provider == Provider::Ngrok { "https://ngrok.com/download" } else { "https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/" },
                    )
                } else {
                    anyhow::Error::from(error)
                }
            })
            .with_context(|| format!("Could not start {}", executable.display()))?;
        read_output(child.stdout.take().expect("piped stdout"), tx.clone());
        read_output(child.stderr.take().expect("piped stderr"), tx);
        self.child = Some(child);
        Ok(())
    }

    fn poll(&mut self) {
        // Bounded work per UI tick; the channel backpressures unusually noisy clients.
        for _ in 0..100 {
            let Ok(line) = self.output.try_recv() else {
                break;
            };
            // CLI parsers may print help and exit successfully for invalid flags.
            // Keep the first diagnostic even when the log ring fills with help.
            if self.error.is_empty()
                && (line.contains("flag provided but not defined")
                    || line.contains("Incorrect Usage")
                    || line.contains("unknown flag"))
            {
                self.error = line.clone();
            }
            if self.child.is_some() {
                let event = parse_output(self.provider, &line);
                if let Some(url) = event.url {
                    self.url = url;
                }
                self.registered |= event.registered;
                if self.registered && !self.url.is_empty() {
                    self.status = "Connected".into();
                }
            }
            self.logs.push_back(LogLine::new(line));
            if self.logs.len() > MAX_LOG_LINES {
                self.logs.pop_front();
            }
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.child = None; // try_wait reaped it.
                let diagnostic = if self.error.is_empty() {
                    format!(
                        "{} exited ({status}) before the tunnel was stopped. See provider logs below.",
                        self.provider.binary()
                    )
                } else {
                    self.error.clone()
                };
                self.fail(diagnostic);
            }
            Err(error) => self.fail(format!("Could not check tunnel: {error}")),
            Ok(None) if self.status == "Starting" && self.started.elapsed() > START_TIMEOUT => {
                self.fail(
                    "No public connection reported within 90 seconds. Check the logs and retry."
                        .into(),
                );
            }
            Ok(None) => {}
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Kill the exact owned child, never a provider found by name. wait
            // reaps it, so Stop/quit cannot leave a background tunnel or zombie.
            let _ = child.kill();
            let _ = child.wait();
        }
        self.status = "Stopped".into();
        self.url.clear();
        self.registered = false;
    }

    fn fail(&mut self, error: String) {
        self.stop();
        self.status = "Failed".into();
        self.error = error;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_output(stream: impl Read + Send + 'static, tx: SyncSender<String>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        loop {
            let mut bytes = Vec::new();
            // Cap even a single unterminated line; never buffer arbitrary output.
            match (&mut reader)
                .take(MAX_LINE_BYTES)
                .read_until(b'\n', &mut bytes)
            {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line: String = String::from_utf8_lossy(&bytes)
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\t')
                        .collect();
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

#[derive(Default)]
struct OutputEvent {
    url: Option<String>,
    registered: bool,
}

fn parse_output(provider: Provider, line: &str) -> OutputEvent {
    let json = serde_json::from_str::<serde_json::Value>(line).ok();
    let message = json
        .as_ref()
        .and_then(|v| v.get("message").or_else(|| v.get("msg")))
        .and_then(|v| v.as_str())
        .unwrap_or(line);
    match provider {
        Provider::Cloudflare => OutputEvent {
            url: message.split_whitespace().find_map(|word| {
                let word = word.trim_matches(|c| matches!(c, '"' | '|' | ','));
                valid_url(word)
                    .filter(|url| url.ends_with(".trycloudflare.com"))
                    .map(str::to_owned)
            }),
            registered: message.contains("Registered tunnel connection"),
        },
        Provider::Ngrok if message == "started tunnel" => {
            let url = json
                .as_ref()
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
                .and_then(valid_url)
                .map(str::to_owned);
            OutputEvent {
                registered: url.is_some(),
                url,
            }
        }
        _ => OutputEvent::default(),
    }
}

// Only accept an HTTPS origin, not an arbitrary log link, shell text, or credentials.
fn valid_url(value: &str) -> Option<&str> {
    let host = value.strip_prefix("https://")?;
    (!host.is_empty()
        && host.contains('.')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')))
    .then_some(value)
}

pub fn availability() -> String {
    let path = tunnel::desktop_path();
    [Provider::Cloudflare, Provider::Ngrok]
        .iter()
        .map(|p| {
            format!(
                "{}: {}",
                p.binary(),
                if tunnel::find_executable(p.binary(), &path).is_some() {
                    "installed"
                } else {
                    "not found"
                }
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_refresh_preserves_unchanged_chips() {
        let model = ModelRc::new(VecModel::from(vec!["api".into(), "shop".into()]));
        let names = vec!["api".into(), "shop".into()];
        assert!(!connected_names_changed(&model, &names));
        assert!(connected_names_changed(&model, &["api".into()]));
        assert!(connected_names_changed(
            &model,
            &["api".into(), "web".into()]
        ));
        assert!(connected_names_changed(
            &model,
            &["shop".into(), "api".into()]
        ));
        assert!(!connected_names_changed(&ModelRc::default(), &[]));
    }

    #[test]
    fn parses_only_public_provider_urls_and_readiness() {
        let cloud = parse_output(
            Provider::Cloudflare,
            r#"{"message":"| https://blue-cat.trycloudflare.com |"}"#,
        );
        assert_eq!(
            cloud.url.as_deref(),
            Some("https://blue-cat.trycloudflare.com")
        );
        assert!(!cloud.registered);
        assert!(
            parse_output(
                Provider::Cloudflare,
                r#"{"message":"Registered tunnel connection connIndex=0"}"#
            )
            .registered
        );
        let ngrok = parse_output(
            Provider::Ngrok,
            r#"{"msg":"started tunnel","url":"https://example.ngrok-free.app"}"#,
        );
        assert!(ngrok.registered);
        assert_eq!(ngrok.url.as_deref(), Some("https://example.ngrok-free.app"));
        assert!(
            parse_output(
                Provider::Ngrok,
                r#"{"msg":"error","url":"https://ngrok.com/docs"}"#
            )
            .url
            .is_none()
        );
        assert!(
            parse_output(
                Provider::Cloudflare,
                "https://api.trycloudflare.com.evil.com"
            )
            .url
            .is_none()
        );
        assert!(valid_url("https://user:secret@host.com").is_none());
        assert!(valid_url("http://localhost:4040").is_none());
    }

    fn running_session() -> Session {
        let route = Route::new("shop", 4321, None, None).unwrap();
        let (_tx, rx) = mpsc::sync_channel(1);
        Session {
            route,
            provider: Provider::Cloudflare,
            child: Some(Command::new("/bin/sleep").arg("30").spawn().unwrap()),
            output: rx,
            started: Instant::now(),
            status: "Starting".into(),
            url: String::new(),
            registered: false,
            error: String::new(),
            logs: VecDeque::new(),
        }
    }

    #[test]
    fn stopping_and_route_replacement_reap_children() {
        let mut manager = Manager::default();
        let session = running_session();
        let mut replacement = session.route.clone();
        manager.sessions.insert("shop".into(), session);
        replacement.port = 4322;
        manager.reconcile(&[replacement]);
        let session = &manager.sessions["shop"];
        assert!(session.child.is_none());
        assert_eq!(session.status, "Stopped");
        manager.sessions.insert("shop".into(), running_session());
        manager.stop_all();
        assert!(manager.sessions["shop"].child.is_none());
        manager.reconcile(&[]);
        assert!(manager.sessions.is_empty());
    }

    #[test]
    fn chips_only_list_connected_tunnels_and_logs_filter_and_clear() {
        let mut manager = Manager::default();
        let mut first = running_session();
        first.status = "Connected".into();
        first.logs.push_back(LogLine::new(
            r#"{"level":"warn","message":"reconnecting"}"#.into(),
        ));
        manager.sessions.insert("shop".into(), first);
        let mut second = running_session();
        second.route.name = "api".into();
        second
            .logs
            .push_back(LogLine::new("Starting provider".into()));
        manager.sessions.insert("api".into(), second);
        assert_eq!(manager.connected_names(), ["shop"]);
        assert_eq!(manager.log_rows("").len(), 2);
        let rows = manager.log_rows("shop");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].level, "WARN");
        assert_eq!(rows[0].message, "reconnecting");
        manager.clear_logs("shop");
        assert_eq!(manager.log_rows("").len(), 1);
        manager.stop("shop");
        assert!(manager.connected_names().is_empty());
        manager.clear_logs("");
        assert!(manager.log_rows("").is_empty());
    }

    #[test]
    fn startup_timeout_clears_url_and_stops_child() {
        let mut session = running_session();
        session.started = Instant::now() - START_TIMEOUT - Duration::from_secs(1);
        session.url = "https://blue-cat.trycloudflare.com".into();
        session.poll();
        assert_eq!(session.status, "Failed");
        assert!(session.child.is_none());
        assert!(session.url.is_empty());
    }

    #[test]
    fn output_is_bounded_and_drained() {
        let (tx, rx) = mpsc::sync_channel(10);
        read_output(std::io::Cursor::new(vec![b'x'; 5000]), tx);
        let lines: Vec<_> = rx.iter().collect();
        assert_eq!(lines.len(), 3);
        assert!(
            lines
                .iter()
                .all(|line| line.len() <= MAX_LINE_BYTES as usize)
        );
    }
}
