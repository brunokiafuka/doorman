mod client;
mod run;
mod system;
mod tunnel;

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use doorman_core::{
    BUILTIN_DOMAIN, Request, ResolverStatus, Response, Route, Status, resolver_remove_command,
};

use crate::client::{call, unexpected};

#[derive(Debug, Parser)]
#[command(
    name = "doorman",
    version,
    about = "Friendly HTTPS names for local apps",
    after_help = "Run `doorman` with no arguments to start your package.json \"dev\" script,\n\
                  or `doorman <name> <command…>` as a shorthand for `doorman run --name`."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a dev server on a free port behind https://<name>.localhost.
    Run {
        /// Route name; inferred from doorman.json, package.json, or the folder.
        #[arg(long)]
        name: Option<String>,
        /// The command to run; defaults to the package.json "dev" script.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Register a fixed route to a port that's already running (persists across restarts).
    #[command(alias = "alias")]
    Add {
        /// DNS-safe name, optionally nested (e.g. `api.shop`).
        name: String,
        /// Port the local server is listening on.
        port: u16,
        /// Optional project directory shown in the app and error pages.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Optional owner PID. The route expires when this process exits.
        #[arg(long)]
        pid: Option<u32>,
    },
    /// List registered routes.
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print the URL for a route.
    Get { name: String },
    /// Publicly expose a running route using an installed cloudflared or ngrok.
    Tunnel {
        /// Existing route name (see `doorman list`).
        name: String,
        /// Auto prefers cloudflared when both providers are installed.
        #[arg(long, value_enum, default_value_t = tunnel::Provider::Auto)]
        provider: tunnel::Provider,
    },
    /// Remove a route.
    #[command(alias = "rm")]
    Remove { name: String },
    /// Show recently proxied requests (headers and bodies are never captured).
    Traffic {
        #[arg(long)]
        route: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Forget all captured request metadata.
    ClearTraffic,
    /// Manage custom domains like .test in addition to the built-in .localhost.
    #[command(subcommand)]
    Domain(DomainCommand),
    /// Send unknown subdomains (e.g. tenant.web.localhost) to their parent route.
    Wildcard {
        #[arg(value_parser = ["on", "off"])]
        state: Option<String>,
    },
    /// One-time setup: trust HTTPS and finish every custom domain.
    Setup,
    /// Trust Doorman's local certificate authority in your login keychain.
    Trust,
    /// Start Doorman at login.
    #[command(subcommand)]
    Service(ServiceCommand),
    /// Start the daemon in the background.
    Start,
    /// Stop the daemon.
    Stop,
    /// Restart the daemon (e.g. after freeing port 443).
    Restart,
    /// Check the daemon, ports, HTTPS trust, domains, and every route.
    Doctor,
    /// Remove all Doorman state, certificate trust, and the login service.
    Clean,
    #[command(external_subcommand)]
    Shorthand(Vec<String>),
}

#[derive(Debug, Subcommand)]
enum DomainCommand {
    #[command(alias = "ls")]
    List,
    /// Add a domain suffix, e.g. `test` or `dev.acme.com`, and point macOS at Doorman.
    Add {
        name: String,
        /// Only register the domain; run `doorman domain setup` later.
        #[arg(long)]
        no_setup: bool,
    },
    /// Finish setup for every domain that doesn't resolve yet (one password prompt).
    Setup,
    /// Remove a domain suffix and its resolver file.
    #[command(alias = "rm")]
    Remove { name: String },
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    Install,
    Uninstall,
    Status,
}

fn main() -> ExitCode {
    // Exit quietly when piped into `head` and friends instead of panicking.
    // SAFETY: restoring the default SIGPIPE disposition has no preconditions.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    match dispatch(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("\x1b[31merror:\x1b[0m {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: Cli) -> Result<ExitCode> {
    let Some(command) = cli.command else {
        return run::run(run::RunOptions {
            name: None,
            command: Vec::new(),
        });
    };
    match command {
        Command::Run { name, command } => return run::run(run::RunOptions { name, command }),
        Command::Shorthand(mut words) => {
            if words.len() < 2 {
                bail!(
                    "unknown command '{}'. Did you mean `doorman run --name {0} <command>`?",
                    words.first().map_or("", String::as_str)
                );
            }
            let name = words.remove(0);
            return run::run(run::RunOptions {
                name: Some(name),
                command: words,
            });
        }
        Command::Add {
            name,
            port,
            project,
            pid,
        } => add(name, port, project, pid)?,
        Command::List { json } => list(json)?,
        Command::Tunnel { name, provider } => return tunnel::run(&name, provider),
        Command::Get { name } => {
            let status = client::status()?;
            println!(
                "{}",
                status.endpoints.url(&format!("{name}.{BUILTIN_DOMAIN}"))
            );
        }
        Command::Remove { name } => match call(&Request::Remove { name })? {
            Response::Removed { name } => println!("Removed {name}"),
            other => return Err(unexpected(other)),
        },
        Command::Traffic { route, limit, json } => traffic(route, limit, json)?,
        Command::ClearTraffic => match call(&Request::ClearTraffic)? {
            Response::TrafficCleared => println!("Cleared captured traffic metadata."),
            other => return Err(unexpected(other)),
        },
        Command::Domain(command) => domain(command)?,
        Command::Wildcard { state } => wildcard(state)?,
        Command::Setup => system::setup()?,
        Command::Trust => system::trust()?,
        Command::Service(ServiceCommand::Install) => system::service_install()?,
        Command::Service(ServiceCommand::Uninstall) => system::service_uninstall()?,
        Command::Service(ServiceCommand::Status) => system::service_status()?,
        Command::Start => {
            client::ensure_running()?;
            print_endpoints(&client::status()?);
        }
        Command::Stop => {
            if system::stop_daemon() {
                println!("Stopped Doorman");
            } else {
                println!("Doorman wasn't running");
            }
        }
        Command::Restart => {
            system::stop_daemon();
            // A login service restarts the daemon by itself.
            for _ in 0..40 {
                if client::is_running() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            client::ensure_running()?;
            print_endpoints(&client::status()?);
        }
        Command::Doctor => doctor()?,
        Command::Clean => system::clean()?,
    }
    Ok(ExitCode::SUCCESS)
}

fn add(name: String, port: u16, project: Option<PathBuf>, pid: Option<u32>) -> Result<()> {
    client::ensure_running()?;
    let project = project.map(|path| {
        path.canonicalize()
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    });
    let route = Route::new(name, port, pid, project)?;
    match call(&Request::Add { route })? {
        Response::Added { route, .. } => {
            let status = client::status()?;
            println!("Added {}", status.endpoints.url(&route.hostname()));
            println!("  → 127.0.0.1:{}", route.port);
        }
        other => return Err(unexpected(other)),
    }
    Ok(())
}

fn list(json: bool) -> Result<()> {
    let Response::Routes { routes, .. } = call(&Request::List)? else {
        bail!("unexpected daemon response");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&routes)?);
        return Ok(());
    }
    if routes.is_empty() {
        println!("No routes yet. In a project, run: doorman run");
        return Ok(());
    }
    let status = client::status()?;
    for route in routes {
        let marker = if app_listening(route.port) {
            "\x1b[32m●\x1b[0m"
        } else {
            "\x1b[33m●\x1b[0m"
        };
        let owner = match (&route.command, route.pid) {
            (Some(command), _) => format!("  \x1b[2m{command}\x1b[0m"),
            (None, Some(pid)) => format!("  \x1b[2mpid {pid}\x1b[0m"),
            (None, None) => String::new(),
        };
        let branch = route
            .git
            .as_ref()
            .and_then(|git| {
                let branch = git.branch.as_deref()?;
                let mark = if git.linked { "⑂ " } else { "" };
                Some(format!("  \x1b[35m{mark}{branch}\x1b[0m"))
            })
            .unwrap_or_default();
        println!(
            "{marker} {:<40} → 127.0.0.1:{}{branch}{owner}",
            status.endpoints.url(&route.hostname()),
            route.port
        );
        for domain in &status.domains {
            println!(
                "  {}",
                status.endpoints.url(&route.hostname_on(&domain.name))
            );
        }
    }
    Ok(())
}

