import { define } from "../../utils.ts";
import { BACKEND_URL } from "../../utils.ts";
import { getLogger } from "../../logger.ts";

const log = getLogger("api-proxy");

/** Forward a proxied response back to the client, logging the outcome. */
function proxyResponse(
  method: string,
  path: string,
  res: Response,
  extraHeaders?: HeadersInit,
): Response {
  const contentType = res.headers.get("Content-Type") ?? "";
  const headers = new Headers({ "Content-Type": contentType, ...extraHeaders });

  if (!res.ok) {
    log.warn("Backend returned error", {
      method,
      path,
      status: res.status,
    });
  } else {
    log.debug("Proxy response", { method, path, status: res.status });
  }

  return new Response(res.body, { status: res.status, headers });
}

// Proxy all /api/* requests to the Rust backend.
// This handles REST endpoints and the SSE stream.
export const handler = define.handlers({
  async GET(ctx) {
    const path = ctx.params.path;
    const url = new URL(ctx.req.url);
    const target = `${BACKEND_URL}/api/${path}${url.search}`;

    const headers = new Headers();
    // Forward relevant headers
    const lastEventId = ctx.req.headers.get("Last-Event-ID");
    if (lastEventId) {
      headers.set("Last-Event-ID", lastEventId);
    }
    const accept = ctx.req.headers.get("Accept");
    if (accept) {
      headers.set("Accept", accept);
    }

    try {
      const res = await fetch(target, { headers });

      // For SSE, stream the response body through
      const contentType = res.headers.get("Content-Type") ?? "";
      const extra: Record<string, string> = {};
      if (contentType.includes("text/event-stream")) {
        extra["Cache-Control"] = "no-cache";
        extra["Connection"] = "keep-alive";
        log.info("SSE stream opened", { path });
      }

      return proxyResponse("GET", path, res, extra);
    } catch (err) {
      log.error("Backend unreachable", { method: "GET", path }, err);
      return new Response("Bad Gateway", { status: 502 });
    }
  },

  async POST(ctx) {
    const path = ctx.params.path;
    const target = `${BACKEND_URL}/api/${path}`;
    const body = await ctx.req.text();

    try {
      const res = await fetch(target, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body,
      });

      return proxyResponse("POST", path, res);
    } catch (err) {
      log.error("Backend unreachable", { method: "POST", path }, err);
      return new Response("Bad Gateway", { status: 502 });
    }
  },

  async PUT(ctx) {
    const path = ctx.params.path;
    const target = `${BACKEND_URL}/api/${path}`;
    const body = await ctx.req.text();

    try {
      const res = await fetch(target, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body,
      });

      return proxyResponse("PUT", path, res);
    } catch (err) {
      log.error("Backend unreachable", { method: "PUT", path }, err);
      return new Response("Bad Gateway", { status: 502 });
    }
  },

  async DELETE(ctx) {
    const path = ctx.params.path;
    const target = `${BACKEND_URL}/api/${path}`;

    try {
      const res = await fetch(target, { method: "DELETE" });

      return proxyResponse("DELETE", path, res);
    } catch (err) {
      log.error("Backend unreachable", { method: "DELETE", path }, err);
      return new Response("Bad Gateway", { status: 502 });
    }
  },
});
