import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { defineCollection } from "astro:content";

const REPO = "https://github.com/korora-tech/rockhopper";
const README = fileURLToPath(new URL("../../README.md", import.meta.url));

// The docs page is the repository README, so the two never drift apart.
// Links relative to the repository root are pointed at GitHub, and the
// title is dropped because the page supplies its own.
function prepare(markdown: string): string {
  return markdown
    .replace(/^# .*\n+/, "")
    .replace(/\]\((?!https?:|#|mailto:)([^)\s]+)\)/g, `](${REPO}/blob/main/$1)`);
}

const docs = defineCollection({
  loader: {
    name: "readme",
    async load({ store, renderMarkdown, generateDigest, watcher }) {
      const sync = async () => {
        const body = prepare(await readFile(README, "utf8"));
        store.set({
          id: "readme",
          data: {},
          body,
          digest: generateDigest(body),
          rendered: await renderMarkdown(body),
        });
      };
      await sync();
      watcher?.add(README);
      watcher?.on("change", (path) => {
        if (path === README) void sync();
      });
    },
  },
});

export const collections = { docs };
