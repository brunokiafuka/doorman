use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Preferred ports. The daemon falls back to the `FALLBACK_*` ports when another
/// process (or the OS) holds these.
pub const DEFAULT_HTTP_PORT: u16 = 80;
pub const DEFAULT_HTTPS_PORT: u16 = 443;
pub const FALLBACK_HTTP_PORT: u16 = 3210;
pub const FALLBACK_HTTPS_PORT: u16 = 3443;
/// Kept for older callers; the HTTP fallback port.
pub const DEFAULT_PROXY_PORT: u16 = FALLBACK_HTTP_PORT;
pub const DEFAULT_DNS_PORT: u16 = 53535;
/// Range `doorman run` picks app ports from.
pub const APP_PORT_RANGE: std::ops::RangeInclusive<u16> = 4000..=4999;

pub const SOCKET_FILE: &str = "doorman.sock";
pub const CONFIG_FILE: &str = "config.json";
pub const CA_CERT_FILE: &str = "ca.pem";
pub const CA_KEY_FILE: &str = "ca-key.pem";
pub const CA_COMMON_NAME: &str = "Doorman Local CA";
pub const PROJECT_CONFIG_FILE: &str = "doorman.json";
/// Resolves to loopback everywhere without any setup, so it is always available.
pub const BUILTIN_DOMAIN: &str = "localhost";
pub const RESOLVER_DIR: &str = "/etc/resolver";
/// Route names may nest (e.g. `api.shop`), but not without bound.
pub const MAX_NAME_LABELS: usize = 4;

/// Top-level domains reserved for private use; they never collide with real sites.
const PRIVATE_TLDS: &[&str] = &[
    "test", "internal", "example", "invalid", "lan", "home", "corp", "private",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Route {
    /// One or more DNS labels, e.g. `web` or `fix-ui.web`.
    pub name: String,
    pub port: u16,
    /// Owner process. Routes with an owner vanish when it exits and are never persisted.
    pub pid: Option<u32>,
    pub project: Option<String>,
    /// Command `doorman run` started, for display.
    #[serde(default)]
    pub command: Option<String>,
    /// Checkout the route was started from, when `doorman run` ran inside git.
    #[serde(default)]
    pub git: Option<GitContext>,
    pub created_at: DateTime<Utc>,
}

/// Where a route's app is checked out, so clients can group checkouts of one repo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitContext {
    /// Shared git directory; identical for the main checkout and every worktree.
    pub repo: String,
    /// Display name for the repository (its main checkout's folder).
    pub repo_name: String,
    /// Checked-out branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Root of this checkout.
    pub worktree: String,
    /// Whether this checkout is a linked worktree rather than the main checkout.
    pub linked: bool,
    /// Route name without the worktree's branch prefix, shared across checkouts.
    pub base_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrafficEntry {
    pub id: u64,
    pub route: String,
    /// Host the request arrived on, e.g. `web.test`. Absent in older daemons.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub scheme: Option<String>,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub request_bytes: Option<u64>,
    pub response_bytes: Option<u64>,
    pub captured_at: DateTime<Utc>,
}

/// A user-provided domain suffix and whether macOS knows to ask Doorman about it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Domain {
    pub name: String,
    pub resolver: ResolverStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolverStatus {
    /// `/etc/resolver/<domain>` points at the Doorman DNS responder.
    Installed,
    /// No resolver file exists yet.
    Missing,
    /// A resolver file exists but points somewhere else.
    Conflict,
}

/// Where the proxy is reachable. URLs prefer HTTPS when it is available.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoints {
    pub http_port: u16,
    pub https_port: Option<u16>,
}

impl Endpoints {
    pub fn url(&self, host: &str) -> String {
        match self.https_port {
            Some(443) => format!("https://{host}"),
            Some(port) => format!("https://{host}:{port}"),
            None => host_url(host, self.http_port),
        }
    }
}

/// Everything a client needs to render daemon state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Status {
    pub version: String,
    pub endpoints: Endpoints,
    pub dns_port: Option<u16>,
    pub domains: Vec<Domain>,
    pub wildcard: bool,
    pub ca_cert: Option<String>,
    pub ca_trusted: bool,
}

/// Settings and static routes the daemon persists across restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub wildcard: bool,
    #[serde(default)]
    pub routes: Vec<Route>,
}

