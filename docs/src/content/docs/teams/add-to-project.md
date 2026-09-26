---
title: Add to a project
description: A copyable integration that preserves your server command and gives the team a stable URL.
---
The goal: a new teammate clones the repository, installs dependencies, and starts the same named app as everyone else. Doorman is installed on each developer's Mac; it is not an npm dependency.

## Choose how the team starts the app

### Option A: Keep the existing scripts

If `npm run dev` already starts your server, leave it in place. Add this file beside `package.json`:

```json title="doorman.json"
{
  "name": "shop",
  "script": "dev"
}
```

Commit the file. Teammates run `doorman` from the project directory to use HTTPS, or their existing `npm run dev` command to start the server directly.

### Option B: Make Doorman the default dev command

Move the original dev command to `dev:app`, then point Doorman at it. Here is a complete scripts-and-configuration example for a Vite project; merge these fields into your existing `package.json`:

```json title="package.json (relevant fields)"
{
  "scripts": {
    "dev": "doorman",
    "dev:app": "vite"
  },
  "doorman": {
    "name": "shop",
    "script": "dev:app"
  }
}
```

Keep **your actual server command** in `dev:app`—for example, `next dev` or `astro dev` instead of `vite`.

Now the familiar command starts the named HTTPS app:

```bash
npm run dev
# → https://shop.localhost
```

The same arrangement works with `pnpm dev`, `yarn dev`, and `bun run dev`.

:::caution[Avoid calling Doorman recursively]
If `dev` runs `doorman`, configure `script` as `dev:app`. Leaving it as `dev` would ask Doorman to start itself. Use either `doorman.json` or the package block; the file takes precedence and the two are not merged.
:::

## Commit the shared contract

Commit the configuration and any script changes. Choose a stable name such as `shop`, rather than relying on each developer's checkout folder name. Leave `appPort` unset unless a tool requires a particular port; a stable URL does not need a fixed backend port.

Machine setup stays separate: installation, certificate trust, login service, and custom domains happen on each laptop. A project configuration does not configure these automatically.

## Keep CI and other platforms working

For Option B, CI and non-macOS contributors can call the server directly without Doorman installed:

```bash
npm run dev:app
```

When Doorman is installed but you want to bypass its proxy and port handling:

```bash
DOORMAN=0 npm run dev
```

This passes through to the configured server command. It does not provide HTTPS, routing, or Doorman's injected environment variables. Keep existing build and test scripts independent of the local proxy unless your tests deliberately exercise it.

## Verify before sharing

1. Run the documented start command from a fresh terminal.
2. Open the URL from `doorman get shop` and check hot reload.
3. Stop the command and confirm the temporary route disappears from `doorman list`.
4. Run the direct server script to verify the fallback works.
5. Add the [teammate setup checklist](/teams/onboarding/) to your project README.

For multiple packages or an API, continue to [monorepos and services](/teams/monorepos/).
