//! Machine-level setup: certificate trust, resolver files, login service, cleanup.
//! Anything needing admin rights runs in the foreground so the user sees the prompt.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use doorman_core::{
    CA_COMMON_NAME, Request, ResolverStatus, Response, ca_cert_path, certificate_trusted,
    configure_firefox_trust, remove_firefox_trust, resolver_install_script, resolver_path,
    state_dir,
};

use crate::client;

const SERVICE_LABEL: &str = "dev.doorman.daemon";

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Browser {
    All,
    Firefox,
    Edge,
}

fn login_keychain() -> PathBuf {
    home().join("Library/Keychains/login.keychain-db")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn trust(browser: Browser) -> Result<()> {
    client::ensure_running()?;
    let status = client::status()?;
    let Some(ca) = status.ca_cert else {
        bail!("HTTPS is disabled for this daemon (DOORMAN_HTTPS=0), so there is nothing to trust");
    };
    if certificate_trusted(ca.as_ref()) {
        println!("✓ {CA_COMMON_NAME} is already trusted");
        print_browser_status(browser)?;
        return Ok(());
    }
    println!("Adding {CA_COMMON_NAME} to your login keychain. macOS will ask you to confirm.");
    let ok = Command::new("/usr/bin/security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-k"])
        .arg(login_keychain())
        .arg(&ca)
        .status()
        .context("run security add-trusted-cert")?
        .success();
    if !ok || !certificate_trusted(ca.as_ref()) {
        bail!("the certificate wasn't trusted; run `doorman trust` again and approve the prompt");
    }
    println!(
        "✓ macOS trust is enabled for Chrome, Microsoft Edge, Safari, and Node (via NODE_EXTRA_CA_CERTS)"
    );
    print_browser_status(browser)?;
    Ok(())
}

fn print_browser_status(browser: Browser) -> Result<()> {
    match browser {
        Browser::All | Browser::Firefox => {
            let profiles = configure_firefox_trust()
                .context("configure Firefox to trust Doorman's local certificate authority")?;
            print_firefox_status(profiles);
        }
        Browser::Edge => println!(
            "✓ Microsoft Edge uses the macOS login keychain; restart open Edge tabs"
        ),
    }
    if matches!(browser, Browser::All) {
        println!("✓ Microsoft Edge uses the macOS login keychain; restart open Edge tabs");
    }
    Ok(())
}

fn print_firefox_status(profiles: usize) {
    if profiles == 0 {
        println!(
            "  Firefox: no profile found; enable security.enterprise_roots.enabled in about:config"
        );
    } else {
        println!(
            "✓ Firefox trust enabled for {profiles} profile{}; restart Firefox tabs",
            if profiles == 1 { "" } else { "s" }
        );
    }
}

/// One stop: trust the CA, then install every missing resolver file with one sudo prompt.
pub fn setup(browser: Browser) -> Result<()> {
    client::ensure_running()?;
    trust(browser)?;

    setup_domains()?;

    let endpoints = client::status()?.endpoints;
    match endpoints.https_port {
        Some(443) => println!("✓ Clean URLs: https://<name>.localhost"),
        Some(port) => println!(
            "! Port 443 is held by another process, so URLs include :{port}. Stop it and run `doorman restart`."
        ),
        None => println!(
            "! HTTPS is off; URLs use http:// on port {}",
            endpoints.http_port
        ),
    }
    Ok(())
}

/// Points every custom domain that isn't set up yet at Doorman's DNS responder.
pub fn setup_domains() -> Result<()> {
    client::ensure_running()?;
    let status = client::status()?;
    let pending: Vec<&str> = status
        .domains
        .iter()
        .filter(|domain| domain.resolver != ResolverStatus::Installed)
        .map(|domain| domain.name.as_str())
        .collect();
    if pending.is_empty() {
        if !status.domains.is_empty() {
            println!("✓ Every custom domain resolves through Doorman");
        }
        return Ok(());
    }
    install_resolvers(&pending, status.dns_port)
}

/// Writes `/etc/resolver/<domain>` for each domain with a single sudo prompt.
pub fn install_resolvers(domains: &[&str], dns_port: Option<u16>) -> Result<()> {
    let Some(dns_port) = dns_port else {
        bail!(
            "Doorman's DNS responder isn't running (its port is taken); set DOORMAN_DNS_PORT and run `doorman restart`"
        );
    };
    let script = resolver_install_script(domains, dns_port);
    println!(
        "Pointing {} at Doorman. This writes to /etc/resolver, so macOS asks for your password.",
        dotted(domains)
    );
    run_as_admin(&script, "resolver setup was cancelled")?;
    println!("✓ {} now resolves through Doorman", dotted(domains));
    Ok(())
}

/// Deletes resolver files that exist, with a single sudo prompt.
pub fn remove_resolvers(domains: &[&str]) -> Result<()> {
    let files: Vec<String> = domains
        .iter()
        .map(|domain| resolver_path(domain))
        .filter(|path| path.exists())
        .map(|path| path.display().to_string())
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    println!("Removing {} (needs your password).", files.join(", "));
    run_as_admin(
        &format!("rm -f {}", files.join(" ")),
        "resolver cleanup was cancelled",
    )
}

