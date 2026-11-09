import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

// The build goes into the dlux crate, whose build script embeds it into the binary.
// `npm run dev` proxies the API to a `dlux serve` on :8080.
export default defineConfig({
  plugins: [svelte(), tailwindcss()],
  build: {
    outDir: "../crates/dlux/web-dist",
    emptyOutDir: true,
    assetsDir: "assets",
    sourcemap: false,
  },
  server: {
    proxy: {
      "/api": { target: "http://127.0.0.1:8080", ws: true, changeOrigin: false },
    },
  },
});