fn traffic(route: Option<String>, limit: usize, json: bool) -> Result<()> {
    let Response::Traffic { entries } = call(&Request::Traffic { route, limit })? else {
        bail!("unexpected daemon response");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("No traffic captured yet.");
    }
    if json {
        return Ok(());
    }
    for entry in entries {
        println!(
            "{} {:<7} {:<3} {:>5}ms  {}{}",
            entry
                .captured_at
                .with_timezone(&chrono::Local)
                .format("%H:%M:%S"),
            entry.method,
            entry.status,
            entry.duration_ms,
            entry
                .host
                .clone()
                .unwrap_or_else(|| format!("{}.{BUILTIN_DOMAIN}", entry.route)),
            entry.path
        );
    }
    Ok(())
}

fn domain(command: DomainCommand) -> Result<()> {
    client::ensure_running()?;
    match command {
        DomainCommand::List => match call(&Request::Domains)? {
            Response::Domains { domains, .. } => {
                println!("✓ .localhost              built in");
                for domain in &domains {
                    let (marker, note) = match domain.resolver {
                        ResolverStatus::Installed => ("✓", "ready"),
                        ResolverStatus::Missing => ("!", "needs `doorman domain setup`"),
                        ResolverStatus::Conflict => (
                            "!",
                            "resolver file points elsewhere; run `doorman domain setup`",
                        ),
                    };
                    println!("{marker} .{:<24} {note}", domain.name);
                }
            }
            other => return Err(unexpected(other)),
        },
        DomainCommand::Add { name, no_setup } => match call(&Request::AddDomain { name })? {
            Response::DomainAdded { domain, dns_port } => {
                println!("Added .{}", domain.name);
                if domain.resolver == ResolverStatus::Installed {
                    println!("✓ .{} already resolves through Doorman", domain.name);
                } else if no_setup || !system::interactive() {
                    println!("Run `doorman domain setup` to finish.");
                } else {
                    system::install_resolvers(&[domain.name.as_str()], dns_port)?;
                }
            }
            other => return Err(unexpected(other)),
        },
        DomainCommand::Setup => system::setup_domains()?,
        DomainCommand::Remove { name } => match call(&Request::RemoveDomain { name })? {
            Response::DomainRemoved { name } => {
                println!("Removed .{name}");
                if doorman_core::resolver_path(&name).exists() {
                    if system::interactive() {
                        system::remove_resolvers(&[name.as_str()])?;
                    } else {
                        println!(
                            "Delete its resolver file with: {}",
                            resolver_remove_command(&name)
                        );
                    }
                }
            }
            other => return Err(unexpected(other)),
        },
    }
    Ok(())
}

