import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// base '/app/' — the daemon serves the built bundle at GET /app (see
// src/webapp.rs). The dev proxy points at a locally running daemon (or the
// scratchpad mock) so `npm run dev` exercises the real endpoints.
export default defineConfig({
  base: "/app/",
  plugins: [react()],
  server: {
    proxy: {
      "/v1": "http://127.0.0.1:8123",
      "/events": "http://127.0.0.1:8123",
      "/webhook": "http://127.0.0.1:8123",
    },
  },
  build: {
    target: "es2022",
    assetsInlineLimit: 8192,
  },
});
