use std::{
    convert::Infallible,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bytes::Bytes;
use doorman_core::{Route, TrafficEntry};
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::{
    Request, Response, StatusCode, Version,
    body::Incoming,
    header::{ACCEPT, CONNECTION, HOST, HeaderValue, LOCATION, UPGRADE},
    service::service_fn,
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

use crate::{
    pages::{self, ErrorPage, Hint, Probe},
    state::{App, LookupError},
};

pub type ProxyBody = BoxBody<Bytes, hyper::Error>;

const HOPS_HEADER: &str = "x-doorman-hops";
/// A request passing through Doorman this many times is almost certainly looping.
const MAX_HOPS: u8 = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

pub async fn serve(listener: TcpListener, app: Arc<App>, tls: Option<TlsAcceptor>) {
    let scheme = if tls.is_some() {
        Scheme::Https
    } else {
        Scheme::Http
    };
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                eprintln!("proxy accept: {error}");
                continue;
            }
        };
        // Privileged ports are bound on all interfaces; only this machine may connect.
        if !peer.ip().to_canonical().is_loopback() {
            continue;
        }
        let app = app.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(request, app.clone(), scheme, peer));
            let builder = auto::Builder::new(TokioExecutor::new());
            let result = match tls {
                Some(acceptor) => match acceptor.accept(stream).await {
                    Ok(stream) => {
                        builder
                            .serve_connection_with_upgrades(TokioIo::new(stream), service)
                            .await
                    }
                    Err(_) => return, // Untrusted certificate or unknown host; nothing to log.
                },
                None => {
                    builder
                        .serve_connection_with_upgrades(TokioIo::new(stream), service)
                        .await
                }
            };
            if let Err(error) = result {
                let message = error.to_string();
                if !message.contains("connection closed") && !message.contains("reset") {
                    eprintln!("proxy connection: {message}");
                }
            }
        });
    }
}

async fn handle(
    request: Request<Incoming>,
    app: Arc<App>,
    scheme: Scheme,
    peer: SocketAddr,
) -> Result<Response<ProxyBody>, Infallible> {
    let started_at = Instant::now();
    let method = request.method().to_string();
    let path = request
        .uri()
        .path_and_query()
        .map_or_else(|| "/".to_owned(), ToString::to_string);
    let host = request_host(&request);
    let host_name = host.split(':').next().unwrap_or_default().to_owned();
    let wants_html =
        header(&request, ACCEPT.as_str()).is_some_and(|accept| accept.contains("text/html"));
    let probe = header(&request, pages::PROBE_HEADER).map(Probe::parse);
    let hops = header(&request, HOPS_HEADER)
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);

    let route = match app.lookup(&host).await {
        Ok(route) => route,
        Err(error) => {
            if probe.is_some() {
                return Ok(probe_response(false));
            }
            return Ok(lookup_error_response(&error, &host_name, wants_html));
        }
    };
    if let Some(probe) = probe {
        let up = probe == Probe::Route || connect_app(route.port).await.is_ok();
        return Ok(probe_response(up));
    }
    if let Some(redirect) = https_redirect(&app, scheme, &request, &host_name, &path, wants_html) {
        return Ok(redirect);
    }

    let request_bytes = content_length(request.headers());
    let response = if hops >= MAX_HOPS {
        loop_response(&route, &host_name, wants_html)
    } else {
        match forward(request, &route, &host, scheme, peer, hops).await {
            Ok(response) => response,
            Err(error) if wants_html => html_response(
                StatusCode::BAD_GATEWAY,
                unreachable_page(&route, &host_name, &format!("{error:#}")),
            ),
            Err(error) => text_response(
                StatusCode::BAD_GATEWAY,
                format!(
                    "Doorman could not reach the app: {error:#}\n\n\
                     Start the app on port {port}, or point the route elsewhere:\n  \
                     doorman add {name} <port>\n",
                    port = route.port,
                    name = route.name
                ),
            ),
        }
    };

    app.record(TrafficEntry {
        id: 0,
        route: route.name.clone(),
        host: Some(host_name),
        scheme: Some(scheme.as_str().to_owned()),
        method,
        path,
        status: response.status().as_u16(),
        duration_ms: started_at.elapsed().as_millis() as u64,
        request_bytes,
        response_bytes: content_length(response.headers()),
        captured_at: chrono::Utc::now(),
    })
    .await;
    Ok(response)
}

fn header<'a>(request: &'a Request<Incoming>, name: &str) -> Option<&'a str> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