impl Route {
    pub fn new(
        name: impl Into<String>,
        port: u16,
        pid: Option<u32>,
        project: Option<String>,
    ) -> Result<Self, ValidationError> {
        let name = name.into();
        validate_name(&name)?;
        validate_port(port)?;
        Ok(Self {
            name,
            port,
            pid,
            project,
            command: None,
            git: None,
            created_at: Utc::now(),
        })
    }

    pub fn hostname(&self) -> String {
        self.hostname_on(BUILTIN_DOMAIN)
    }

    pub fn hostname_on(&self, domain: &str) -> String {
        format!("{}.{domain}", self.name)
    }

    pub fn url(&self, proxy_port: u16) -> String {
        self.url_on(BUILTIN_DOMAIN, proxy_port)
    }

    pub fn url_on(&self, domain: &str, proxy_port: u16) -> String {
        host_url(&self.hostname_on(domain), proxy_port)
    }
}

pub fn host_url(host: &str, proxy_port: u16) -> String {
    if proxy_port == 80 {
        format!("http://{host}")
    } else {
        format!("http://{host}:{proxy_port}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    List,
    Traffic { route: Option<String>, limit: usize },
    ClearTraffic,
    Add { route: Route },
    Remove { name: String },
    Domains,
    AddDomain { name: String },
    RemoveDomain { name: String },
    SetWildcard { enabled: bool },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong {
        proxy_port: u16,
        version: String,
    },
    Status {
        status: Status,
    },
    Routes {
        routes: Vec<Route>,
        proxy_port: u16,
        #[serde(default)]
        https_port: Option<u16>,
    },
    Traffic {
        entries: Vec<TrafficEntry>,
    },
    TrafficCleared,
    Added {
        route: Route,
        proxy_port: u16,
    },
    Removed {
        name: String,
    },
    Domains {
        domains: Vec<Domain>,
        /// `None` when the DNS responder could not bind its port.
        dns_port: Option<u16>,
    },
    DomainAdded {
        domain: Domain,
        dns_port: Option<u16>,
    },
    DomainRemoved {
        name: String,
    },
    ShuttingDown,
    Error {
        message: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error(
        "name must be up to {MAX_NAME_LABELS} dot-separated labels of lowercase letters, digits, or interior hyphens"
    )]
    InvalidName,
    #[error("port must be greater than zero")]
    InvalidPort,
    #[error("domain must be dot-separated labels of lowercase letters, digits, or hyphens")]
    InvalidDomain,
    #[error(".localhost is built in and always available")]
    BuiltinDomain,
    #[error(".local is reserved for Bonjour (mDNS) and would break local network discovery")]
    MdnsDomain,
    #[error(
        ".{0} is a public top-level domain; use a private one like .test, or a subdomain you own like dev.acme.{0}"
    )]
    PublicDomain(String),
}

pub fn validate_name(name: &str) -> Result<(), ValidationError> {
    let labels: Vec<&str> = name.split('.').collect();
    if labels.len() <= MAX_NAME_LABELS && labels.iter().all(|label| is_dns_label(label)) {
        Ok(())
    } else {
        Err(ValidationError::InvalidName)
    }
}

pub fn validate_port(port: u16) -> Result<(), ValidationError> {
    if port == 0 {
        Err(ValidationError::InvalidPort)
    } else {
        Ok(())
    }
}

/// Normalizes user input like ".Test" into "test" and checks it is safe to route.
pub fn normalize_domain(input: &str) -> Result<String, ValidationError> {
    let domain = input.trim().trim_start_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = domain.split('.').collect();
    if domain.is_empty() || domain.len() > 200 || !labels.iter().all(|label| is_dns_label(label)) {
        return Err(ValidationError::InvalidDomain);
    }

    let tld = *labels.last().expect("split yields at least one label");
    if tld == BUILTIN_DOMAIN {
        return Err(ValidationError::BuiltinDomain);
    }
    if tld == "local" {
        return Err(ValidationError::MdnsDomain);
    }
    // A bare public TLD (e.g. "dev" or "com") would hijack every real site under it.
    // Subdomains of real domains (e.g. "dev.acme.com") only affect names the user owns.
    if labels.len() == 1 && !PRIVATE_TLDS.contains(&tld) {
        return Err(ValidationError::PublicDomain(tld.to_owned()));
    }
    Ok(domain)
}

fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Turns arbitrary text (a package name, a branch) into a single DNS label.
pub fn sanitize_label(input: &str) -> String {
    let input = input.rsplit('/').next().unwrap_or(input);
    let mut label = String::with_capacity(input.len());
    for character in input.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            label.push(character);
        } else if !label.ends_with('-') {
            label.push('-');
        }
    }
    let label = label.trim_matches('-');
    label[..label.len().min(63)]
        .trim_end_matches('-')
        .to_owned()
}

/// Splits a request host like `api.web.acme.test:3210` into `("api.web", "acme.test")`.
/// The longest matching domain wins, so `acme.test` beats `test`.
pub fn split_host<'a>(host: &'a str, domains: &[String]) -> Option<(&'a str, String)> {
    let host = strip_port(host);
    let mut candidates: Vec<&str> = std::iter::once(BUILTIN_DOMAIN)
        .chain(domains.iter().map(String::as_str))
        .collect();
    candidates.sort_by_key(|domain| std::cmp::Reverse(domain.len()));
    candidates.into_iter().find_map(|domain| {
        let name = host.strip_suffix(domain)?.strip_suffix('.')?;
        validate_name(name).ok()?;
        Some((name, domain.to_owned()))
    })
}

/// Kept for callers that only need the name.
pub fn route_name_for_host<'a>(host: &'a str, domains: &[String]) -> Option<&'a str> {
    split_host(host, domains).map(|(name, _)| name)
}

fn strip_port(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host,
    }
}

/// Candidate route names for wildcard fallback: `a.b.web` → `b.web`, `web`.
pub fn parent_names(name: &str) -> impl Iterator<Item = &str> {
    name.match_indices('.')
        .map(move |(index, _)| &name[index + 1..])
}

pub fn resolver_path(domain: &str) -> PathBuf {
    PathBuf::from(RESOLVER_DIR).join(domain)
}

pub fn resolver_contents(dns_port: u16) -> String {
    format!("# Added by Doorman\nnameserver 127.0.0.1\nport {dns_port}\n")
}

/// Shell command the user runs once so macOS sends lookups for `domain` to Doorman.
pub fn resolver_install_command(domain: &str, dns_port: u16) -> String {
    format!(
        "sudo mkdir -p {RESOLVER_DIR} && printf '{}' | sudo tee {} > /dev/null",
        resolver_contents(dns_port).replace('\n', "\\n"),
        resolver_path(domain).display()
    )
}

/// One-line shell script (run as root) that points `domains` at Doorman's DNS responder.
pub fn resolver_install_script(domains: &[&str], dns_port: u16) -> String {
    std::iter::once(format!("mkdir -p {RESOLVER_DIR}"))
        .chain(domains.iter().map(|domain| {
            format!(
                "printf '{}' > {}",
                resolver_contents(dns_port).replace('\n', "\\n"),
                resolver_path(domain).display()
            )
        }))
        .collect::<Vec<_>>()
        .join(" && ")
}

/// One-line shell script (run as root) that deletes the resolver files for `domains`.
pub fn resolver_remove_script(domains: &[&str]) -> String {
    let files: Vec<String> = domains
        .iter()
        .map(|domain| resolver_path(domain).display().to_string())
        .collect();
    format!("rm -f {}", files.join(" "))
}

pub fn resolver_remove_command(domain: &str) -> String {
    format!("sudo rm {}", resolver_path(domain).display())
}

pub fn resolver_status(domain: &str, dns_port: u16) -> ResolverStatus {
    match std::fs::read_to_string(resolver_path(domain)) {
        Ok(contents) => {
            let has_line = |expected: &[&str]| {
                contents
                    .lines()
                    .any(|line| line.split_whitespace().eq(expected.iter().copied()))
            };
            if has_line(&["nameserver", "127.0.0.1"]) && has_line(&["port", &dns_port.to_string()])
            {
                ResolverStatus::Installed
            } else {
                ResolverStatus::Conflict
            }
        }
        Err(_) => ResolverStatus::Missing,
    }
}

/// Whether macOS trusts the certificate at `path` for TLS (checked via the keychain).
pub fn certificate_trusted(path: &std::path::Path) -> bool {
    std::process::Command::new("/usr/bin/security")
        .args(["verify-cert", "-q", "-p", "ssl", "-c"])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("doorman")
}

pub fn socket_path() -> PathBuf {
    state_dir().join(SOCKET_FILE)
}

pub fn config_path() -> PathBuf {
    state_dir().join(CONFIG_FILE)
}

