---
title: HTTPS & custom domains
description: Understand local certificate trust, fallback ports, and per-machine domain setup.
---
## Start with .localhost

Every route answers on `.localhost` without custom DNS setup. The daemon creates a local certificate authority on first start and issues certificates for hosts on demand. Run `doorman setup` or `doorman trust` to add it to your login keychain. Chrome, Edge, Safari, and other Chromium browsers use that keychain.

Firefox uses its own trust store. For each detected profile, Doorman reads `security.enterprise_roots.enabled` from `prefs.js` and `user.js`. It leaves the profile alone when the setting is absent or already `true`, and appends a marked block to `user.js` only when the last setting is `false`. `prefs.js` is never edited because Firefox rewrites it. If Doorman changes a profile, restart Firefox; restart browsers after setup so they reload trust settings. A machine with no Firefox profiles prints nothing about Firefox. `doorman clean` removes Doorman's block, but Firefox may keep the value it copied into `prefs.js`.

Doorman prefers ports 443 and 80. If occupied, it falls back to HTTPS 3443 and HTTP 3210, and generated URLs include the port. After freeing the standard ports, run `doorman restart`.

## Add a development suffix

```bash
doorman domain add test
doorman domain list
```

A route named `shop` now also answers at `https://shop.test`. `.localhost` continues to work. Domain setup writes `/etc/resolver/test` and requests your password when needed. Repeat this on each teammate's Mac.

You can also use a development subdomain you own:

```bash
doorman domain add dev.acme.com
# shop → https://shop.dev.acme.com
```

Replace that example with your own suffix. Doorman refuses `.local` (Bonjour) and bare public TLDs such as `.dev` or `.com` to avoid intercepting real sites.

## Separate registration from resolver setup

```bash
doorman domain add test --no-setup
# Later, interactively:
doorman domain setup
```

The DNS responder listens on loopback port 53535 and returns loopback addresses for configured suffixes. Resolver setup tells macOS to ask it. Registering a domain alone does not complete DNS setup.

Remove a suffix and its resolver file with:

```bash
doorman domain remove test
```

## Tenant subdomains

```bash
doorman wildcard on
```

With wildcard fallback enabled, an unknown route such as `tenant.shop.localhost` can fall back to `shop`. This is a daemon-wide setting, not a field in project configuration. Use `doorman wildcard off` to disable it.

Your application remains responsible for interpreting tenant hostnames and configuring cookies, trusted hosts, and authentication.
