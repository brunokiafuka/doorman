---
title: CLI commands
description: A task-oriented command reference for routes, domains, traffic, and lifecycle.
---
Use `doorman --help` or `doorman <command> --help` for built-in usage.

## Run and route

| Command | Purpose |
| --- | --- |
| `doorman` | Run the configured package script |
| `doorman run <command...>` | Run an explicit command behind a named route |
| `doorman run --name shop <command...>` | Choose the route name |
| `doorman shop <command...>` | Shorthand for the preceding command |
| `doorman add shop 3000` | Register a persistent route to an existing server |
| `doorman add shop 3000 --project /path/to/shop` | Associate a project directory |
| `doorman add shop 3000 --pid 1234` | Route expires when the owner process exits |
| `doorman list [--json]` | List routes; alias `ls` |
| `doorman get shop` | Print the route URL |
| `doorman remove shop` | Remove a route; alias `rm` |
| `doorman wildcard [on\|off]` | Inspect or change parent-route fallback |

`alias` is an alias of `add`. Removing a fixed route does not terminate the server that another tool started.

## Domains and HTTPS

| Command | Purpose |
| --- | --- |
| `doorman setup` | Trust HTTPS and finish custom-domain setup |
| `doorman trust` | Trust the local CA in the login keychain |
| `doorman domain add test [--no-setup]` | Register a suffix, optionally deferring resolver setup |
| `doorman domain list` | List suffixes and setup status |
| `doorman domain setup` | Finish resolver setup |
| `doorman domain remove test` | Remove a suffix and its resolver file |

## Inspect and share

```bash
doorman traffic --route shop --limit 50
doorman traffic --json
doorman clear-traffic
doorman tunnel shop --provider auto
```

Traffic defaults to 25 entries. Tunnel providers are `auto`, `cloudflare`, and `ngrok`. See [sharing boundaries](/guides/inspect-and-share/) before exposing an app.

## Lifecycle

| Command | Purpose |
| --- | --- |
| `doorman start` | Start the background daemon |
| `doorman stop` | Stop the daemon |
| `doorman restart` | Restart the daemon |
| `doorman service install` | Install the login service |
| `doorman service uninstall` | Remove the login service |
| `doorman service status` | Inspect service status |
| `doorman doctor` | Diagnose ports, trust, DNS, domains, and routes |
| `doorman clean` | Remove state, certificate trust, and login service |

Treat `clean` as a deliberate reset. Fixed routes and settings persist across ordinary restarts; managed run routes belong to the processes that started them.
