import path from "node:path";
import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/** With `base: "/flow/"`, only `/flow/` is a valid dev entry; `/flow` returns 404 without this. */
function flowSlashRedirect(): Plugin {
  return {
    name: "flow-slash-redirect",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const path = ((req as { url?: string }).url ?? "").split("?")[0];
        if (path === "/flow") {
          res.statusCode = 302;
          res.setHeader("Location", "/flow/");
          res.end();
          return;
        }
        next();
      });
    },
  };
}

export default defineConfig({
  plugins: [react(), tailwindcss(), flowSlashRedirect()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  base: "/flow/",
  build: {
    outDir: "../app/static/dist",
    emptyOutDir: true,
  },
  server: {
    port: 5174,
    proxy: {
      // Minimal HTML UI + API docs live on FastAPI (18410), not on Vite.
      "/ui": "http://127.0.0.1:18410",
      "/docs": "http://127.0.0.1:18410",
      "/openapi.json": "http://127.0.0.1:18410",
      "/processes": "http://127.0.0.1:18410",
      "/teams": "http://127.0.0.1:18410",
      "/projects": "http://127.0.0.1:18410",
      "/api": "http://127.0.0.1:18410",
      "/health": "http://127.0.0.1:18410",
    },
  },
});
