// Every request comes through here first (run_worker_first in wrangler.jsonc):
// other hosts and plain HTTP redirect to https://scrawl.kitlangton.com, and
// hashed build assets get a long cache. Release files and the appcast are
// returned exactly as uploaded, on any host, so shipped apps keep updating.
// `wrangler dev --var ANY_HOST:1` turns the redirect off for local testing.
type Env = { ASSETS: { fetch(request: Request): Promise<Response> }; ANY_HOST?: string }

const HOST = "scrawl.kitlangton.com"

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url)
    if (url.pathname === "/appcast.xml" || url.pathname.startsWith("/releases/")) return env.ASSETS.fetch(request)

    if (!env.ANY_HOST && (url.hostname !== HOST || url.protocol !== "https:")) {
      url.protocol = "https:"
      url.hostname = HOST
      url.port = ""
      return Response.redirect(url.toString(), 301)
    }

    const response = await env.ASSETS.fetch(request)
    const headers = new Headers(response.headers)
    if (url.pathname.startsWith("/assets/")) headers.set("Cache-Control", "public, max-age=31536000, immutable")
    if (url.protocol === "https:") headers.set("Strict-Transport-Security", "max-age=31536000")
    headers.set("X-Content-Type-Options", "nosniff")
    headers.set("Referrer-Policy", "strict-origin-when-cross-origin")
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers })
  },
}