/// Whether a password prompt can reach the user.
pub fn interactive() -> bool {
    // SAFETY: isatty has no preconditions.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

fn run_as_admin(script: &str, cancelled: &str) -> Result<()> {
    let ok = Command::new("sudo")
        .args(["/bin/sh", "-c", script])
        .status()
        .context("run sudo")?
        .success();
    if !ok {
        bail!("{cancelled}");
    }
    Ok(())
}

fn dotted(domains: &[&str]) -> String {
    domains
        .iter()
        .map(|domain| format!(".{domain}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn plist_path() -> PathBuf {
    home().join(format!("Library/LaunchAgents/{SERVICE_LABEL}.plist"))
}

fn launchd_domain() -> String {
    // SAFETY: getuid has no preconditions.
    format!("gui/{}", unsafe { libc::getuid() })
}

pub fn service_install() -> Result<()> {
    let daemon = client::daemon_path().context("can't find doorman-daemon to install")?;
    let log = state_dir().join("daemon.log");
    std::fs::create_dir_all(state_dir())?;
    let environment: String = std::env::vars()
        .filter(|(key, _)| key.starts_with("DOORMAN_") && key != "DOORMAN_DAEMON_PATH")
        .map(|(key, value)| {
            format!(
                "    <key>{}</key><string>{}</string>\n",
                xml(&key),
                xml(&value)
            )
        })
        .collect();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{SERVICE_LABEL}</string>
  <key>ProgramArguments</key><array><string>{daemon}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>EnvironmentVariables</key>
  <dict>
{environment}  </dict>
</dict>
</plist>
"#,
        daemon = xml(&daemon.display().to_string()),
        log = xml(&log.display().to_string()),
    );
    std::fs::create_dir_all(plist_path().parent().expect("LaunchAgents parent"))?;
    std::fs::write(plist_path(), plist)?;

    // Hand over from a manually started daemon to the launchd-managed one.
    stop_daemon();
    let _ = launchctl(&["bootout", &format!("{}/{SERVICE_LABEL}", launchd_domain())]);
    if !launchctl(&[
        "bootstrap",
        &launchd_domain(),
        &plist_path().display().to_string(),
    ])? {
        bail!("launchctl couldn't load {}", plist_path().display());
    }
    for _ in 0..60 {
        if client::is_running() {
            println!("✓ Doorman starts at login ({})", plist_path().display());
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    bail!(
        "the service loaded but the daemon isn't answering; see {}",
        log.display()
    )
}

pub fn service_uninstall() -> Result<()> {
    let loaded = launchctl(&["bootout", &format!("{}/{SERVICE_LABEL}", launchd_domain())])?;
    let existed = plist_path().exists();
    if existed {
        std::fs::remove_file(plist_path())?;
    }
    if loaded || existed {
        println!("✓ Doorman no longer starts at login");
    } else {
        println!("Doorman wasn't installed as a login service");
    }
    Ok(())
}

pub fn service_status() -> Result<()> {
    let installed = plist_path().exists();
    let loaded = Command::new("launchctl")
        .args(["print", &format!("{}/{SERVICE_LABEL}", launchd_domain())])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    println!(
        "{} login service {}",
        if installed && loaded { "✓" } else { "·" },
        match (installed, loaded) {
            (true, true) => "installed and running",
            (true, false) => "installed but not loaded; run `doorman service install`",
            (false, _) => "not installed; run `doorman service install`",
        }
    );
    Ok(())
}

fn launchctl(args: &[&str]) -> Result<bool> {
    Ok(Command::new("launchctl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run launchctl")?
        .success())
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Asks a running daemon to exit; returns whether one was running.
pub fn stop_daemon() -> bool {
    if !client::is_running() {
        return false;
    }
    let _ = client::call(&Request::Shutdown);
    for _ in 0..40 {
        if !client::is_running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    true
}

/// Removes everything Doorman created for this user.
pub fn clean() -> Result<()> {
    let domains = match client::call(&Request::Domains) {
        Ok(Response::Domains { domains, .. }) => domains,
        _ => Vec::new(),
    };
    service_uninstall()?;
    if stop_daemon() {
        println!("✓ Stopped the daemon");
    }

    let ca = ca_cert_path();
    if ca.exists() {
        let _ = Command::new("/usr/bin/security")
            .args(["remove-trusted-cert"])
            .arg(&ca)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Delete every copy the keychain holds; stops when none are left.
        for _ in 0..10 {
            let deleted = Command::new("/usr/bin/security")
                .args(["delete-certificate", "-c", CA_COMMON_NAME])
                .arg(login_keychain())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !deleted {
                break;
            }
        }
        println!("✓ Removed {CA_COMMON_NAME} from your keychain");
    }

    remove_firefox_trust().context("remove Doorman's Firefox trust setting")?;

    if state_dir().exists() {
        std::fs::remove_dir_all(state_dir())?;
        println!("✓ Deleted {}", state_dir().display());
    }

    let names: Vec<&str> = domains.iter().map(|domain| domain.name.as_str()).collect();
    if interactive() {
        remove_resolvers(&names)?;
    }
    Ok(())
}
