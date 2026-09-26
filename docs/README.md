# Doorman documentation

An Astro Starlight site using [starlight-theme-md3](https://axiaobo7788.github.io/starlight-material-design-theme/).

Use Node.js 22.19 or newer (including the requirements of build dependencies).

```bash
cd docs
npm ci
npm run dev
```

The reading path is: understand Doorman → install → run an app → integrate a project → onboard a team → everyday workflows → reference. Start with `src/content/docs/index.mdx`; navigation is explicit in `astro.config.mjs`.

```bash
npm run check
npm run build
npm run preview
```

The build emits a static site to `dist/` and indexes search. Preview the production build to test search. Prebuild/predev copy the repository installer, original screenshot, and licensed Geist fonts from the desktop app into `public/`, so the site serves `/install.sh` without a second maintained copy.

The canonical production origin defaults to `https://getdoorman.dev`. Override `SITE_URL` for another domain. Deploy `dist/` to a host at the domain root; the navigation uses root-relative links. The Documentation workflow checks and builds pull requests without deploying them. Pushes to `main` (or a manual run on `main`) upload `docs/dist/` as a Pages artifact and deploy it to the `github-pages` environment. In repository Settings → Pages, select **GitHub Actions** as the source and keep **getdoorman.dev** as the custom domain. Do not publish the `docs/` source directory directly.

Content is in `src/content/docs/`. Keep examples grounded in the CLI, explain expected results, and update tutorials alongside reference changes. Do not put a Doorman wrapper in `dev` without pointing `script` to the underlying server script.
