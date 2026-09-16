use std::{
    cell::RefCell,
    collections::HashMap,
    env,
    io::{BufRead, BufReader, Write},
    net::{IpAddr, Ipv4Addr, TcpStream, ToSocketAddrs},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command, Stdio},
    rc::Rc,
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use doorman_core::{
    BUILTIN_DOMAIN, Domain, Endpoints, Request, ResolverStatus, Response, Route, TrafficEntry,
    certificate_trusted, resolver_install_script, resolver_remove_script, socket_path,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

/// Ports commonly used by local dev servers, offered as one-click suggestions.
const COMMON_DEV_PORTS: &[u16] = &[
    3000, 3001, 4000, 4200, 4321, 5173, 5174, 8000, 8080, 8787, 8888,
];

#[derive(Default)]
struct State {
    routes: Vec<Route>,
    proxy_port: u16,
    endpoints: Option<Endpoints>,
    entries: Vec<TrafficEntry>,
    domains: Vec<Domain>,
    dns_port: Option<u16>,
    /// Newest request id currently rendered in the traffic table.
    shown_max_id: u64,
}

struct Models {
    routes: Rc<VecModel<RouteItem>>,
    route_names: Rc<VecModel<SharedString>>,
    route_traffic: Rc<VecModel<TrafficItem>>,
    traffic: Rc<VecModel<TrafficItem>>,
    detected_ports: Rc<VecModel<i32>>,
    domains: Rc<VecModel<DomainItem>>,
    route_checkouts: Rc<VecModel<RouteItem>>,
}

fn main() -> Result<()> {
    ensure_daemon();

    let app = MainWindow::new()?;
    let tray = DoormanTray::new()?;
    let state = Rc::new(RefCell::new(State::default()));
    let models = Rc::new(Models {
        routes: Rc::new(VecModel::default()),
        route_names: Rc::new(VecModel::default()),
        route_traffic: Rc::new(VecModel::default()),
        traffic: Rc::new(VecModel::default()),
        detected_ports: Rc::new(VecModel::default()),
        domains: Rc::new(VecModel::default()),
        route_checkouts: Rc::new(VecModel::default()),
    });
    app.set_routes(ModelRc::from(models.routes.clone()));
    app.set_route_names(ModelRc::from(models.route_names.clone()));
    app.set_route_traffic(ModelRc::from(models.route_traffic.clone()));
    app.set_traffic(ModelRc::from(models.traffic.clone()));
    app.set_detected_ports(ModelRc::from(models.detected_ports.clone()));
    app.set_domains(ModelRc::from(models.domains.clone()));
    app.set_route_checkouts(ModelRc::from(models.route_checkouts.clone()));
    app.set_version(env!("CARGO_PKG_VERSION").into());

    // Fetches from the daemon, then renders. `force` re-renders traffic even while paused.
    let refresh: Rc<dyn Fn(bool)> = {
        let app = app.as_weak();
        let state = state.clone();
        let models = models.clone();
        Rc::new(move |force| {
            if let Some(app) = app.upgrade() {
                fetch(&app, &mut state.borrow_mut());
                render(&app, &mut state.borrow_mut(), &models, force);
            }
        })
    };

    {
        let refresh = refresh.clone();
        app.on_refresh(move || refresh(false));
    }
    {
        let refresh = refresh.clone();
        tray.on_refresh(move || refresh(false));
    }
    {
        let app_weak = app.as_weak();
        let state = state.clone();
        let models = models.clone();
        app.on_filters_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                render(&app, &mut state.borrow_mut(), &models, true);
            }
        });
    }
    {
        let refresh = refresh.clone();
        let app_weak = app.as_weak();
        app.on_resume(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_paused(false);
            }
            refresh(true);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_replay_request(move |request| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_replaying(true);
            let proxy_port = u16::try_from(app.get_proxy_port()).unwrap_or(80);
            let app_weak = app_weak.clone();
            // Replays through the proxy on a worker thread so a slow target never blocks the UI.
            thread::spawn(move || {
                let result = replay(&request.host, &request.method, &request.path, proxy_port);
                let _ = app_weak.upgrade_in_event_loop(move |app| {
                    app.set_replaying(false);
                    match result {
                        Ok((status, elapsed_ms)) => {
                            show_toast(
                                &app,
                                format!("Replayed · {status} in {elapsed_ms} ms").into(),
                            );
                            app.invoke_resume();
                            // Follow the replayed request if it's visible under the current filters.
                            if let Some(newest) = app.get_traffic().row_data(0)
                                && newest.id != request.id
                                && newest.route == request.route
                                && newest.method == request.method
                                && newest.path == request.path
                            {
                                app.set_selected_request(newest);
                            }
                        }
                        Err(error) => show_toast(&app, format!("Replay failed: {error}").into()),
                    }
                });
            });
        });
    }
    {
        let app_weak = app.as_weak();
        let state = state.clone();
        app.on_setup_domains(move || {
            let state = state.borrow();
            let pending: Vec<String> = state
                .domains
                .iter()
                .filter(|domain| domain.resolver != ResolverStatus::Installed)
                .map(|domain| domain.name.clone())
                .collect();
            set_up_resolvers(app_weak.clone(), pending, state.dns_port);
        });
    }
    {
        let app_weak = app.as_weak();
        let refresh = refresh.clone();
        let state = state.clone();
        app.on_add_domain(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            match call(&Request::AddDomain {
                name: app.get_domain_draft().to_string(),
            }) {
                Ok(Response::DomainAdded { domain, .. }) => {
                    app.set_domain_draft(SharedString::new());
                    app.set_domain_error(SharedString::new());
                    refresh(true);
                    if domain.resolver == ResolverStatus::Installed {
                        show_toast(
                            &app,
                            format!("Added .{} · ready to use", domain.name).into(),
                        );
                    } else {
                        // Finish right away, like `doorman domain add` does.
                        let dns_port = state.borrow().dns_port;
                        set_up_resolvers(app.as_weak(), vec![domain.name], dns_port);
                    }
                    true
                }
                Ok(Response::Error { message }) => {
                    app.set_domain_error(message.into());
                    false
                }
                Ok(other) => {
                    app.set_domain_error(format!("Unexpected daemon response: {other:?}").into());
                    false
                }
                Err(error) => {
                    app.set_domain_error(error.to_string().into());
                    false
                }
            }
        });
    }
    {
        let app_weak = app.as_weak();
        let refresh = refresh.clone();
        app.on_remove_domain(move |name| {
            let result = call(&Request::RemoveDomain {
                name: name.to_string(),
            });
            if let Some(app) = app_weak.upgrade() {
                match result {
                    Ok(Response::DomainRemoved { name }) => {
                        show_toast(&app, format!("Removed .{name}").into());
                        if doorman_core::resolver_path(&name).exists() {
                            let app_weak = app.as_weak();
                            thread::spawn(move || {
                                let result = run_as_administrator(
                                    &resolver_remove_script(&[name.as_str()]),
                                    &format!(
                                        "Doorman wants to remove its resolver file for .{name}."
                                    ),
                                );
                                if let Err(message) = result {
                                    let _ = app_weak.upgrade_in_event_loop(move |app| {
                                        show_toast(&app, message.into());
                                    });
                                }
                            });
                        }
                    }
                    Ok(Response::Error { message }) => show_toast(&app, message.into()),
                    Ok(_) => {}
                    Err(error) => show_toast(&app, error.to_string().into()),
                }
            }
            refresh(true);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_verify_domain(move |name| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_verifying_domain(name.clone());
            let app_weak = app_weak.clone();
            // System lookups can stall on an unresponsive resolver, so keep them off the UI thread.
            thread::spawn(move || {
                let resolves = resolves_to_loopback(&format!("doorman-probe.{name}"));
                let _ = app_weak.upgrade_in_event_loop(move |app| {
                    app.set_verifying_domain(SharedString::new());
                    let message = if resolves {
                        format!("✓ .{name} resolves to 127.0.0.1")
                    } else {
                        format!(".{name} doesn't resolve to 127.0.0.1 yet")
                    };
                    show_toast(&app, message.into());
                });
            });
        });
    }
    {
        let app_weak = app.as_weak();
        let refresh = refresh.clone();
        app.on_set_wildcard(move |enabled| {
            let result = call(&Request::SetWildcard { enabled });
            if let Some(app) = app_weak.upgrade() {
                match result {
                    Ok(Response::Status { .. }) => show_toast(
                        &app,
                        if enabled {
                            "Wildcard subdomains on"
                        } else {
                            "Wildcard subdomains off"
                        }
                        .into(),
                    ),
                    Ok(Response::Error { message }) => show_toast(&app, message.into()),
                    Ok(_) => {}
                    Err(error) => show_toast(&app, error.to_string().into()),
                }
            }
            refresh(true);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_trust_ca(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Ok(Response::Status { status }) = call(&Request::Status) else {
                return;
            };
            let Some(ca) = status.ca_cert else {
                return;
            };
            app.set_trusting(true);
            let app_weak = app_weak.clone();
            // macOS shows its own approval dialog; keep the window responsive meanwhile.
            thread::spawn(move || {
                let keychain = PathBuf::from(env::var_os("HOME").unwrap_or_default())
                    .join("Library/Keychains/login.keychain-db");
                let _ = Command::new("/usr/bin/security")
                    .args(["add-trusted-cert", "-r", "trustRoot", "-k"])
                    .arg(keychain)
                    .arg(&ca)
                    .status();
                let trusted = certificate_trusted(ca.as_ref());
                let _ = app_weak.upgrade_in_event_loop(move |app| {
                    app.set_trusting(false);
                    app.set_ca_trusted(trusted);
                    show_toast(
                        &app,
                        if trusted {
                            "Certificate trusted · restart open browser tabs"
                        } else {
                            "Certificate wasn't trusted"
                        }
                        .into(),
                    );
                });
            });
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_stop_route(move |route| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // The owner is the `doorman run` process; it stops the server's whole process
            // group and removes the route itself.
            let stopped = Command::new("kill")
                .args(["-TERM", route.pid.as_str()])
                .status()
                .is_ok_and(|status| status.success());
            let label = if route.base_name.is_empty() {
                &route.name
            } else {
                &route.base_name
            };
            show_toast(
                &app,
                if stopped {
                    format!("Stopping {label}…")
                } else {
                    format!("Couldn't stop {label}; it may have already exited")
                }
                .into(),
            );
            let app_weak = app.as_weak();
            Timer::single_shot(Duration::from_millis(1200), move || {
                if let Some(app) = app_weak.upgrade() {
                    app.invoke_refresh();
                }
            });
        });
    }
    app.on_reveal_path(|path| {
        let _ = Command::new("open").arg(path.as_str()).spawn();
    });
    {
        let app_weak = app.as_weak();
        app.on_open_in_editor(move |path| {
            if !open_in_editor(&path)
                && let Some(app) = app_weak.upgrade()
            {
                show_toast(&app, "No supported editor found; opened in Finder".into());
            }
        });
    }
    app.on_open_url(|url| {
        let _ = Command::new("open").arg(url.as_str()).spawn();
    });
    {
        let app_weak = app.as_weak();
        app.on_copy_text(move |text, message| {
            copy_to_clipboard(&text);
            if let Some(app) = app_weak.upgrade() {
                show_toast(&app, message);
            }
        });
    }
    {
        let refresh = refresh.clone();
        let app_weak = app.as_weak();
        app.on_remove_route(move |name| {
            let result = call(&Request::Remove {
                name: name.to_string(),
            });
            if let Some(app) = app_weak.upgrade() {
                app.set_has_selected_route(false);
                match result {
                    Ok(Response::Removed { name }) => {
                        show_toast(&app, format!("Removed {name}.localhost").into())
                    }
                    Ok(Response::Error { message }) => show_toast(&app, message.into()),
                    _ => {}
                }
            }
            refresh(true);
        });
    }
    {
        let app_weak = app.as_weak();
        let state = state.clone();
        let models = models.clone();
        app.on_scan_ports(move || {
            let taken: Vec<u16> = state.borrow().routes.iter().map(|r| r.port).collect();
            let ports = COMMON_DEV_PORTS
                .iter()
                .copied()
                .filter(|port| !taken.contains(port) && is_listening(*port, 40))
                .map(i32::from)
                .collect::<Vec<_>>();
            models.detected_ports.set_vec(ports);
            if let Some(app) = app_weak.upgrade() {
                app.set_form_error(SharedString::new());
            }
        });
    }
    {
        let app_weak = app.as_weak();
        let state = state.clone();
        let refresh = refresh.clone();
        app.on_save_route(move || {
            let Some(app) = app_weak.upgrade() else {
                return false;
            };
            let result = save_route(&app, &state.borrow());
            match result {
                Ok(route) => {
                    app.set_form_error(SharedString::new());
                    show_toast(
                        &app,
                        if app.get_form_mode() == "create" {
                            format!("Created {}.localhost", route.name)
                        } else {
                            format!("Saved {}.localhost", route.name)
                        }
                        .into(),
                    );
                    refresh(true);
                    if let Some(item) = app
                        .get_routes()
                        .iter()
                        .find(|item| item.name.as_str() == route.name)
                    {
                        app.set_selected_route(item);
                        app.set_has_selected_route(true);
                    }
                    refresh(true);
                    true
                }
                Err(error) => {
                    app.set_form_error(error.to_string().into());
                    false
                }
            }
        });
    }
    {
        let refresh = refresh.clone();
        let app_weak = app.as_weak();
        app.on_clear_traffic(move || {
            let _ = call(&Request::ClearTraffic);
            if let Some(app) = app_weak.upgrade() {
                app.set_has_selected_request(false);
                show_toast(&app, "Traffic cleared".into());
            }
            refresh(true);
        });
    }
    {
        let app = app.as_weak();
        tray.on_open(move || {
            if let Some(app) = app.upgrade() {
                let _ = app.show();
            }
        });
    }
    tray.on_quit(|| {
        let _ = slint::quit_event_loop();
    });

    refresh(true);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(2), {
        let refresh = refresh.clone();
        move || refresh(false)
    });

    app.show()?;
    tray.show()?;
    slint::run_event_loop()?;
    Ok(())
}

fn ensure_daemon() {
    if matches!(call(&Request::Ping), Ok(Response::Pong { .. })) {
        return;
    }

    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("DOORMAN_DAEMON_PATH") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(executable) = env::current_exe()
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join("doorman-daemon"));
    }
    candidates.push(PathBuf::from("doorman-daemon"));

    for candidate in candidates {
        let started = Command::new(&candidate)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok();
        if !started {
            continue;
        }

        for _ in 0..20 {
            thread::sleep(Duration::from_millis(50));
            if matches!(call(&Request::Ping), Ok(Response::Pong { .. })) {
                return;
            }
        }
    }
}