fn wildcard(state: Option<String>) -> Result<()> {
    let status = match state.as_deref() {
        None => client::status()?,
        Some(state) => match call(&Request::SetWildcard {
            enabled: state == "on",
        })? {
            Response::Status { status } => status,
            other => return Err(unexpected(other)),
        },
    };
    println!(
        "Wildcard subdomains are {}",
        if status.wildcard {
            "on: tenant.web.localhost falls back to the web route"
        } else {
            "off"
        }
    );
    Ok(())
}

fn print_endpoints(status: &Status) {
    println!("Doorman {} is running", status.version);
    if let Some(port) = status.endpoints.https_port {
        println!("  https port {port}");
    }
    println!("  http  port {}", status.endpoints.http_port);
}

fn app_listening(port: u16) -> bool {
    [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ]
    .into_iter()
    .any(|ip| TcpStream::connect_timeout(&(ip, port).into(), Duration::from_millis(200)).is_ok())
}

fn resolves_to_loopback(host: &str) -> bool {
    (host, 80)
        .to_socket_addrs()
        .is_ok_and(|mut addresses| addresses.any(|address| address.ip().is_loopback()))
}

fn doctor() -> Result<()> {
    let status = client::status()?;
    println!("✓ daemon  {}", status.version);

    let endpoints = status.endpoints;
    match endpoints.https_port {
        Some(443) => println!("✓ https   port 443"),
        Some(port) => println!(
            "! https   port {port}, so URLs include :{port} (443 is taken, or DOORMAN_HTTPS_PORT is set)"
        ),
        None => println!("! https   off"),
    }
    println!(
        "{} http    port {}",
        if endpoints.http_port == 80 {
            "✓"
        } else {
            "·"
        },
        endpoints.http_port
    );
    if status.ca_cert.is_some() {
        if status.ca_trusted {
            println!("✓ trust   local certificate authority is trusted");
        } else {
            println!("! trust   browsers will warn; run `doorman trust`");
        }
    }
    match status.dns_port {
        Some(port) => println!("✓ dns     127.0.0.1:{port}"),
        None if status.domains.is_empty() => {}
        None => println!("! dns     responder isn't running; custom domains can't resolve"),
    }
    for domain in &status.domains {
        if resolves_to_loopback(&format!("doorman-probe.{}", domain.name)) {
            println!("✓ domain  .{}", domain.name);
        } else {
            println!(
                "! domain  .{} doesn't resolve yet; run `doorman domain setup`",
                domain.name
            );
        }
    }
    println!("· wildcard {}", if status.wildcard { "on" } else { "off" });

    if let Response::Routes { routes, .. } = call(&Request::List)? {
        if routes.is_empty() {
            println!("· routes  none registered");
        }
        for route in routes {
            let marker = if app_listening(route.port) {
                "✓"
            } else {
                "!"
            };
            println!(
                "{marker} route   {:<24} 127.0.0.1:{}",
                route.name, route.port
            );
        }
    }
    Ok(())
}
