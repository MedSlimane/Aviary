# Aviary site

The marketing site is a static Vite build. It has no server runtime, Pages
Functions, environment secrets, or external data dependencies.

## Local build

```bash
bun install --frozen-lockfile
bun run build
```

The deployable output is written to `dist/`.

## Cloudflare Pages

Connect the `MedSlimane/Aviary` GitHub repository and use these settings:

| Setting | Value |
|---|---|
| Production branch | `main` |
| Root directory | `site` |
| Build command | `bun install --frozen-lockfile && bun run build` |
| Build output directory | `dist` |
| Build system | `v3` |

Set `BUN_VERSION` to `1.3.13` for both production and preview builds. Node is
pinned in `.node-version`; Pages build image v3 supports both runtimes.

The project intentionally has no Wrangler configuration. Git-integrated static
Pages projects do not require one, and adding `pages_build_output_dir` would
make that file the source of truth for overlapping dashboard configuration.

`public/_headers` is copied into the build for Cloudflare to apply to static
responses. The top-level `404.html` prevents unknown URLs from being treated as
single-page-app routes.