fn fetch(app: &MainWindow, state: &mut State) {
    match call(&Request::List) {
        Ok(Response::Routes {
            routes, proxy_port, ..
        }) => {
            state.routes = routes;
            state.proxy_port = proxy_port;
            app.set_daemon_online(true);
            app.set_proxy_port(i32::from(proxy_port));
            app.set_status_detail(format!("Proxy ready on port {proxy_port}").into());
        }
        Ok(Response::Error { message }) => set_offline(app, state, message),
        Ok(_) => set_offline(app, state, "Unexpected daemon response".into()),
        Err(error) => set_offline(app, state, error.to_string()),
    }

    if let Ok(Response::Traffic { entries }) = call(&Request::Traffic {
        route: None,
        limit: 200,
    }) {
        state.entries = entries;
    }

    if let Ok(Response::Status { status }) = call(&Request::Status) {
        let endpoints = status.endpoints;
        let (scheme, port) = match endpoints.https_port {
            Some(port) => ("https://", (port != 443).then_some(port)),
            None => (
                "http://",
                (endpoints.http_port != 80).then_some(endpoints.http_port),
            ),
        };
        let suffix = port.map(|port| format!(":{port}")).unwrap_or_default();
        app.set_url_scheme(scheme.into());
        app.set_url_port_suffix(suffix.into());
        app.set_listen_label(
            format!(":{}", endpoints.https_port.unwrap_or(endpoints.http_port)).into(),
        );
        app.set_https_enabled(endpoints.https_port.is_some());
        app.set_ca_trusted(status.ca_trusted);
        app.set_wildcard(status.wildcard);
        state.endpoints = Some(endpoints);
        state.domains = status.domains;
        state.dns_port = status.dns_port;
    }
}

