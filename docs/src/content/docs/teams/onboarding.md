---
title: Onboard teammates
description: A repository-ready setup checklist and a clear split between shared and local configuration.
---
A reliable onboarding guide states what to install once, what to run per checkout, and how to recognize success.

## Put this in your project README

Adapt the package manager and route name. This example assumes the [default dev command integration](/teams/add-to-project/#option-b-make-doorman-the-default-dev-command):

```markdown title="README.md snippet"
## Local development (macOS)

1. Install the project's runtime and package manager.
2. Install Doorman:
   `curl -fsSL https://getdoorman.dev/install.sh | sh`
3. Run `doorman setup` once and approve local certificate trust.
4. Install this checkout's dependencies with `npm ci`.
5. Run `npm run dev`.
6. Open the URL printed by Doorman (normally https://shop.localhost).

Check your setup with `doorman doctor`.
Stop the app with Ctrl-C.

For CI or development without Doorman, run `npm run dev:app`.
```

Use `npm install` instead of `npm ci` if the project does not have an npm lockfile, or substitute the team's package manager.

## Shared versus local

| Commit to the repository | Configure on each Mac |
| --- | --- |
| Route name and server script | Doorman installation and version |
| Package scripts | Local certificate trust |
| Expected development origins | Optional custom-domain resolver |
| Setup and troubleshooting instructions | Optional login service |

Do not commit local CA files or copy them between machines. Doorman creates a separate authority per user installation.

## Agree on domains before depending on them

Prefer `.localhost` for the smallest setup. If your app needs `.test` or an owned development suffix, include the exact `doorman domain add ...` command in onboarding. A committed `doorman.json` does not register a domain on another laptop.

If OAuth callbacks or another tool requires a fixed origin, document the expected hostname and port. A busy port 443 causes a fallback URL, which may require updating the callback configuration or freeing the port.

## Check the new teammate's result

Have them open the app, edit a page, confirm hot reload, and call any companion API. `doorman doctor` checks local infrastructure; it cannot verify your application's authentication, CORS policy, or service dependencies.

For problems, start with the [symptom-based troubleshooting guide](/guides/troubleshooting/).