pub fn ca_cert_path() -> PathBuf {
    state_dir().join(CA_CERT_FILE)
}

pub fn ca_key_path() -> PathBuf {
    state_dir().join(CA_KEY_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_dns_safe_names() {
        for name in ["web", "api-v2", "a1", "api.web", "fix-ui.api.web"] {
            assert_eq!(validate_name(name), Ok(()));
        }
    }

    #[test]
    fn rejects_unsafe_names() {
        for name in [
            "",
            "Web",
            "api_v2",
            "-api",
            "api-",
            "a..b",
            ".web",
            "a.b.c.d.e",
        ] {
            assert_eq!(validate_name(name), Err(ValidationError::InvalidName));
        }
    }

    #[test]
    fn formats_urls() {
        let route = Route::new("web", 5173, None, None).unwrap();
        assert_eq!(route.url(3210), "http://web.localhost:3210");
        assert_eq!(route.url(80), "http://web.localhost");
        assert_eq!(route.url_on("acme.test", 3210), "http://web.acme.test:3210");

        let https = |port| Endpoints {
            http_port: 80,
            https_port: port,
        };
        assert_eq!(https(Some(443)).url("web.test"), "https://web.test");
        assert_eq!(https(Some(3443)).url("web.test"), "https://web.test:3443");
        assert_eq!(https(None).url("web.test"), "http://web.test");
    }

    #[test]
    fn normalizes_private_and_owned_domains() {
        assert_eq!(normalize_domain(".Test"), Ok("test".into()));
        assert_eq!(
            normalize_domain("acme.internal"),
            Ok("acme.internal".into())
        );
        assert_eq!(normalize_domain("dev.acme.com"), Ok("dev.acme.com".into()));
        assert_eq!(normalize_domain("myteam.dev"), Ok("myteam.dev".into()));
    }

    #[test]
    fn rejects_unsafe_domains() {
        assert_eq!(
            normalize_domain("localhost"),
            Err(ValidationError::BuiltinDomain)
        );
        assert_eq!(normalize_domain("local"), Err(ValidationError::MdnsDomain));
        assert_eq!(
            normalize_domain("acme.local"),
            Err(ValidationError::MdnsDomain)
        );
        assert_eq!(
            normalize_domain("dev"),
            Err(ValidationError::PublicDomain("dev".into()))
        );
        assert_eq!(
            normalize_domain("com"),
            Err(ValidationError::PublicDomain("com".into()))
        );
        for bad in ["", ".", "a..b", "under_score", "-x.test"] {
            assert_eq!(normalize_domain(bad), Err(ValidationError::InvalidDomain));
        }
    }

    #[test]
    fn splits_hosts_into_names_and_domains() {
        let domains = vec!["test".to_owned(), "acme.test".to_owned()];
        assert_eq!(
            split_host("web.localhost:3210", &domains),
            Some(("web", "localhost".into()))
        );
        assert_eq!(
            split_host("api.web.test", &domains),
            Some(("api.web", "test".into()))
        );
        assert_eq!(
            split_host("web.acme.test", &domains),
            Some(("web", "acme.test".into()))
        );
        assert_eq!(split_host("web.example", &domains), None);
        assert_eq!(split_host("test", &domains), None);
        assert_eq!(split_host("a.b.c.d.e.test", &domains), None);
    }

    #[test]
    fn lists_parent_names() {
        assert_eq!(
            parent_names("a.b.web").collect::<Vec<_>>(),
            ["b.web", "web"]
        );
        assert_eq!(parent_names("web").count(), 0);
    }

    #[test]
    fn builds_resolver_scripts() {
        assert_eq!(
            resolver_install_script(&["test", "acme.internal"], 53535),
            "mkdir -p /etc/resolver && \
             printf '# Added by Doorman\\nnameserver 127.0.0.1\\nport 53535\\n' > /etc/resolver/test && \
             printf '# Added by Doorman\\nnameserver 127.0.0.1\\nport 53535\\n' > /etc/resolver/acme.internal"
        );
        assert_eq!(
            resolver_remove_script(&["test", "acme.internal"]),
            "rm -f /etc/resolver/test /etc/resolver/acme.internal"
        );
    }

    #[test]
    fn sanitizes_labels() {
        assert_eq!(sanitize_label("@acme/Web_App"), "web-app");
        assert_eq!(sanitize_label("feature/Fix UI!!"), "fix-ui");
        assert_eq!(sanitize_label("--x--"), "x");
    }
}