fn render(app: &MainWindow, state: &mut State, models: &Models, force: bool) {
    let endpoints = state.endpoints.unwrap_or(Endpoints {
        http_port: state.proxy_port,
        https_port: None,
    });
    // Per-route stats derived from the captured traffic window.
    let mut stats: HashMap<&str, RouteStats> = HashMap::new();
    for entry in &state.entries {
        let stat = stats.entry(entry.route.as_str()).or_default();
        stat.requests += 1;
        stat.total_ms += entry.duration_ms;
        if entry.status >= 400 {
            stat.errors += 1;
        }
        // Entries are newest first.
        stat.last.get_or_insert(entry.captured_at);
    }

    let query = app.get_route_query().trim().to_lowercase();
    let matching = state
        .routes
        .iter()
        .filter(|route| {
            let git = route.git.as_ref();
            query.is_empty()
                || route.name.contains(&query)
                || route.port.to_string().contains(&query)
                || route
                    .project
                    .as_deref()
                    .is_some_and(|p| p.to_lowercase().contains(&query))
                || git.is_some_and(|git| {
                    git.repo_name.to_lowercase().contains(&query)
                        || git
                            .branch
                            .as_deref()
                            .is_some_and(|branch| branch.to_lowercase().contains(&query))
                })
        })
        .collect::<Vec<_>>();
    let items = grouped_route_rows(&matching, &endpoints, &stats);
    let routes_by_name: HashMap<&str, &Route> = state
        .routes
        .iter()
        .map(|route| (route.name.as_str(), route))
        .collect();

    if app.get_has_selected_route() {
        let selected = app.get_selected_route().name;
        match state
            .routes
            .iter()
            .find(|route| route.name == selected.as_str())
        {
            Some(route) => {
                app.set_selected_route(route_item(
                    route,
                    &endpoints,
                    stats.get(route.name.as_str()),
                ));
                models.route_traffic.set_vec(
                    state
                        .entries
                        .iter()
                        .filter(|entry| entry.route == route.name)
                        .take(6)
                        .map(|entry| traffic_item(entry, &endpoints, &routes_by_name))
                        .collect::<Vec<_>>(),
                );
                models.route_checkouts.set_vec(
                    other_checkouts(route, &state.routes)
                        .map(|other| route_item(other, &endpoints, stats.get(other.name.as_str())))
                        .collect::<Vec<_>>(),
                );
            }
            None => app.set_has_selected_route(false),
        }
    }

    models.routes.set_vec(items);
    let domain_items = std::iter::once(DomainItem {
        name: BUILTIN_DOMAIN.into(),
        builtin: true,
        status: "ready".into(),
    })
    .chain(state.domains.iter().map(|domain| {
        DomainItem {
            name: domain.name.clone().into(),
            builtin: false,
            status: match domain.resolver {
                ResolverStatus::Installed => "ready",
                ResolverStatus::Missing => "setup",
                ResolverStatus::Conflict => "conflict",
            }
            .into(),
        }
    }))
    .collect::<Vec<_>>();
    // Only replace when changed so hover and scroll state in the list survive polling.
    if domain_items.len() != models.domains.row_count()
        || domain_items
            .iter()
            .zip(models.domains.iter())
            .any(|(a, b)| *a != b)
    {
        models.domains.set_vec(domain_items);
    }
    app.set_dns_running(state.dns_port.is_some());
    app.set_route_total(state.routes.len() as i32);
    let names = state
        .routes
        .iter()
        .map(|route| SharedString::from(route.name.as_str()))
        .collect::<Vec<_>>();
    if names.len() != models.route_names.row_count()
        || names
            .iter()
            .zip(models.route_names.iter())
            .any(|(a, b)| *a != b)
    {
        models.route_names.set_vec(names);
    }

    app.set_traffic_total(state.entries.len() as i32);
    let newest = state.entries.first().map_or(0, |entry| entry.id);
    if app.get_paused() && !force {
        let pending = state
            .entries
            .iter()
            .take_while(|entry| entry.id > state.shown_max_id)
            .count();
        app.set_pending_count(pending as i32);
        return;
    }

    let route_filter = app.get_traffic_route_filter();
    let status_filter = app.get_traffic_status_filter();
    let rows = state
        .entries
        .iter()
        .filter(|entry| route_filter.is_empty() || entry.route == route_filter.as_str())
        .filter(|entry| match status_filter.as_str() {
            "ok" => entry.status < 400,
            "errors" => entry.status >= 400,
            _ => true,
        })
        .map(|entry| traffic_item(entry, &endpoints, &routes_by_name))
        .collect::<Vec<_>>();
    models.traffic.set_vec(rows);
    state.shown_max_id = newest;
    app.set_pending_count(0);

    if app.get_has_selected_request() {
        let id = app.get_selected_request().id;
        if !state.entries.iter().any(|entry| entry.id as i32 == id) {
            app.set_has_selected_request(false);
        }
    }
}

