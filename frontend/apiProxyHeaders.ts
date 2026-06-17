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
