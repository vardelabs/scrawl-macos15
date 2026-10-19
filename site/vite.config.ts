import { defineConfig, type Plugin } from "vite"
import react from "@vitejs/plugin-react"
import { agentFiles, jsonLd } from "./scripts/agent-files.ts"

const TYPES: Record<string, string> = { ".txt": "text/plain; charset=utf-8", ".md": "text/markdown; charset=utf-8" }

// llms.txt, llms-full.txt and skill.md at the site root, plus the JSON-LD in index.html.
function forAgents(): Plugin {
  return {
    name: "scrawl-agent-files",
    transformIndexHtml: () => [{ tag: "script", attrs: { type: "application/ld+json" }, children: jsonLd(), injectTo: "head" }],
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const name = req.url?.split("?")[0].slice(1) ?? ""
        const file = agentFiles()[name]
        if (file === undefined) return next()
        res.setHeader("Content-Type", TYPES[name.slice(name.lastIndexOf("."))])
        res.end(file)
      })
    },
    generateBundle() {
      if (this.environment?.config.build.ssr) return
      for (const [fileName, source] of Object.entries(agentFiles())) this.emitFile({ type: "asset", fileName, source })
    },
  }
}

export default defineConfig({
  plugins: [react(), forAgents()],
  // three.js lives in the lazily loaded hero brush chunk.
  build: { chunkSizeWarningLimit: 700 },
  // Previews are shared over Tailscale (studio.*.ts.net).
  server: { allowedHosts: [".ts.net"] },
  preview: { allowedHosts: [".ts.net"] },
})