#[derive(Default)]
struct RouteStats {
    requests: u64,
    errors: u64,
    total_ms: u64,
    last: Option<DateTime<Utc>>,
}

/// Rows for the Routes table: grouped under a header per repository once any route
/// carries git context, main checkouts before worktrees; flat otherwise.
fn grouped_route_rows(
    routes: &[&Route],
    endpoints: &Endpoints,
    stats: &HashMap<&str, RouteStats>,
) -> Vec<RouteItem> {
    let item = |route: &Route| route_item(route, endpoints, stats.get(route.name.as_str()));
    if routes.iter().all(|route| route.git.is_none()) {
        return routes.iter().map(|route| item(route)).collect();
    }

    let mut groups: Vec<(String, Vec<&Route>)> = Vec::new();
    for route in routes {
        let label = route
            .git
            .as_ref()
            .map_or("Other routes", |git| git.repo_name.as_str());
        match groups.iter_mut().find(|(name, _)| name == label) {
            Some((_, members)) => members.push(route),
            None => groups.push((label.to_owned(), vec![route])),
        }
    }
    // Repositories alphabetically, routes without git last.
    groups.sort_by_key(|(name, members)| (members[0].git.is_none(), name.to_lowercase()));

    let mut rows = Vec::new();
    for (name, mut members) in groups {
        members.sort_by_key(|route| {
            let git = route.git.as_ref();
            (
                git.map_or(route.name.as_str(), |git| git.base_name.as_str())
                    .to_owned(),
                git.is_some_and(|git| git.linked),
                git.and_then(|git| git.branch.clone()),
            )
        });
        rows.push(RouteItem {
            header: name.into(),
            header_count: members.len() as i32,
            ..RouteItem::default()
        });
        rows.extend(members.into_iter().map(item));
    }
    rows
}

