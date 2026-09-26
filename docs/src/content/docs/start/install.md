---
title: Install on macOS
description: Install the app and CLI, trust local HTTPS, and verify your setup.
---
You need a Mac with Apple Silicon or Intel, a terminal, and a working development project. Install your project's runtime and dependencies separately; Doorman does not install Node.js or your package manager.

## 1. Install the app and CLI

```bash
curl -fsSL https://getdoorman.dev/install.sh | sh
```

The installer downloads the latest universal release, checks its SHA-256, installs `Doorman.app` in `/Applications`, and links the CLI into `~/.local/bin`. It adds that directory to your shell's PATH if needed.

The installer also opens the **Doorman macOS app**. You get two ways to work:

- **The CLI** starts your dev servers and gives them named HTTPS URLs.
- **The macOS app** shows your registered routes and running projects, with branch context and a live request inspector.

An empty app is normal on a fresh installation. Projects appear when you start them with `doorman` or register an existing server with `doorman add`; Doorman does not scan your computer for projects. You can reopen the app from Applications or Spotlight at any time.

Open a new terminal if `doorman` is not found after installation.

## 2. Set up trusted HTTPS

```bash
doorman setup
```

Approve the keychain prompt to trust the certificate authority created on your machine. Setup also finishes any custom domains already registered. You do not need a custom domain to use `.localhost`.

Each teammate does this on their own computer. Never copy a teammate's certificate authority or private key.

## 3. Check the installation

```bash
doorman --version
doorman doctor
```

Doctor reports the daemon, ports, HTTPS trust, DNS, domains, and routes. An empty route list is expected before starting your first app.

Optionally start Doorman at login:

```bash
doorman service install
```

## Installer options

| Variable | Purpose |
| --- | --- |
| `DOORMAN_VERSION` | Pin a release, for example `0.2.0` |
| `DOORMAN_INSTALL_DIR` | Change the app installation directory |
| `DOORMAN_BIN_DIR` | Change the CLI link directory |
| `DOORMAN_NO_MODIFY_PATH=1` | Keep shell profiles untouched |
| `DOORMAN_NO_LAUNCH=1` | Skip opening the desktop app |

For a pinned install, set the variable on the installer process:

```bash
curl -fsSL https://getdoorman.dev/install.sh | DOORMAN_VERSION=0.2.0 sh
```

Use a version that exists in [GitHub Releases](https://github.com/brunokiafuka/doorman/releases). For local builds, see [build from source](/reference/development/).

**Next:** [Run your first app](/start/first-app/).