/// HTTP/1 carries the host in a header; HTTP/2 carries it in the URI authority.
fn request_host(request: &Request<Incoming>) -> String {
    header(request, HOST.as_str())
        .map(str::to_owned)
        .or_else(|| request.uri().authority().map(ToString::to_string))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Sends browsers from http:// to https:// once the certificate is trusted, so typed
/// URLs land on the secure origin. Other clients keep plain HTTP.
fn https_redirect(
    app: &App,
    scheme: Scheme,
    request: &Request<Incoming>,
    host: &str,
    path: &str,
    wants_html: bool,
) -> Option<Response<ProxyBody>> {
    let navigational = matches!(request.method().as_str(), "GET" | "HEAD");
    let ready = app.endpoints.https_port.is_some() && app.ca_trusted.load(Ordering::Relaxed);
    if scheme != Scheme::Http || !navigational || !wants_html || !ready {
        return None;
    }
    let location = format!("{}{path}", app.endpoints.url(host));
    Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(LOCATION, location)
        .body(empty_body())
        .ok()
}

async fn connect_app(port: u16) -> Result<TcpStream> {
    let attempt = |address: SocketAddr| {
        tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(address))
    };
    // Dev servers bind either loopback family depending on how `localhost` resolved.
    if let Ok(Ok(stream)) = attempt((Ipv4Addr::LOCALHOST, port).into()).await {
        return Ok(stream);
    }
    attempt((Ipv6Addr::LOCALHOST, port).into())
        .await
        .context("timed out")?
        .with_context(|| format!("connect to 127.0.0.1:{port} or [::1]:{port}"))
}

fn is_upgrade(request: &Request<Incoming>) -> bool {
    request.version() == Version::HTTP_11
        && request.headers().contains_key(UPGRADE)
        && header(request, CONNECTION.as_str())
            .is_some_and(|value| value.to_ascii_lowercase().contains("upgrade"))
}

async fn forward(
    mut request: Request<Incoming>,
    route: &Route,
    host: &str,
    scheme: Scheme,
    peer: SocketAddr,
    hops: u8,
) -> Result<Response<ProxyBody>> {
    let upstream = connect_app(route.port).await?;
    let client_upgrade = is_upgrade(&request).then(|| hyper::upgrade::on(&mut request));

    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |value| value.as_str())
        .parse()?;
    *request.uri_mut() = path;
    // Apps always speak HTTP/1.1 to Doorman, whatever the browser negotiated.
    *request.version_mut() = Version::HTTP_11;
    let headers = request.headers_mut();
    let host_value =
        HeaderValue::from_str(host).unwrap_or_else(|_| HeaderValue::from_static("localhost"));
    headers.insert(HOST, host_value.clone());
    headers.insert("x-forwarded-host", host_value);
    headers.insert(
        "x-forwarded-proto",
        HeaderValue::from_static(scheme.as_str()),
    );
    if let Ok(value) = HeaderValue::from_str(&peer.ip().to_canonical().to_string()) {
        headers.insert("x-forwarded-for", value);
    }
    headers.insert(HOPS_HEADER, HeaderValue::from(i32::from(hops) + 1));

    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(upstream)).await?;
    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            eprintln!("upstream connection: {error}");
        }
    });

    let mut response = sender.send_request(request).await?;
    if response.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(client_upgrade) = client_upgrade
    {
        let upstream_upgrade = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            match tokio::try_join!(client_upgrade, upstream_upgrade) {
                Ok((client, upstream)) => {
                    let _ = tokio::io::copy_bidirectional(
                        &mut TokioIo::new(client),
                        &mut TokioIo::new(upstream),
                    )
                    .await;
                }
                Err(error) => eprintln!("upgrade: {error}"),
            }
        });
    }
    Ok(response.map(|body| body.boxed()))
}