/// Other checkouts (main or worktrees) running the same app as `route`.
fn other_checkouts<'a>(route: &'a Route, routes: &'a [Route]) -> impl Iterator<Item = &'a Route> {
    let git = route.git.as_ref();
    routes.iter().filter(move |other| {
        other.name != route.name
            && git.is_some_and(|git| {
                other.git.as_ref().is_some_and(|theirs| {
                    theirs.repo == git.repo && theirs.base_name == git.base_name
                })
            })
    })
}

fn tilde_path(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

fn route_item(route: &Route, endpoints: &Endpoints, stats: Option<&RouteStats>) -> RouteItem {
    let (requests, errors, avg, last) = match stats {
        Some(s) if s.requests > 0 => (
            s.requests,
            s.errors,
            format!("{} ms", s.total_ms / s.requests),
            s.last.map(relative_time).unwrap_or_default(),
        ),
        _ => (0, 0, String::new(), String::new()),
    };
    let git = route.git.as_ref();
    RouteItem {
        name: route.name.clone().into(),
        url: endpoints.url(&route.hostname()).into(),
        command: route.command.clone().unwrap_or_default().into(),
        header: SharedString::new(),
        header_count: 0,
        repo: git.map(|git| git.repo.clone()).unwrap_or_default().into(),
        branch: git
            .and_then(|git| git.branch.clone())
            .unwrap_or_default()
            .into(),
        linked: git.is_some_and(|git| git.linked),
        base_name: git
            .map(|git| git.base_name.clone())
            .unwrap_or_default()
            .into(),
        worktree: git
            .map(|git| tilde_path(&git.worktree))
            .unwrap_or_default()
            .into(),
        worktree_path: git
            .map(|git| git.worktree.clone())
            .unwrap_or_default()
            .into(),
        target: format!("127.0.0.1:{}", route.port).into(),
        port: i32::from(route.port),
        project: route.project.clone().unwrap_or_default().into(),
        pid: route
            .pid
            .map(|pid| pid.to_string())
            .unwrap_or_default()
            .into(),
        created: route
            .created_at
            .with_timezone(&Local)
            .format("%b %-d, %H:%M")
            .to_string()
            .into(),
        healthy: is_listening(route.port, 120),
        requests: requests as i32,
        errors: errors as i32,
        last_seen: last.into(),
        avg_latency: avg.into(),
    }
}

fn traffic_item(
    entry: &TrafficEntry,
    endpoints: &Endpoints,
    routes: &HashMap<&str, &Route>,
) -> TrafficItem {
    // Show worktree routes as their app name plus a branch badge.
    let worktree = routes
        .get(entry.route.as_str())
        .and_then(|route| route.git.as_ref())
        .filter(|git| git.linked);
    let host = entry
        .host
        .clone()
        .unwrap_or_else(|| format!("{}.{BUILTIN_DOMAIN}", entry.route));
    let origin = match entry.scheme.as_deref() {
        Some("http") => doorman_core::host_url(&host, endpoints.http_port),
        _ => endpoints.url(&host),
    };
    let url = format!("{origin}{}", entry.path);
    let curl = if entry.method == "GET" {
        format!("curl -i '{url}'")
    } else {
        format!("curl -i -X {} '{url}'", entry.method)
    };
    let captured = entry.captured_at.with_timezone(&Local);
    TrafficItem {
        id: entry.id as i32,
        route: entry.route.clone().into(),
        route_label: worktree
            .map_or_else(|| entry.route.clone(), |git| git.base_name.clone())
            .into(),
        branch: worktree
            .and_then(|git| git.branch.clone())
            .unwrap_or_default()
            .into(),
        host: host.into(),
        method: entry.method.clone().into(),
        path: entry.path.clone().into(),
        url: url.into(),
        status: i32::from(entry.status),
        duration: format!("{} ms", entry.duration_ms).into(),
        size: entry
            .response_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "—".into())
            .into(),
        request_size: entry
            .request_bytes
            .map(format_bytes)
            .unwrap_or_default()
            .into(),
        response_size: entry
            .response_bytes
            .map(format_bytes)
            .unwrap_or_default()
            .into(),
        time: captured.format("%H:%M:%S").to_string().into(),
        full_time: captured.format("%b %-d, %H:%M:%S%.3f").to_string().into(),
        curl: curl.into(),
    }
}

