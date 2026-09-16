mod control;
mod dns;
mod pages;
mod proxy;
mod state;
mod tls;

use std::{
    net::{Ipv4Addr, Ipv6Addr},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::{Context, Result};
use doorman_core::{
    DEFAULT_DNS_PORT, DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT, Endpoints, FALLBACK_HTTP_PORT,
    FALLBACK_HTTPS_PORT, ca_cert_path, certificate_trusted, socket_path, state_dir,
};
use tokio::{
    net::{TcpListener, UdpSocket, UnixListener, UnixStream},
    signal,
    task::JoinHandle,
};
use tokio_rustls::TlsAcceptor;

use crate::state::App;

#[tokio::main]
async fn main() -> Result<()> {
    std::fs::create_dir_all(state_dir()).context("create Doorman state directory")?;
    set_permissions(&state_dir(), 0o700)?;

    let socket = socket_path();
    if socket.exists() {
        if UnixStream::connect(&socket).await.is_ok() {
            // Exit cleanly so launchd's KeepAlive doesn't keep retrying.
            println!("Doorman is already running at {}", socket.display());
            return Ok(());
        }
        std::fs::remove_file(&socket).context("remove stale Doorman control socket")?;
    }
    let control = UnixListener::bind(&socket).context("bind Doorman control socket")?;
    set_permissions(&socket, 0o600)?;

    // DOORMAN_PROXY_PORT is the older name for the HTTP port.
    let http_env = env_port("DOORMAN_HTTP_PORT").or_else(|| env_port("DOORMAN_PROXY_PORT"));
    let (http_port, http) = bind_proxy(http_env, DEFAULT_HTTP_PORT, FALLBACK_HTTP_PORT)
        .await
        .context("bind HTTP proxy")?;
    let tls_enabled = std::env::var("DOORMAN_HTTPS").as_deref() != Ok("0");
    let https = if tls_enabled {
        match bind_proxy(
            env_port("DOORMAN_HTTPS_PORT"),
            DEFAULT_HTTPS_PORT,
            FALLBACK_HTTPS_PORT,
        )
        .await
        {
            Ok(bound) => Some(bound),
            Err(error) => {
                eprintln!("HTTPS disabled: {error:#}");
                None
            }
        }
    } else {
        None
    };

    // DNS is optional: without it, custom domains don't resolve but .localhost still works.
    let dns_port = env_port("DOORMAN_DNS_PORT").unwrap_or(DEFAULT_DNS_PORT);
    let dns = match UdpSocket::bind((Ipv4Addr::LOCALHOST, dns_port)).await {
        Ok(socket) => Some(socket),
        Err(error) => {
            eprintln!("DNS responder disabled, could not bind 127.0.0.1:{dns_port}: {error}");
            None
        }
    };

    let endpoints = Endpoints {
        http_port,
        https_port: https.as_ref().map(|(port, _)| *port),
    };
    let app = Arc::new(App::new(
        state::load_config(),
        endpoints,
        dns_port,
        dns.is_some(),
        https.is_some(),
    ));

    let mut tasks: Vec<JoinHandle<()>> = vec![tokio::spawn(control::serve(control, app.clone()))];
    for listener in http {
        tasks.push(tokio::spawn(proxy::serve(listener, app.clone(), None)));
    }
    if let Some((_, listeners)) = https {
        let ca = Arc::new(tls::Ca::load_or_create()?);
        let acceptor = TlsAcceptor::from(tls::server_config(ca, app.settings.clone())?);
        for listener in listeners {
            tasks.push(tokio::spawn(proxy::serve(
                listener,
                app.clone(),
                Some(acceptor.clone()),
            )));
        }
        tasks.push(tokio::spawn(watch_trust(app.clone())));
    }
    if let Some(socket) = dns {
        tasks.push(tokio::spawn(dns::serve(socket, app.clone())));
    }

    println!("Doorman is ready");
    println!("  http    port {http_port}");
    if let Some(port) = endpoints.https_port {
        println!("  https   port {port}");
    }
    if app.dns_running {
        println!("  dns     127.0.0.1:{dns_port}");
    }
    println!("  control {}", socket.display());

    tokio::select! {
        result = signal::ctrl_c() => result.context("wait for shutdown signal")?,
        () = app.shutdown.notified() => {}
    }
    println!("\nStopping Doorman…");
    for task in tasks {
        task.abort();
    }
    let _ = std::fs::remove_file(socket);
    Ok(())
}

fn env_port(key: &str) -> Option<u16> {
    std::env::var(key).ok().and_then(|value| value.parse().ok())
}

/// Binds the preferred port, or the fallback when it's taken. Ports below 1024 can only
/// be bound without root on the wildcard address, so those listen everywhere and the
/// proxy drops non-loopback peers; other ports listen on loopback only.
async fn bind_proxy(
    explicit: Option<u16>,
    preferred: u16,
    fallback: u16,
) -> Result<(u16, Vec<TcpListener>)> {
    let port = explicit.unwrap_or(preferred);
    // A previous proxy (or a restarting daemon) can take a moment to release its port.
    let mut attempt = bind_port(port).await;
    for _ in 0..10 {
        match &attempt {
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                attempt = bind_port(port).await;
            }
            _ => break,
        }
    }
    match attempt {
        Ok(listeners) => Ok((port, listeners)),
        Err(error) if explicit.is_none() => {
            eprintln!("port {port} unavailable ({error}); using {fallback}");
            Ok((fallback, bind_port(fallback).await?))
        }
        Err(error) => Err(error).with_context(|| format!("bind port {port}")),
    }
}

async fn bind_port(port: u16) -> std::io::Result<Vec<TcpListener>> {
    if port < 1024 {
        // On macOS the IPv6 wildcard socket is dual-stack.
        return Ok(vec![
            TcpListener::bind((Ipv6Addr::UNSPECIFIED, port)).await?,
        ]);
    }
    let mut listeners = vec![TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?];
    if let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
        listeners.push(listener);
    }
    Ok(listeners)
}

/// Keeps the cached trust state fresh; `security` is too slow to call per request.
async fn watch_trust(app: Arc<App>) {
    loop {
        let trusted = tokio::task::spawn_blocking(|| certificate_trusted(&ca_cert_path()))
            .await
            .unwrap_or(false);
        app.ca_trusted.store(trusted, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

fn set_permissions(path: &std::path::Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}
