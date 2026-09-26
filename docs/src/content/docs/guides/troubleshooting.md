---
title: Troubleshooting
description: Diagnose local setup, routing, trust, and application integration problems.
---
Start with:

```bash
doorman doctor
doorman list
```

Use the URL printed by Doorman, including a fallback port if one is present.

## The shell cannot find doorman

Open a new terminal after installation. Check that `~/.local/bin` is on PATH. If you installed with `DOORMAN_NO_MODIFY_PATH=1`, add your configured CLI directory to PATH yourself.

## The browser does not trust HTTPS

Run `doorman trust` and approve the keychain prompt. Recheck with `doorman doctor`. For Firefox, check `security.enterprise_roots.enabled`. Do not disable TLS verification as an onboarding workaround.

## The URL includes :3443

Another process may be using port 443. Doctor reports the active endpoints. Use the printed URL, or stop the conflicting service if appropriate and run `doorman restart`. Update application callback URLs when their origin includes a port.

## A route exists, but the app cannot be reached

Check the server's terminal output. A fixed route does not start a server; its port must already be listening. For managed commands, the app must use Doorman's `PORT` or an injected framework flag.

Compound scripts with `&&`, pipes, or environment prefixes are left untouched by flag injection. Make the server command directly invokable, have it read `PORT`, or start it yourself on a known port and use `doorman add`.

## The route name is already in use

Stop the existing managed server or choose another `--name`. If it is an obsolete fixed route, remove it with `doorman remove <name>` before starting a managed route with that name.

## The dev script tries to start itself

If `dev` is `doorman`, set the configuration's `script` to a separate real server script such as `dev:app`. Check for a `doorman.json` overriding the package block. Follow the [project integration example](/teams/add-to-project/).

## A custom domain does not resolve

Run `doorman domain list`, then `doorman domain setup` to finish resolver installation. Check `doorman doctor` again. Each Mac needs its own resolver setup; pulling the repository does not install it.

## Requests return 508

A proxy loop sent the request back through Doorman. Point the development proxy at the upstream app's direct loopback port, or enable Host-header rewriting (often `changeOrigin`) when forwarding through a named Doorman route.

## The page works but the API or login does not

Check app-level CORS, cookies, allowed hosts, OAuth redirect URIs, and API base URLs against the actual HTTPS origin. A worktree changes the hostname; a fallback port changes the origin. Doorman cannot infer these policies for your application.

## A tunnel fails to start

Check that the selected provider is installed and, for ngrok, authenticated. Read provider output in the terminal or desktop tunnel logs. Authentication failure does not silently switch providers.

## Reset deliberately

`doorman restart` restarts the daemon. `doorman clean` removes Doorman state, certificate trust, and the login service; it is a reset, not a routine diagnostic command. Expect to set up trust and routes again after cleaning.
