import { assertEquals } from "@std/assert";
import {
  filterProxyHeaders,
  forwardRequestHeaders,
} from "../../apiProxyHeaders.ts";

// ---------------------------------------------------------------------------
// filterProxyHeaders
// ---------------------------------------------------------------------------

Deno.test("filterProxyHeaders - passes through normal headers", () => {
  const input = new Headers({
    "Content-Type": "application/json",
    "X-Custom": "value",
    "Cache-Control": "no-cache",
  });
  const result = filterProxyHeaders(input);
  assertEquals(result.get("Content-Type"), "application/json");
  assertEquals(result.get("X-Custom"), "value");
  assertEquals(result.get("Cache-Control"), "no-cache");
});

Deno.test("filterProxyHeaders - strips hop-by-hop headers", () => {
  const input = new Headers({
    "Content-Type": "text/html",
    "Connection": "keep-alive",
    "Keep-Alive": "timeout=5",
    "Transfer-Encoding": "chunked",
    "Upgrade": "websocket",
    "TE": "trailers",
    "Trailers": "x",
    "Proxy-Authenticate": "Basic",
    "Proxy-Authorization": "Basic abc",
  });
  const result = filterProxyHeaders(input);
  assertEquals(result.get("Content-Type"), "text/html");
  assertEquals(result.get("Connection"), null);
  assertEquals(result.get("Keep-Alive"), null);
  assertEquals(result.get("Transfer-Encoding"), null);
  assertEquals(result.get("Upgrade"), null);
  assertEquals(result.get("TE"), null);
  assertEquals(result.get("Trailers"), null);
  assertEquals(result.get("Proxy-Authenticate"), null);
  assertEquals(result.get("Proxy-Authorization"), null);
});

Deno.test("filterProxyHeaders - handles empty headers", () => {
  const result = filterProxyHeaders(new Headers());
  assertEquals([...result.entries()].length, 0);
});

Deno.test("filterProxyHeaders - case-insensitive hop-by-hop filtering", () => {
  const input = new Headers();
  input.set("CONNECTION", "close");
  input.set("content-type", "text/plain");
  const result = filterProxyHeaders(input);
  assertEquals(result.get("connection"), null);
  assertEquals(result.get("content-type"), "text/plain");
});

// ---------------------------------------------------------------------------
// forwardRequestHeaders
// ---------------------------------------------------------------------------

Deno.test("forwardRequestHeaders - forwards allowlisted headers", () => {
  const req = new Request("https://example.com", {
    headers: {
      "Accept": "application/json",
      "Authorization": "Bearer token123",
      "Content-Type": "application/json",
      "Cache-Control": "no-cache",
      "If-Match": '"etag-value"',
      "If-Modified-Since": "Mon, 01 Jan 2024 00:00:00 GMT",
      "If-None-Match": '"another-etag"',
      "Last-Event-ID": "42",
    },
  });
  const result = forwardRequestHeaders(req);
  assertEquals(result.get("Accept"), "application/json");
  assertEquals(result.get("Authorization"), "Bearer token123");
  assertEquals(result.get("Content-Type"), "application/json");
  assertEquals(result.get("Cache-Control"), "no-cache");
  assertEquals(result.get("If-Match"), '"etag-value"');
  assertEquals(result.get("If-None-Match"), '"another-etag"');
  assertEquals(result.get("Last-Event-ID"), "42");
});

Deno.test("forwardRequestHeaders - does not forward non-allowlisted headers", () => {
  const req = new Request("https://example.com", {
    headers: {
      "Accept": "application/json",
      "Cookie": "session=abc",
      "X-Custom-Header": "value",
      "User-Agent": "test-agent",
      "Host": "example.com",
      "Referer": "https://other.com",
    },
  });
  const result = forwardRequestHeaders(req);
  assertEquals(result.get("Accept"), "application/json");
  assertEquals(result.get("Cookie"), null);
  assertEquals(result.get("X-Custom-Header"), null);
  assertEquals(result.get("User-Agent"), null);
  assertEquals(result.get("Host"), null);
  assertEquals(result.get("Referer"), null);
});

Deno.test("forwardRequestHeaders - handles request with no matching headers", () => {
  const req = new Request("https://example.com", {
    headers: {
      "X-Foo": "bar",
    },
  });
  const result = forwardRequestHeaders(req);
  assertEquals([...result.entries()].length, 0);
});