fn content_length(headers: &hyper::HeaderMap) -> Option<u64> {
    headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn lookup_error_response(error: &LookupError, host: &str, wants_html: bool) -> Response<ProxyBody> {
    match error {
        LookupError::UnknownHost { accepted } => text_response(
            StatusCode::MISDIRECTED_REQUEST,
            format!("Doorman accepts {}", accepted.join(", ")),
        ),
        LookupError::NotFound { name } if wants_html => {
            html_response(StatusCode::NOT_FOUND, not_found_page(name, host))
        }
        LookupError::NotFound { name } => text_response(
            StatusCode::NOT_FOUND,
            format!(
                "No Doorman route named '{name}'.\n\nFrom your project, run:\n  doorman run <dev command>\nor register a fixed port:\n  doorman add {name} <port>\n"
            ),
        ),
    }
}

fn loop_response(route: &Route, host: &str, wants_html: bool) -> Response<ProxyBody> {
    let message = format!(
        "{host} keeps proxying back through Doorman. If this app forwards requests to \
         another Doorman route, rewrite the Host header to the target (for example \
         changeOrigin: true in http-proxy or Vite's server.proxy)."
    );
    if !wants_html {
        return text_response(StatusCode::LOOP_DETECTED, format!("{message}\n"));
    }
    let name = pages::escape(&route.name);
    html_response(
        StatusCode::LOOP_DETECTED,
        ErrorPage {
            status: 508,
            reason: "Loop detected",
            host,
            headline: format!("<code>{}</code> is going in circles", pages::escape(host)),
            summary: "This request passed through Doorman over and over without reaching an app."
                .into(),
            waiting_for: String::new(),
            probe: Probe::None,
            hints: vec![
                Hint {
                    title: "Rewrite the Host header when proxying",
                    body: &format!(
                        "If <code>{name}</code> forwards requests to another Doorman URL, set \
                         <code>changeOrigin: true</code> (http-proxy, Vite <code>server.proxy</code>) \
                         so the target sees its own host."
                    ),
                    command: None,
                },
                Hint {
                    title: "Check where the route points",
                    body: "Make sure the route's port is the app itself, not Doorman's proxy port.",
                    command: Some("doorman list".into()),
                },
            ],
            detail: None,
        }
        .render(),
    )
}

fn unreachable_page(route: &Route, host: &str, error: &str) -> String {
    let name = pages::escape(&route.name);
    let port = route.port;
    let start_body = match (&route.command, &route.project) {
        (Some(command), _) => format!(
            "<code>{}</code> is registered but not listening on port <code>{port}</code> yet. \
             It may still be starting, or it may have exited.",
            pages::escape(command)
        ),
        (None, Some(project)) => format!(
            "Run your dev server from <code>{}</code> so it listens on port <code>{port}</code>.",
            pages::escape(&tilde_path(project))
        ),
        (None, None) => format!("Run your dev server so it listens on port <code>{port}</code>."),
    };
    let start_command = route
        .project
        .as_deref()
        .map(|project| format!("cd {} && doorman run", shell_path(project)));
    let retarget_body = format!(
        "Update the route from your terminal, or open the Doorman app and choose \
         <b>Routes → {name} → Configure</b>."
    );
    ErrorPage {
        status: 502,
        reason: "Bad gateway",
        host,
        headline: format!("<code>{}</code> isn't answering", pages::escape(host)),
        summary: format!(
            "Doorman knocked on <code>127.0.0.1:{port}</code>, but nothing opened the door."
        ),
        waiting_for: format!("Waiting for port {port}…"),
        probe: Probe::App,
        hints: vec![
            Hint {
                title: "Start your app",
                body: &start_body,
                command: start_command,
            },
            Hint {
                title: "Running on a different port?",
                body: &retarget_body,
                command: Some(format!("doorman add {} <port>", route.name)),
            },
            Hint {
                title: "Still stuck?",
                body: "Check the daemon, proxy, certificates, domains, and every route in one go.",
                command: Some("doorman doctor".into()),
            },
        ],
        detail: Some(error.to_owned()),
    }
    .render()
}

fn not_found_page(name: &str, host: &str) -> String {
    let escaped = pages::escape(name);
    let run_body = format!(
        "From the project folder, wrap your dev command. Doorman picks a port and names the \
         route after the project, or pass <code>--name {escaped}</code>."
    );
    let add_body = format!(
        "Point <code>{escaped}</code> at a port that's already running, or open the Doorman app \
         and choose <b>Routes → New route</b>."
    );
    ErrorPage {
        status: 404,
        reason: "No such route",
        host,
        headline: format!("No route named <code>{escaped}</code>"),
        summary: "Doorman is running, but nothing is registered under this name yet.".into(),
        waiting_for: format!("Waiting for a route named {escaped}…"),
        probe: Probe::Route,
        hints: vec![
            Hint {
                title: "Run your app through Doorman",
                body: &run_body,
                command: Some(format!("doorman run --name {name} npm run dev")),
            },
            Hint {
                title: "Or register a fixed port",
                body: &add_body,
                command: Some(format!("doorman add {name} 5173")),
            },
            Hint {
                title: "Looking for another app?",
                body: "List every route Doorman knows about.",
                command: Some("doorman list".into()),
            },
        ],
        detail: None,
    }
    .render()
}

/// Shortens paths under the home directory to `~/…` for display.
fn tilde_path(path: &str) -> String {
    home_relative(path).map_or_else(|| path.to_owned(), |rest| format!("~/{rest}"))
}

/// A `cd`-ready path that still lets the shell expand `~`.
fn shell_path(path: &str) -> String {
    home_relative(path).map_or_else(
        || shell_quote(path),
        |rest| format!("~/{}", shell_quote(rest)),
    )
}

fn home_relative(path: &str) -> Option<&str> {
    let home = std::env::var("HOME").ok()?;
    path.strip_prefix(home.trim_end_matches('/'))?
        .strip_prefix('/')
        .filter(|rest| !rest.is_empty())
}

fn shell_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-~".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

fn empty_body() -> ProxyBody {
    Empty::new().map_err(|never| match never {}).boxed()
}

/// Answers the error pages' polling without touching the app or the traffic log.
fn probe_response(up: bool) -> Response<ProxyBody> {
    Response::builder()
        .status(if up {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        })
        .header("cache-control", "no-store")
        .body(empty_body())
        .expect("valid static response")
}

fn html_response(status: StatusCode, html: String) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .body(
            Full::new(Bytes::from(html))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("valid static response")
}

fn text_response(status: StatusCode, message: impl Into<String>) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from(message.into()))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("valid static response")
}
