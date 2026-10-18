use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod tunnel;

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
pub fn certificate_trusted(path: &Path) -> bool {
    std::process::Command::new("/usr/bin/security")
        .args(["verify-cert", "-q", "-p", "ssl", "-c"])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Adds a certificate to the user's macOS login keychain.
///
/// The boolean is false when macOS rejected or cancelled the operation; errors
/// are reserved for failing to start or wait for the `security` command.
pub fn add_trusted_certificate(path: &Path) -> std::io::Result<bool> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let keychain = home.join("Library/Keychains/login.keychain-db");
    Ok(std::process::Command::new("/usr/bin/security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-k"])
        .arg(keychain)
        .arg(path)
        .status()?
        .success())
}

const FIREFOX_PREF_NAME: &str = "security.enterprise_roots.enabled";
const FIREFOX_PREF_START: &str = "// Doorman: begin Firefox trust";
const FIREFOX_PREF_END: &str = "// Doorman: end Firefox trust";
const FIREFOX_PREF: &str = r#"user_pref("security.enterprise_roots.enabled", true);"#;

/// Whether a Firefox profile reads trusted roots from the macOS keychain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirefoxTrust {
    /// Never set in this profile, so Firefox's built-in default applies.
    Default,
    On,
    Off,
}

pub struct FirefoxProfile {
    pub path: PathBuf,
    pub trust: FirefoxTrust,
}

/// Every Firefox profile for this user, with its current keychain-trust setting.
pub fn firefox_profiles() -> Vec<FirefoxProfile> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    firefox_profile_paths(&home)
        .into_iter()
        .map(|path| FirefoxProfile {
            trust: firefox_profile_trust(&path),
            path,
        })
        .collect()
}

/// Turns keychain trust on in every profile that has it switched off, by adding a
/// marked block to `user.js`. Returns how many profiles changed.
///
/// Firefox rewrites `prefs.js` on exit, so `user.js` is the only file that is safe
/// to edit while Firefox is running.
pub fn enable_firefox_trust() -> std::io::Result<usize> {
    let mut changed = 0;
    for profile in firefox_profiles() {
        if profile.trust != FirefoxTrust::Off {
            continue;
        }
        let path = profile.path.join("user.js");
        let current = fs::read_to_string(&path).unwrap_or_default();
        fs::write(&path, add_firefox_trust_block(&current))?;
        changed += 1;
    }
    Ok(changed)
}

fn firefox_profile_trust(profile: &Path) -> FirefoxTrust {
    let read = |name: &str| {
        fs::read_to_string(profile.join(name))
            .ok()
            .and_then(|contents| firefox_pref_value(&contents))
    };
    // Firefox applies user.js on top of prefs.js at startup, so it wins.
    match read("user.js").or_else(|| read("prefs.js")) {
        Some(true) => FirefoxTrust::On,
        Some(false) => FirefoxTrust::Off,
        None => FirefoxTrust::Default,
    }
}

/// The value of the last `user_pref("security.enterprise_roots.enabled", ...);` line.
/// Later lines override earlier ones, so only the last one counts.
fn firefox_pref_value(contents: &str) -> Option<bool> {
    contents.lines().rev().find_map(|line| {
        let rest = line.trim().strip_prefix("user_pref(")?.trim_start();
        let rest = rest.strip_prefix('"')?.strip_prefix(FIREFOX_PREF_NAME)?;
        let value = rest.strip_prefix('"')?.trim_start().strip_prefix(',')?;
        let value = value
            .trim()
            .strip_suffix(';')?
            .trim_end()
            .strip_suffix(')')?;
        match value.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    })
}

/// Removes the preference block written by [`enable_firefox_trust`].
/// Existing Firefox preferences are left untouched.
pub fn remove_firefox_trust() -> std::io::Result<()> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(());
    };
    for profile in firefox_profile_paths(&home) {
        let path = profile.join("user.js");
        let Ok(current) = fs::read_to_string(&path) else {
            continue;
        };
        let updated = remove_firefox_trust_block(&current);
        if updated != current {
            if updated.trim().is_empty() {
                fs::remove_file(path)?;
            } else {
                fs::write(path, updated)?;
            }
        }
    }
    Ok(())
}

