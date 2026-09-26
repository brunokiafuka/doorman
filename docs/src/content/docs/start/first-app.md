---
title: Run your first app
description: Start an existing dev server, open its HTTPS address, and verify the route.
---
Start with an existing project whose `package.json` has a working `dev` script. Install its dependencies with your normal package manager before continuing.

## 1. Start from the project directory

```bash
cd my-app
doorman
```

Doorman detects npm, pnpm, Yarn, or Bun from the invoking package manager or the nearest lockfile. It runs the configured script (`dev` by default), selects a free port in 4000–4999, and prints the URL.

The name comes from project configuration, the package name, the Git root, or the folder. For a predictable example, choose it explicitly:

```bash
doorman run --name shop npm run dev
```

## 2. Open the printed URL

Open `https://shop.localhost` when using the explicit name above. If port 443 is occupied, the printed URL includes Doorman's fallback HTTPS port. Always use that printed address.

Edit a page in your app and check that hot reload works. In a second terminal, inspect the route:

```bash
doorman list
doorman get shop
doorman traffic --route shop --limit 10
```

You should see the route and, after browsing the app, recent request metadata.

## 3. Stop the server

Press `Ctrl-C` in the terminal that started it. Doorman forwards the signal to the server's process group and removes its temporary route.

## Using a different command

```bash
doorman run --name api pnpm start
```

The command must bind to the supplied `PORT`, or be a framework that Doorman recognizes and gives port flags to. [Configuration reference](/reference/configuration/#frameworks-and-ports) explains the supported cases.

If your server is already running on a fixed port, register it instead:

```bash
doorman add shop 3000
doorman get shop
# When finished:
doorman remove shop
```

Removing a fixed route does not stop the server.

**Next:** [Make this repeatable for your team](/teams/add-to-project/).
