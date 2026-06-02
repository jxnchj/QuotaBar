import { defineConfig } from "vite";

// Tauri dev server settings: fixed port, no auto-open browser, watch host/network.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      // Don't watch the Rust crate (cargo handles its own rebuilds).
      ignored: ["**/src-tauri/**"],
    },
  },
});
