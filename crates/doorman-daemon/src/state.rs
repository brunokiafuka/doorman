use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result};
use doorman_core::{
    Config, Domain, Endpoints, Route, Status, TrafficEntry, ca_cert_path, config_path,
    parent_names, resolver_status, split_host,
};
use tokio::sync::{Notify, RwLock as AsyncRwLock};

pub const MAX_TRAFFIC_ENTRIES: usize = 200;

/// Settings that the TLS resolver reads synchronously during handshakes.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub domains: Vec<String>,
    pub wildcard: bool,
}

pub struct App {
    pub routes: AsyncRwLock<HashMap<String, Route>>,
    pub traffic: AsyncRwLock<VecDeque<TrafficEntry>>,
    pub settings: Arc<RwLock<Settings>>,
    pub endpoints: Endpoints,
    /// Port written into resolver files.
    pub dns_port: u16,
    pub dns_running: bool,
    pub tls_enabled: bool,
    pub ca_trusted: AtomicBool,
    pub shutdown: Notify,
    next_traffic_id: AtomicU64,
}

pub enum LookupError {
    /// The host isn't `<name>.<domain>` for any known domain.
    UnknownHost { accepted: Vec<String> },
    /// The host is well-formed, but no route has that name.
    NotFound { name: String },
}

impl App {
    pub fn new(
        config: Config,
        endpoints: Endpoints,
        dns_port: u16,
        dns_running: bool,
        tls_enabled: bool,
    ) -> Self {
        let routes = config
            .routes
            .into_iter()
            .map(|route| (route.name.clone(), route))
            .collect();
        Self {
            routes: AsyncRwLock::new(routes),
            traffic: AsyncRwLock::new(VecDeque::with_capacity(MAX_TRAFFIC_ENTRIES)),
            settings: Arc::new(RwLock::new(Settings {
                domains: config.domains,
                wildcard: config.wildcard,
            })),
            endpoints,
            dns_port,
            dns_running,
            tls_enabled,
            ca_trusted: AtomicBool::new(false),
            shutdown: Notify::new(),
            next_traffic_id: AtomicU64::new(1),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().expect("settings").clone()
    }

    pub fn domain_info(&self, name: &str) -> Domain {
        Domain {
            name: name.to_owned(),
            resolver: resolver_status(name, self.dns_port),
        }
    }

    pub fn status(&self) -> Status {
        let settings = self.settings();
        Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            endpoints: self.endpoints,
            dns_port: self.dns_running.then_some(self.dns_port),
            domains: settings
                .domains
                .iter()
                .map(|name| self.domain_info(name))
                .collect(),
            wildcard: settings.wildcard,
            ca_cert: self
                .tls_enabled
                .then(|| ca_cert_path().display().to_string()),
            ca_trusted: self.ca_trusted.load(Ordering::Relaxed),
        }
    }

    /// Resolves a request host to a route, dropping routes whose owner has exited.
    pub async fn lookup(&self, host: &str) -> Result<Route, LookupError> {
        let settings = self.settings();
        let Some((name, _)) = split_host(host, &settings.domains) else {
            return Err(LookupError::UnknownHost {
                accepted: std::iter::once("localhost")
                    .chain(settings.domains.iter().map(String::as_str))
                    .map(|domain| format!("<name>.{domain}"))
                    .collect(),
            });
        };
        self.prune_dead_routes().await;
        let routes = self.routes.read().await;
        let wildcard_parents = parent_names(name).filter(|_| settings.wildcard);
        std::iter::once(name)
            .chain(wildcard_parents)
            .find_map(|candidate| routes.get(candidate).cloned())
            .ok_or_else(|| LookupError::NotFound {
                name: name.to_owned(),
            })
    }

    pub async fn prune_dead_routes(&self) {
        let dead: Vec<String> = self
            .routes
            .read()
            .await
            .values()
            .filter(|route| route.pid.is_some_and(|pid| !process_is_alive(pid)))
            .map(|route| route.name.clone())
            .collect();
        if !dead.is_empty() {
            let mut routes = self.routes.write().await;
            for name in dead {
                routes.remove(&name);
            }
        }
    }

    pub async fn record(&self, mut entry: TrafficEntry) {
        entry.id = self.next_traffic_id.fetch_add(1, Ordering::Relaxed);
        let mut traffic = self.traffic.write().await;
        if traffic.len() == MAX_TRAFFIC_ENTRIES {
            traffic.pop_front();
        }
        traffic.push_back(entry);
    }

    /// Writes settings and ownerless (static) routes. Routes owned by a process are
    /// intentionally ephemeral.
    pub async fn persist(&self) -> Result<()> {
        let settings = self.settings();
        let mut routes: Vec<Route> = self
            .routes
            .read()
            .await
            .values()
            .filter(|route| route.pid.is_none())
            .cloned()
            .collect();
        routes.sort_by(|left, right| left.name.cmp(&right.name));
        save_config(&Config {
            domains: settings.domains,
            wildcard: settings.wildcard,
            routes,
        })
    }
}

pub fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

fn save_config(config: &Config) -> Result<()> {
    let path = config_path();
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(config)?)?;
    std::fs::rename(temporary, path).context("save Doorman config")?;
    Ok(())
}

pub fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 checks existence; EPERM means it exists but belongs to someone else.
    // SAFETY: kill with signal 0 sends nothing and has no side effects.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
