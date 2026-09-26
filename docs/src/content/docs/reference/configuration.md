---
title: Project configuration
description: Configuration precedence, supported fields, environment variables, and port handling.
---
## Discovery and precedence

Doorman searches upward from the current directory for the first directory containing `doorman.json` or `package.json`. It reads project settings there. A readable `doorman.json` takes precedence over the `package.json` `doorman` block; the two are not merged.

```json title="doorman.json"
{
  "name": "shop",
  "script": "dev:app",
  "appPort": 3000
}
```

| Field | Default | Meaning |
| --- | --- | --- |
| `name` | Inferred | DNS-safe route name, including nested names such as `api.shop` |
| `script` | `dev` | Package script run when no explicit command is supplied |
| `appPort` | Free port in 4000–4999 | Backend port for the main checkout; linked worktrees use a free port |

Omit `appPort` unless you need a fixed backend port. The example's `dev:app` must exist in your package scripts.

The package alternative accepts an object with the same fields or a name string:

```json title="package.json (relevant field)"
{ "doorman": "shop" }
```

Route naming prefers an explicit `--name`, then the configured name, package name (scope stripped), Git-root folder, and current project folder. Linked worktrees add a branch prefix when available.

## Environment passed to the app

| Variable | Value |
| --- | --- |
| `PORT` | Selected backend port |
| `HOST` | `127.0.0.1` |
| `DOORMAN_URL` | Primary `.localhost` URL, including the HTTPS port if needed |
| `DOORMAN_NAME` | Effective route name, including a worktree prefix |
| `NODE_EXTRA_CA_CERTS` | Local CA certificate path when available |

These are server-process variables. A framework may require its own mechanism to expose values to browser code. Bypass mode (`DOORMAN=0`) does not inject them.

## Frameworks and ports

Doorman adds port flags for recognized Vite, Astro, Angular, React Router, Wrangler, webpack-dev-server, Expo, React Native, and Storybook commands, including supported package-script wrappers. Servers that already honor `PORT` can use the injected variable.

Compound scripts (`&&`, pipes, environment prefixes) are left untouched. For an unrecognized server, configure it to honor `PORT` or register its known port using a fixed route. Doorman also supplies Vite's additional allowed-hosts environment variable for local suffixes.

## Daemon environment

| Variable | Default / purpose |
| --- | --- |
| `DOORMAN_HTTPS_PORT` | 443, falling back to 3443 |
| `DOORMAN_HTTP_PORT` | 80, falling back to 3210 |
| `DOORMAN_PROXY_PORT` | Legacy HTTP-port variable |
| `DOORMAN_HTTPS` | HTTPS enabled; `0` disables it |
| `DOORMAN_DNS_PORT` | 53535 |
| `DOORMAN_DAEMON_PATH` | Override daemon binary discovery |
| `DOORMAN` | Set to `0` to bypass managed routing when running an app |

Listener settings apply when the daemon starts. An already-running daemon does not adopt changes from a new shell. Ensure the intended environment is present in the process starting it.
