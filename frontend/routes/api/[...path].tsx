import { define } from "../../utils.ts";
import { BACKEND_URL } from "../../utils.ts";
import { getLogger } from "../../logger.ts";

const log = getLogger("api-proxy");
const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailers",
  "transfer-encoding",
  "upgrade",
]);

export function filterProxyHeaders(headers: Headers): Headers {
  const filtered = new Headers();
  for (const [key, value] of headers.entries()) {
    if (!HOP_BY_HOP_HEADERS.has(key.toLowerCase())) {
      filtered.set(key, value);
    }
  }
  return filtered;
}

export function forwardRequestHeaders(req: Request): Headers {
  const forwarded = new Headers();
  const allowList = [
    "accept",
    "authorization",
    "cache-control",
    "content-type",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "last-event-id",
  ];

  for (const header of allowList) {
    const value = req.headers.get(header);
    if (value) {
      forwarded.set(header, value);
    }
  }

  return forwarded;
}

/** Forward a proxied response back to the client, logging the outcome. */
function proxyResponse(
  method: string,
  path: string,
  res: Response,
  extraHeaders?: HeadersInit,
): Response {
  const headers = filterProxyHeaders(res.headers);
  if (extraHeaders) {
    for (const [key, value] of Object.entries(extraHeaders)) {
      headers.set(key, value);
    }
  }

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

interface ProxyContext {
  params: Record<string, string>;
  req: Request;
}

async function forwardToBackend(ctx: ProxyContext, method: string) {
  const path = ctx.params.path;
  if (!path) {
    log.warn("Missing proxy path", { method });
    return new Response("Bad Request", { status: 400 });
  }
  const incomingUrl = new URL(ctx.req.url);
  const target = `${BACKEND_URL}/api/${path}${incomingUrl.search}`;
  const headers = forwardRequestHeaders(ctx.req);

  try {
    const options: RequestInit = {
      method,
      headers,
    };

    if (method !== "GET" && method !== "DELETE") {
      const body = await ctx.req.arrayBuffer();
      options.body = body.byteLength > 0 ? body : null;
    }

    const res = await fetch(target, options);

    const contentType = res.headers.get("Content-Type") ?? "";
    const extra: Record<string, string> = {};
    if (contentType.includes("text/event-stream")) {
      extra["Cache-Control"] = "no-cache";
      extra["Connection"] = "keep-alive";
      log.info("SSE stream opened", { path });
    }

    return proxyResponse(method, path, res, extra);
  } catch (err) {
    log.error("Backend unreachable", { method, path }, err);
    return new Response("Bad Gateway", { status: 502 });
  }
}

// Proxy all /api/* requests to the Rust backend.
// This handles REST endpoints and the SSE stream.
export const handler = define.handlers({
  async GET(ctx) {
    return await forwardToBackend(ctx, "GET");
  },

  async POST(ctx) {
    return await forwardToBackend(ctx, "POST");
  },

  async PUT(ctx) {
    return await forwardToBackend(ctx, "PUT");
  },

  async DELETE(ctx) {
    return await forwardToBackend(ctx, "DELETE");
  },
});
