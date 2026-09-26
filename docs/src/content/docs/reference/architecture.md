---
title: Security & architecture
description: Understand what Doorman exposes, stores, and trusts.
---
## The components

| Component | Responsibility |
| --- | --- |
| `doorman-core` | Shared protocol, types, and validation |
| `doorman-cli` | Start apps, manage routes, and diagnose setup |
| `doorman-daemon` | Control socket, HTTP/HTTPS proxy, CA, and DNS |
| `doorman-desktop` | Native Slint companion for routes, traffic, and tunnels |

The daemon issues host certificates on demand, supports HTTP/2, and relays WebSockets over HTTP/1.1 and HTTPS. Leaf certificates last 397 days.

## Local by default

The daemon runs as your user, never root. On macOS it can bind standard low ports on a wildcard address, but it drops non-loopback connections before reading request bytes. Proxy upstreams are restricted to registered ports on `127.0.0.1` or `::1`.

The DNS responder binds to loopback and answers only for configured domains. Certificates are issued only for `.localhost` and configured suffixes. `/etc/hosts` is not modified.

## Files and permissions

The control socket and CA key live in a user-only state directory with mode `0700`; the key has mode `0600`. Keep that private key local. Trusting the CA lets certificates issued by this installation be accepted on your machine.

Elevated approval is limited to custom-domain resolver files and the keychain trust prompt. Resolver operations use a visible sudo prompt.

## Traffic data

The inspector is a bounded, in-memory list of the newest 200 requests. It records method, host, path, status, timing, and sizes, not headers, cookies, or bodies. Paths can still contain sensitive query parameters; keep that in mind when sharing screenshots or copied metadata.

## Explicit public access

A public tunnel is a separate, explicitly started provider process targeting one app's HTTP port. It bypasses Doorman's proxy and traffic inspector. See [inspect and share](/guides/inspect-and-share/) for its lifetime and access implications.
