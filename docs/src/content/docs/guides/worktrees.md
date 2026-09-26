---
title: Branches & worktrees
description: Run parallel checkouts without competing for one route or port.
---
A linked Git worktree lets you run another branch beside your main checkout. Doorman recognizes linked worktrees and prefixes the route with a DNS-safe branch name.

## Try a parallel checkout

With a main checkout configured as `shop`:

```bash
# In the main checkout:
doorman
# → https://shop.localhost
```

In another terminal, create a linked worktree from the repository:

```bash
git worktree add ../shop-fix -b fix-ui
cd ../shop-fix
# Install this checkout's dependencies with your package manager first.
npm ci
doorman
# → https://fix-ui.shop.localhost
```

Both servers get their own route. A linked worktree uses a free port even if `appPort` is pinned for the main checkout. Branch names are sanitized into DNS-safe labels; use the printed URL as the source of truth. Detached HEAD has no branch name to prefix automatically—choose a distinct `--name`.

## Find the right checkout

The desktop app groups running routes by repository and shows a branch badge for each checkout. Use **Open in editor**, **Reveal in Finder**, or **Stop** from the route. `doorman list` also shows branch context.

App configuration may need to account for branch-specific origins, especially OAuth callbacks and API CORS allowlists. Doorman names the route; it does not rewrite these application settings.

## Clean up

Stop the dev server with `Ctrl-C`, or use **Stop** in the desktop app. The temporary route disappears. Remove a worktree only when you are finished with its files and Git changes.