fn save_route(app: &MainWindow, state: &State) -> Result<Route> {
    let name = app.get_form_name().trim().to_lowercase();
    let port: u16 = app
        .get_form_port()
        .trim()
        .parse()
        .ok()
        .filter(|port| *port > 0)
        .context("Port must be a number between 1 and 65535")?;
    let project = Some(app.get_form_project().trim().to_owned()).filter(|p| !p.is_empty());
    let original = app.get_form_original();
    let original = state
        .routes
        .iter()
        .find(|route| !original.is_empty() && route.name == original.as_str());

    if state
        .routes
        .iter()
        .any(|route| route.name == name && original.is_none_or(|o| o.name != name))
    {
        anyhow::bail!("{name}.localhost is already registered");
    }

    let mut route = Route::new(name, port, original.and_then(|o| o.pid), project)?;
    if let Some(original) = original {
        route.created_at = original.created_at;
    }

    match call(&Request::Add {
        route: route.clone(),
    })? {
        Response::Added { .. } => {}
        Response::Error { message } => anyhow::bail!(message),
        other => anyhow::bail!("Unexpected daemon response: {other:?}"),
    }
    if let Some(original) = original
        && original.name != route.name
    {
        let _ = call(&Request::Remove {
            name: original.name.clone(),
        });
    }
    Ok(route)
}

