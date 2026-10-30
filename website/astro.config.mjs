// @ts-check
import cloudflare from "@astrojs/cloudflare";
import { defineConfig, fontProviders } from "astro/config";

export default defineConfig({
  site: "https://rockhopper.korora.tech",
  // Static output: the adapter writes the Build Output cf deploys, and the
  // Worker serves only static assets.
  output: "static",
  adapter: cloudflare(),
  trailingSlash: "never",
  build: {
    format: "file",
  },
  fonts: [
    {
      provider: fontProviders.fontsource(),
      name: "Geist",
      cssVariable: "--font-sans",
      weights: ["100 900"],
      fallbacks: ["system-ui", "sans-serif"],
    },
    {
      provider: fontProviders.fontsource(),
      name: "Geist Mono",
      cssVariable: "--font-mono",
      weights: ["100 900"],
      fallbacks: ["ui-monospace", "monospace"],
    },
  ],
  vite: {
    build: {
      // Keep every script in a file, so the Content-Security-Policy in
      // public/_headers can stay at script-src 'self'.
      assetsInlineLimit: 0,
    },
  },
  markdown: {
    shikiConfig: {
      themes: { light: "github-light", dark: "github-dark-default" },
      defaultColor: false,
    },
  },
});
