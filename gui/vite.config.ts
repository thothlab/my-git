import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const host = process.env.TAURI_DEV_HOST;

// Vite is driven by Tauri (beforeDevCommand). Port is pinned so tauri.conf.json's
// devUrl matches; a mismatch here is the classic "blank window".
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    // Non-default port so my-git's dev server never collides with another Tauri
    // project (they all default to 1420). Must match tauri.conf.json's devUrl.
    port: 1425,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1426 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    minify: "esbuild",
    sourcemap: false,
    emptyOutDir: true,
  },
});