fn relative_time(at: DateTime<Utc>) -> String {
    let seconds = (Utc::now() - at).num_seconds().max(0);
    match seconds {
        0..=4 => "just now".into(),
        5..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn format_bytes(bytes: u64) -> String {
    match bytes {
        0..1_024 => format!("{bytes} B"),
        1_024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1_024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

fn is_listening(port: u16, timeout_ms: u64) -> bool {
    [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    ]
    .into_iter()
    .any(|ip| {
        TcpStream::connect_timeout(&(ip, port).into(), Duration::from_millis(timeout_ms)).is_ok()
    })
}

fn copy_to_clipboard(text: &str) {
    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

fn show_toast(app: &MainWindow, message: SharedString) {
    app.set_toast(message.clone());
    let app = app.as_weak();
    Timer::single_shot(Duration::from_millis(1800), move || {
        if let Some(app) = app.upgrade()
            && app.get_toast() == message
        {
            app.set_toast(SharedString::new());
        }
    });
}

fn set_offline(app: &MainWindow, state: &mut State, detail: String) {
    state.routes.clear();
    state.entries.clear();
    app.set_daemon_online(false);
    app.set_status_detail(detail.into());
}

/// Resends a captured request's method and path through the Doorman proxy. Headers and
/// bodies are never captured, so the replay carries neither.
fn replay(host: &str, method: &str, path: &str, proxy_port: u16) -> Result<(u16, u128)> {
    let started = std::time::Instant::now();
    let mut stream = TcpStream::connect_timeout(
        &(Ipv4Addr::LOCALHOST, proxy_port).into(),
        Duration::from_secs(2),
    )
    .context("proxy is not reachable")?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{proxy_port}\r\nUser-Agent: doorman-replay\r\nAccept: */*\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;

    let mut status_line = String::new();
    BufReader::new(stream).read_line(&mut status_line)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .context("unexpected response from proxy")?;
    Ok((status, started.elapsed().as_millis()))
}

/// Writes resolver files for `domains` behind the standard macOS administrator prompt.
fn set_up_resolvers(app: slint::Weak<MainWindow>, domains: Vec<String>, dns_port: Option<u16>) {
    let Some(ui) = app.upgrade() else {
        return;
    };
    if domains.is_empty() {
        return;
    }
    let Some(dns_port) = dns_port else {
        show_toast(
            &ui,
            "Doorman's DNS responder isn't running, so domains can't resolve".into(),
        );
        return;
    };
    ui.set_setting_up_domains(true);
    thread::spawn(move || {
        let names: Vec<&str> = domains.iter().map(String::as_str).collect();
        let dotted = names
            .iter()
            .map(|name| format!(".{name}"))
            .collect::<Vec<_>>()
            .join(", ");
        let result = run_as_administrator(
            &resolver_install_script(&names, dns_port),
            &format!("Doorman wants to send {dotted} lookups to its local DNS responder."),
        );
        let _ = app.upgrade_in_event_loop(move |ui| {
            ui.set_setting_up_domains(false);
            match result {
                Ok(()) => show_toast(&ui, format!("{dotted} is ready").into()),
                Err(message) => show_toast(&ui, message.into()),
            }
            ui.invoke_refresh();
        });
    });
}

/// Runs a shell script as root via the standard macOS password dialog.
fn run_as_administrator(script: &str, prompt: &str) -> Result<(), String> {
    let quote = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    let apple_script = format!(
        "do shell script \"{}\" with prompt \"{}\" with administrator privileges",
        quote(script),
        quote(prompt)
    );
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &apple_script])
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("-128") {
        Err("Setup cancelled".to_owned())
    } else {
        Err(format!("Setup failed: {}", stderr.trim()))
    }
}

/// Opens a folder in the first installed editor. GUI apps don't inherit the shell PATH,
/// so editors are found by bundle identifier rather than their CLI shims.
fn open_in_editor(path: &str) -> bool {
    const EDITORS: &[&str] = &[
        "com.todesktop.230313mzl4w4u92", // Cursor
        "com.microsoft.VSCode",
        "dev.zed.Zed",
        "com.exafunction.windsurf",
        "com.sublimetext.4",
    ];
    let opened = EDITORS.iter().any(|bundle| {
        Command::new("open")
            .args(["-b", bundle, path])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    });
    if !opened {
        let _ = Command::new("open").arg(path).spawn();
    }
    opened
}

/// Resolves through the system resolver, exactly like a browser would.
fn resolves_to_loopback(host: &str) -> bool {
    (host, 80).to_socket_addrs().is_ok_and(|mut addresses| {
        addresses.any(|address| address.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST))
    })
}

fn call(request: &Request) -> Result<Response> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("Start doorman-daemon · {}", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line).context("decode daemon response")
}
