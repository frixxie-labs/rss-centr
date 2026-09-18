import { assertEquals } from "@std/assert";

import {
  normalizeAnalyticsPath,
  sanitizeAnalyticsReferrer,
} from "./analytics.ts";

Deno.test("normalizeAnalyticsPath falls back to root for invalid values", () => {
  assertEquals(normalizeAnalyticsPath(undefined), "/");
  assertEquals(normalizeAnalyticsPath(""), "/");
  assertEquals(normalizeAnalyticsPath("news"), "/");
});

Deno.test("normalizeAnalyticsPath keeps absolute paths", () => {
  assertEquals(normalizeAnalyticsPath("/news"), "/news");
});

Deno.test("sanitizeAnalyticsReferrer keeps same-origin paths only", () => {
  const currentUrl = new URL("https://rss.example/news?q=rust");
  assertEquals(
    sanitizeAnalyticsReferrer("https://rss.example/topics", currentUrl),
    "/topics",
  );
});

Deno.test("sanitizeAnalyticsReferrer reduces cross-origin referrers to origin", () => {
  const currentUrl = new URL("https://rss.example/news");
  assertEquals(
    sanitizeAnalyticsReferrer("https://search.example/results?q=rss", currentUrl),
    "https://search.example",
  );
});
