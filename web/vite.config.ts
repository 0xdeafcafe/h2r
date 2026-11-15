import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const mock = process.env.H2_LIVE_URL ?? "http://127.0.0.1:8075";

export default defineConfig({
  plugins: [solid()],
  base: "/",
  server: {
    port: 5174,
    proxy: {
      // changeOrigin stays off so the mock's well-known names this origin.
      "/v1": { target: mock, ws: true },
      "/healthz": { target: mock },
      "/.well-known": { target: mock },
      // Optional game art; no forwarding headers, so a local server still sees a loopback client.
      // H2_GAME_ART_URL points just this at another server (say, a real one beside the mock).
      "/art/game": { target: process.env.H2_GAME_ART_URL ?? mock },
    },
  },
  build: { outDir: "dist", target: "es2022" },
});
