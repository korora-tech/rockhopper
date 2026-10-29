# rockhopper.korora.tech

The Rockhopper website: an [Astro](https://astro.build) static site served by Cloudflare Workers static assets.

- `src/pages/index.astro`: the home page, with an interactive copy of the launcher (`src/components/Launcher.astro`, sample data in `src/lib/demo.ts`).
- `src/pages/docs.astro`: the repository `README.md`, rendered at build time by the loader in `src/content.config.ts`. Edit the README, not this site, to change the docs.

## Develop

Needs Node 22.23.4 or newer (`.node-version` pins 24).

```sh
npm ci
npm run dev       # http://localhost:4321
npm run check     # type-check
npm run preview   # build, then serve dist/ through wrangler as Cloudflare would
```

## Deploy

`.github/workflows/website.yml` builds every pull request that touches `website/` or `README.md`, and runs `wrangler deploy` on `main`. It needs two repository secrets:

| Secret | Value |
|---|---|
| `CLOUDFLARE_API_TOKEN` | An account API token from the *Edit Cloudflare Workers* template, scoped to that account and the `korora.tech` zone. The first deploy creates the Worker, which needs Workers Admin; attaching the custom domain needs *Zone › Workers Routes › Write* on `korora.tech`. |
| `CLOUDFLARE_ACCOUNT_ID` | The account that holds the `korora.tech` zone |

`wrangler.jsonc` attaches the Worker to `rockhopper.korora.tech` as a custom domain, so Cloudflare creates the DNS record and certificate on first deploy. The `korora.tech` zone must be in the same account.

Response headers (CSP, caching for hashed assets) live in `public/_headers`.
