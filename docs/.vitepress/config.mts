import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { defineConfig } from "vitepress";

const base = "/slop-warded-server/";

// VitePress fails the build on a link to a missing page. This also fails it on a link to a
// missing heading of an existing page, which a renamed heading would otherwise leave behind.
function checkFragments(outDir: string) {
  const ids = new Map<string, Set<string>>();
  const links: { page: string; target: string; fragment: string }[] = [];
  for (const entry of readdirSync(outDir, { recursive: true, encoding: "utf8" })) {
    const page = entry.replaceAll("\\", "/");
    if (!page.endsWith(".html")) continue;
    const html = readFileSync(path.join(outDir, entry), "utf8");
    ids.set(page, new Set(Array.from(html.matchAll(/\sid="([^"]*)"/g), (match) => match[1])));
    for (const [, href] of html.matchAll(/\shref="([^"]*#[^"]*)"/g)) {
      const [address, fragment] = href.split("#", 2);
      if (!fragment || /^[a-z][a-z0-9+.-]*:/i.test(address)) continue;
      const target = address.startsWith(base)
        ? address.slice(base.length) || "index.html"
        : address
          ? path.posix.join(path.posix.dirname(page), address)
          : page;
      links.push({ page, target, fragment: decodeURIComponent(fragment) });
    }
  }
  const broken = links.filter(({ target, fragment }) => !ids.get(target)?.has(fragment));
  if (broken.length > 0) {
    const list = broken.map(({ page, target, fragment }) => `${page}: ${target}#${fragment}`);
    throw new Error(`Links to missing headings:\n${list.join("\n")}`);
  }
}

// The version the site describes. The Pages workflow builds the site from that release's tag,
// whose Cargo.toml names it; scripts/check.sh keeps the pages' pins in step with it.
const manifest = readFileSync(new URL("../../Cargo.toml", import.meta.url), "utf8");
const version = /^version = "([^"]+)"$/m.exec(manifest)?.[1];
if (!version) throw new Error("Cargo.toml names no package version");

const repository = "https://github.com/viell-dev/slop-warded-server";
// The Pages workflow adds rustdoc for the same commit, because no version is on docs.rs.
const api = `https://viell-dev.github.io${base}api/warded_server/`;

type Item = { text: string; link: string };
const start: Item[] = [
  { text: "Getting started", link: "/getting-started" },
  { text: "Routes and requests", link: "/requests" },
  { text: "Glossary", link: "/glossary" },
];
const templates: Item[] = [
  { text: "Minimal", link: "/template-minimal" },
  { text: "API", link: "/template-api" },
  { text: "Website", link: "/template-website" },
];
const projects: Item[] = [
  { text: "A JSON API", link: "/json-api" },
  { text: "A website", link: "/website" },
  { text: "A single-page app", link: "/single-page-app" },
  { text: "Existing Axum applications", link: "/existing-axum" },
];
const guides: Item[] = [
  { text: "Middleware", link: "/middleware" },
  { text: "Streaming and WebSockets", link: "/streaming" },
  { text: "Testing", link: "/testing" },
];
const operation: Item[] = [
  { text: "Configuration", link: "/configuration" },
  { text: "Certificates", link: "/certificates" },
  { text: "HTTP/3", link: "/http3" },
  { text: "Deployment", link: "/deployment" },
];
const help: Item[] = [
  { text: "Troubleshooting", link: "/troubleshooting" },
  { text: "Limits", link: "/limits" },
];
const releases: Item[] = [
  { text: "Versions", link: "/versions" },
  { text: "Changelog", link: "/changelog" },
];

export default defineConfig({
  title: "Warded Server",
  description:
    "A ready-wired Rust server for HTTP/1.1, HTTP/2 and HTTP/3, with the Warded router and project templates.",
  lang: "en",
  base,
  buildEnd: ({ outDir }) => checkFragments(outDir),
  head: [["meta", { name: "theme-color", content: "#3c6e47" }]],
  sitemap: { hostname: "https://viell-dev.github.io/slop-warded-server/" },
  // Pages and their example checks; everything else in this directory builds or tests them.
  srcExclude: ["**/node_modules/**"],
  markdown: {
    // rustdoc's mark for examples that are compiled and not run.
    languageAlias: { "rust,no_run": "rust" },
    languageLabel: { "rust,no_run": "rust" },
    // Show `{{` in code spans as written; Vue would otherwise read it as an interpolation.
    config(md) {
      md.core.ruler.push("code_inline_v_pre", (state) => {
        for (const token of state.tokens)
          for (const child of token.children ?? [])
            if (child.type === "code_inline") child.attrSet("v-pre", "");
      });
    },
  },
  themeConfig: {
    nav: [
      { text: "Guide", items: [...start, ...guides] },
      {
        text: "Projects",
        items: [{ text: "Templates", items: templates }, { text: "Task guides", items: projects }],
      },
      { text: "Running it", items: operation },
      { text: "Help", items: help },
      { text: "API reference", link: api },
      { text: version, items: releases },
    ],
    sidebar: [
      { text: "Start here", items: start },
      { text: "Templates", items: templates },
      { text: "Projects", items: projects },
      { text: "Guides", items: guides },
      { text: "Running it", items: operation },
      { text: "Help", items: help },
      {
        text: "Reference",
        items: [{ text: "API reference", link: api }, ...releases],
      },
    ],
    search: { provider: "local" },
    outline: { level: [2, 3] },
    socialLinks: [{ icon: "github", link: repository }],
    editLink: {
      pattern: `${repository}/edit/main/docs/:path`,
      text: "Suggest a change to this page on main",
    },
    footer: {
      message: `Agent-maintained · <a href="${repository}/blob/${version}/LICENSE-MIT">MIT</a> OR <a href="${repository}/blob/${version}/LICENSE-APACHE">Apache-2.0</a>`,
    },
  },
});
