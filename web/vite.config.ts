import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const agentPlatformProxyTarget =
  process.env.AGENT_PLATFORM_PROXY_TARGET ?? "http://127.0.0.1:18410";

/** With `base: "/flow/"`, only `/flow/` is a valid dev entry; `/flow` returns 404 without this. */
function flowSlashRedirect(): Plugin {
  return {
    name: "flow-slash-redirect",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const p = ((req as { url?: string }).url ?? "").split("?")[0];
        if (p === "/flow") {
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
    port: 3333,
    proxy: {
      "/ui": agentPlatformProxyTarget,
      "/docs": agentPlatformProxyTarget,
      "/openapi.json": agentPlatformProxyTarget,
      "/processes": agentPlatformProxyTarget,
      "/teams": agentPlatformProxyTarget,
      "/projects": agentPlatformProxyTarget,
      "/api": agentPlatformProxyTarget,
      "/health": agentPlatformProxyTarget,
    },
  },
});