fn firefox_profile_paths(home: &Path) -> Vec<PathBuf> {
    let firefox_dir = home.join("Library/Application Support/Firefox");
    let profiles_dir = firefox_dir.join("Profiles");
    let mut profiles = fs::read_dir(&profiles_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();

    // profiles.ini also supports profiles stored outside the default Profiles
    // directory. Include those without relying on an INI parser dependency.
    if let Ok(contents) = fs::read_to_string(firefox_dir.join("profiles.ini")) {
        let mut relative = true;
        let mut profile = None;
        let mut add_profile = |profile: &mut Option<PathBuf>, relative: bool| {
            let Some(path) = profile.take() else {
                return;
            };
            let path = if relative {
                firefox_dir.join(path)
            } else {
                path
            };
            if path.is_dir() {
                profiles.push(path);
            }
        };
        for line in contents.lines().chain(std::iter::once("[end]")) {
            let line = line.trim();
            if line.starts_with('[') {
                add_profile(&mut profile, relative);
                relative = true;
            } else if let Some(value) = line.strip_prefix("IsRelative=") {
                relative = value.trim() != "0";
            } else if let Some(value) = line.strip_prefix("Path=") {
                profile = Some(PathBuf::from(value.trim()));
            }
        }
    }

    profiles.sort();
    profiles.dedup();
    profiles
}

fn add_firefox_trust_block(contents: &str) -> String {
    // Drop any earlier Doorman block so the new one is the last word in the file.
    let mut updated = remove_firefox_trust_block(contents);
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&format!(
        "{FIREFOX_PREF_START}\n{FIREFOX_PREF}\n{FIREFOX_PREF_END}\n"
    ));
    updated
}

fn remove_firefox_trust_block(contents: &str) -> String {
    let mut updated = Vec::new();
    let mut managed = false;
    let mut found = false;
    for line in contents.lines() {
        if line.trim() == FIREFOX_PREF_START {
            managed = true;
            found = true;
            continue;
        }
        if managed {
            if line.trim() == FIREFOX_PREF_END {
                managed = false;
            }
            continue;
        }
        updated.push(line);
    }
    if !found {
        return contents.to_owned();
    }
    if updated.is_empty() {
        String::new()
    } else {
        format!("{}\n", updated.join("\n"))
    }
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

    #[test]
    fn adds_and_removes_only_the_managed_firefox_preference() {
        let original = "user_pref(\"browser.startup.page\", 1);\n";
        let configured = add_firefox_trust_block(original);
        assert!(configured.contains(FIREFOX_PREF));
        assert!(configured.starts_with(original));

        let restored = remove_firefox_trust_block(&configured);
        assert_eq!(restored, original);
    }

    #[test]
    fn reads_the_last_firefox_preference_line() {
        let on = "user_pref(\"security.enterprise_roots.enabled\", true);\n";
        let off = "user_pref(\"security.enterprise_roots.enabled\", false);\n";
        assert_eq!(firefox_pref_value("user_pref(\"browser.startup.page\", 1);\n"), None);
        assert_eq!(firefox_pref_value(on), Some(true));
        assert_eq!(firefox_pref_value(&format!("{on}{off}")), Some(false));
        assert_eq!(firefox_pref_value(&format!("// {on}")), None);
    }

    #[test]
    fn firefox_block_overrides_an_earlier_false_and_is_not_duplicated() {
        let off = "user_pref(\"security.enterprise_roots.enabled\", false);\n";
        let once = add_firefox_trust_block(off);
        assert_eq!(firefox_pref_value(&once), Some(true));
        assert_eq!(add_firefox_trust_block(&once), once);
        assert_eq!(remove_firefox_trust_block(&once), off);
        assert_eq!(remove_firefox_trust_block(&add_firefox_trust_block("")), "");
    }

    #[test]
    fn reads_firefox_trust_from_a_profile_directory() {
        let dir = std::env::temp_dir().join(format!("doorman-ff-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(firefox_profile_trust(&dir), FirefoxTrust::Default);
        fs::write(
            dir.join("prefs.js"),
            "user_pref(\"security.enterprise_roots.enabled\", false);\n",
        )
        .unwrap();
        assert_eq!(firefox_profile_trust(&dir), FirefoxTrust::Off);
        fs::write(dir.join("user.js"), add_firefox_trust_block("")).unwrap();
        assert_eq!(firefox_profile_trust(&dir), FirefoxTrust::On);
        fs::write(
            dir.join("user.js"),
            "user_pref(\"security.enterprise_roots.enabled\", true);\nuser_pref(\"security.enterprise_roots.enabled\", false);\n",
        )
        .unwrap();
        assert_eq!(firefox_profile_trust(&dir), FirefoxTrust::Off);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_relative_and_absolute_firefox_profiles() {
        let home = std::env::temp_dir().join(format!("doorman-ff-paths-{}", std::process::id()));
        let firefox = home.join("Library/Application Support/Firefox");
        let relative = firefox.join("Profiles/relative");
        let absolute = std::env::temp_dir().join(format!("doorman-ff-absolute-{}", std::process::id()));
        fs::create_dir_all(&relative).unwrap();
        fs::create_dir_all(&absolute).unwrap();
        fs::write(
            firefox.join("profiles.ini"),
            format!(
                "[Profile0]\nIsRelative=1\nPath=Profiles/relative\n[Profile1]\nIsRelative=0\nPath={}\n",
                absolute.display()
            ),
        )
        .unwrap();

        let profiles = firefox_profile_paths(&home);
        assert_eq!(profiles, vec![absolute.clone(), relative]);

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(absolute).unwrap();
    }
}
