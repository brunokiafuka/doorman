---
title: Build from source
description: Work on the Rust workspace and maintain the documentation site.
---
## Run the Rust workspace

Clone [the repository](https://github.com/brunokiafuka/doorman), install the Rust toolchain and the native dependencies required by the desktop app, then run from the repository root:

```bash
cargo run -p doorman-cli -- --help
cargo test --workspace --locked
```

Install the CLI and daemon from the checkout:

```bash
cargo install --path crates/doorman-cli --locked
cargo install --path crates/doorman-daemon --locked
```

## Package the macOS app

```bash
cargo install cargo-bundle --version 0.11.0 --locked
scripts/package-macos.sh
# Universal package (both architectures):
DOORMAN_UNIVERSAL=1 scripts/package-macos.sh
```

Packaging writes `outputs/Doorman.app` and `outputs/Doorman-macos.zip`. The universal build requires the Apple Silicon and Intel Rust targets. The release workflow checks the tag against the workspace version, tests, packages, and uploads the archive, checksum, and installer.

## Work on this documentation

The site uses Astro Starlight with the [Material Design 3 theme](https://axiaobo7788.github.io/starlight-material-design-theme/). Use Node.js 22.19 or newer. From the repository root:

```bash
cd docs
npm ci
npm run dev
```

Edit guides in `src/content/docs/`, navigation in `astro.config.mjs`, and local styling in `src/styles/custom.css`.

```bash
npm run check
npm run build
npm run preview
```

Search is indexed during the production build; use the preview server to test it. The static output is in `docs/dist/`. Set `SITE_URL` at build time when deploying to a domain other than `https://getdoorman.dev`.

When changing product behavior, update the relevant tutorial and reference together. Keep the first-app path runnable, and verify team script examples do not recurse.
