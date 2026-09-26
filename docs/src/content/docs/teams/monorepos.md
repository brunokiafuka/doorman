---
title: Monorepos & services
description: Give each package a distinct route and connect services deliberately.
---
Use one route name per running service. A frontend and API can share a naming convention without sharing a port:

| Package | Route name | Local URL |
| --- | --- | --- |
| `apps/web` | `shop` | `https://shop.localhost` |
| `apps/api` | `api.shop` | `https://api.shop.localhost` |
| `apps/admin` | `admin.shop` | `https://admin.shop.localhost` |

These URLs assume the default HTTPS port is available. Check `doorman get <name>` for the actual address.

## Configure each package

Place `doorman.json` beside each package's `package.json`:

```json title="apps/web/doorman.json"
{ "name": "shop", "script": "dev" }
```

```json title="apps/api/doorman.json"
{ "name": "api.shop", "script": "dev" }
```

Run `doorman` inside each package, in separate terminals. Doorman finds the nearest ancestor containing `doorman.json` or `package.json`; it does not merge configuration from a monorepo root. Package-manager detection can find a lockfile higher up the directory tree.

Keep an existing task runner if you want one command to launch everything. Have it run the per-package commands from their package directories. Doorman itself does not orchestrate a multi-service dependency graph.

## Connect the frontend to the API

Configure your app's development API base URL as `https://api.shop.localhost`. Environment variable names depend on your framework. Browser calls across these hostnames are cross-origin, so configure the API's CORS policy for the frontend's actual origin.

For a same-origin development proxy, target the API's direct loopback port, or configure your proxy to rewrite the Host header (`changeOrigin` where supported). Forwarding the frontend Host header back through Doorman can create a loop and produce a 508 response.

## Add an existing container

Publish the container's HTTP service to a loopback port on the host. For a service already reachable at `http://127.0.0.1:8080`:

```bash
doorman add api.shop 8080
doorman get api.shop
```

The fixed route persists across daemon restarts. Remove it with `doorman remove api.shop` when no longer needed. Doorman does not start or stop the container.

## Avoid name collisions

Two active routes on the same machine cannot use the same name with `doorman run`. Use distinct project names, or [Git worktrees](/guides/worktrees/) for parallel checkouts of the same app. Different developers can use the same route name because their proxies are local to their own machines.
