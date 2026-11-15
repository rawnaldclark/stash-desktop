import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Relative asset paths: the same build is served by `stashd serve --ui` and loaded by the desktop window.
export default defineConfig({
  base: "./",
  plugins: [svelte()],
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
  server: { port: 5199, strictPort: true },
});
