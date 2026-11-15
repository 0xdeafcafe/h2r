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
    },
  },
  build: { outDir: "dist", target: "es2022" },
});
