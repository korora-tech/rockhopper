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
npm run preview   # serve the site locally with cf dev, as Cloudflare would
```

## Deploy

`.github/workflows/website.yml` builds every pull request that touches `website/` or `README.md`, and runs `cf deploy` on `main` with the Korora Tech organization's Cloudflare token (`CLOUDLFLARE_API_TOKEN`).

`cloudflare.config.ts` names the account and attaches the Worker to `rockhopper.korora.tech` as a custom domain, so Cloudflare creates the DNS record and certificate on first deploy. The `korora.tech` zone must be in the same account.

Response headers (CSP, caching for hashed assets) live in `public/_headers`.
