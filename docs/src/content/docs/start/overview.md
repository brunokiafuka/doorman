---
title: What is Doorman?
description: Understand the local HTTPS proxy before adding it to a project.
---
Doorman gives local development servers stable names, such as `https://shop.localhost`, instead of making you remember `localhost:5173`.

## Follow a request

```text
Browser: https://shop.localhost
                ↓
Doorman: local HTTPS proxy
                ↓
Your dev server: http://127.0.0.1:4xxx
```

Your browser connects to Doorman over HTTPS. Doorman terminates TLS and forwards the request to the registered loopback port. Your server can keep speaking HTTP. WebSockets, including hot module reload, pass through too.

The CLI starts apps and manages routes. A background daemon handles routing, certificates, and custom-domain DNS. The optional desktop companion makes routes and traffic visible.

## Two ways to register an app

| Workflow | Use it when | Lifetime |
| --- | --- | --- |
| `doorman` or `doorman run <command>` | Doorman should start the dev server | Route ends when the command exits |
| `doorman add shop 3000` | Docker or another tool already starts the server | Route persists until removed |

A route is a name mapped to a local port. Registering a fixed route does not start the app listening on that port.

## What changes for your team?

Commit the route name and server script to your repository. Each developer installs Doorman on their Mac and runs the same command. Their URL is consistent; their process and port remain local.

Doorman is a development tool, not a production ingress or a shared remote environment. The distributed app targets macOS. Keep a direct server script for CI and teammates working on other platforms.

## What to learn next

1. [Install Doorman](/start/install/) and trust local HTTPS.
2. [Run one app](/start/first-app/) and verify the route.
3. [Add it to your project](/teams/add-to-project/) so teammates can repeat the setup.
