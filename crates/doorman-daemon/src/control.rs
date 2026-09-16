use std::sync::Arc;

use anyhow::{Context, Result};
use doorman_core::{Request, Response, normalize_domain};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

use crate::state::{App, MAX_TRAFFIC_ENTRIES};

pub async fn serve(listener: UnixListener, app: Arc<App>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app = app.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle(stream, app).await {
                        eprintln!("control connection: {error:#}");
                    }
                });
            }
            Err(error) => eprintln!("control accept: {error}"),
        }
    }
}

async fn handle(stream: UnixStream, app: Arc<App>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut line = String::new();
    BufReader::new(reader).read_line(&mut line).await?;
    let request: Request = serde_json::from_str(line.trim()).context("decode request")?;
    let shutting_down = matches!(request, Request::Shutdown);

    let response = respond(request, &app).await;
    writer.write_all(&serde_json::to_vec(&response)?).await?;
    writer.write_all(b"\n").await?;
    writer.shutdown().await?;
    if shutting_down {
        app.shutdown.notify_one();
    }
    Ok(())
}

fn error(message: impl std::fmt::Display) -> Response {
    Response::Error {
        message: message.to_string(),
    }
}

async fn respond(request: Request, app: &App) -> Response {
    let proxy_port = app.endpoints.http_port;
    let dns_port = app.dns_running.then_some(app.dns_port);
    match request {
        Request::Ping => Response::Pong {
            proxy_port,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        Request::Status => Response::Status {
            status: app.status(),
        },
        Request::List => {
            app.prune_dead_routes().await;
            let mut routes: Vec<_> = app.routes.read().await.values().cloned().collect();
            routes.sort_by(|left, right| left.name.cmp(&right.name));
            Response::Routes {
                routes,
                proxy_port,
                https_port: app.endpoints.https_port,
            }
        }
        Request::Traffic { route, limit } => {
            let entries = app
                .traffic
                .read()
                .await
                .iter()
                .rev()
                .filter(|entry| route.as_ref().is_none_or(|route| entry.route == *route))
                .take(limit.clamp(1, MAX_TRAFFIC_ENTRIES))
                .cloned()
                .collect();
            Response::Traffic { entries }
        }
        Request::ClearTraffic => {
            app.traffic.write().await.clear();
            Response::TrafficCleared
        }
        Request::Add { route } => {
            if let Err(message) = doorman_core::validate_name(&route.name)
                .and_then(|()| doorman_core::validate_port(route.port))
            {
                return error(message);
            }
            app.routes
                .write()
                .await
                .insert(route.name.clone(), route.clone());
            if route.pid.is_none()
                && let Err(message) = app.persist().await
            {
                return error(format!("{message:#}"));
            }
            Response::Added { route, proxy_port }
        }
        Request::Remove { name } => {
            let removed = app.routes.write().await.remove(&name);
            match removed {
                Some(route) => {
                    if route.pid.is_none()
                        && let Err(message) = app.persist().await
                    {
                        return error(format!("{message:#}"));
                    }
                    Response::Removed { name }
                }
                None => error(format!("route '{name}' is not registered")),
            }
        }
        Request::Domains => Response::Domains {
            domains: app
                .settings()
                .domains
                .iter()
                .map(|name| app.domain_info(name))
                .collect(),
            dns_port,
        },
        Request::AddDomain { name } => {
            let name = match normalize_domain(&name) {
                Ok(name) => name,
                Err(message) => return error(message),
            };
            {
                let mut settings = app.settings.write().expect("settings");
                if settings.domains.contains(&name) {
                    return error(format!(".{name} is already added"));
                }
                settings.domains.push(name.clone());
            }
            if let Err(message) = app.persist().await {
                app.settings
                    .write()
                    .expect("settings")
                    .domains
                    .retain(|domain| *domain != name);
                return error(format!("{message:#}"));
            }
            Response::DomainAdded {
                domain: app.domain_info(&name),
                dns_port,
            }
        }
        Request::RemoveDomain { name } => {
            let name = name.trim().trim_start_matches('.').to_ascii_lowercase();
            {
                let mut settings = app.settings.write().expect("settings");
                if !settings.domains.contains(&name) {
                    return error(format!(".{name} is not a Doorman domain"));
                }
                settings.domains.retain(|domain| *domain != name);
            }
            match app.persist().await {
                Ok(()) => Response::DomainRemoved { name },
                Err(message) => error(format!("{message:#}")),
            }
        }
        Request::SetWildcard { enabled } => {
            app.settings.write().expect("settings").wildcard = enabled;
            match app.persist().await {
                Ok(()) => Response::Status {
                    status: app.status(),
                },
                Err(message) => error(format!("{message:#}")),
            }
        }
        Request::Shutdown => Response::ShuttingDown,
    }
}
