import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Built to dist/, which loom serves (LOOM_WEB_UI, or <data dir>/loom/web). Its CSP takes
// scripts and styles from the page's own origin only: nothing inline, nothing from elsewhere.
export default defineConfig({
  plugins: [svelte()],
  base: "/",
  build: { outDir: "dist", emptyOutDir: true, assetsInlineLimit: 0, modulePreload: { polyfill: false } },
});
