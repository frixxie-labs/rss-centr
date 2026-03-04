import { assertEquals } from "@std/assert";
import {
  compactText,
  hostname,
  previewText,
  timeAgo,
  truncate,
} from "./FeedItemCard.tsx";
import type { FeedItem } from "../types.ts";

// ---------------------------------------------------------------------------
// timeAgo
// ---------------------------------------------------------------------------

Deno.test("timeAgo - returns 'just now' for times less than 60 seconds ago", () => {
  const now = Date.now();
  assertEquals(timeAgo(new Date(now).toISOString(), now), "just now");
  assertEquals(timeAgo(new Date(now - 30_000).toISOString(), now), "just now");
  assertEquals(timeAgo(new Date(now - 59_000).toISOString(), now), "just now");
});

Deno.test("timeAgo - returns minutes ago for times between 1 and 59 minutes", () => {
  const now = Date.now();
  assertEquals(timeAgo(new Date(now - 60_000).toISOString(), now), "1m ago");
  assertEquals(
    timeAgo(new Date(now - 5 * 60_000).toISOString(), now),
    "5m ago",
  );
  assertEquals(
    timeAgo(new Date(now - 59 * 60_000).toISOString(), now),
    "59m ago",
  );
});

Deno.test("timeAgo - returns hours ago for times between 1 and 23 hours", () => {
  const now = Date.now();
  assertEquals(timeAgo(new Date(now - 3600_000).toISOString(), now), "1h ago");
  assertEquals(
    timeAgo(new Date(now - 12 * 3600_000).toISOString(), now),
    "12h ago",
  );
  assertEquals(
    timeAgo(new Date(now - 23 * 3600_000).toISOString(), now),
    "23h ago",
  );
});

Deno.test("timeAgo - returns days ago for times between 1 and 29 days", () => {
  const now = Date.now();
  assertEquals(
    timeAgo(new Date(now - 24 * 3600_000).toISOString(), now),
    "1d ago",
  );
  assertEquals(
    timeAgo(new Date(now - 7 * 24 * 3600_000).toISOString(), now),
    "7d ago",
  );
  assertEquals(
    timeAgo(new Date(now - 29 * 24 * 3600_000).toISOString(), now),
    "29d ago",
  );
});

Deno.test("timeAgo - returns locale date string for 30+ days", () => {
  const now = Date.now();
  const old = new Date(now - 31 * 24 * 3600_000);
  const result = timeAgo(old.toISOString(), now);
  // Should be a locale date, not "Xd ago"
  assertEquals(result.includes("ago"), false);
  assertEquals(result, old.toLocaleDateString());
});

// ---------------------------------------------------------------------------
// hostname
// ---------------------------------------------------------------------------

Deno.test("hostname - extracts hostname from a URL", () => {
  assertEquals(hostname("https://example.com/path"), "example.com");
  assertEquals(
    hostname("https://blog.rust-lang.org/2024/"),
    "blog.rust-lang.org",
  );
});

Deno.test("hostname - strips www. prefix", () => {
  assertEquals(hostname("https://www.example.com/path"), "example.com");
  assertEquals(hostname("https://www.foo.bar.com"), "foo.bar.com");
});

Deno.test("hostname - returns empty string for invalid URL", () => {
  assertEquals(hostname("not a url"), "");
  assertEquals(hostname(""), "");
});

// ---------------------------------------------------------------------------
// compactText
// ---------------------------------------------------------------------------

Deno.test("compactText - strips HTML tags", () => {
  assertEquals(compactText("<p>Hello</p>"), "Hello");
  assertEquals(compactText("<b>bold</b> and <i>italic</i>"), "bold and italic");
});

Deno.test("compactText - collapses whitespace", () => {
  assertEquals(compactText("hello   world"), "hello world");
  assertEquals(compactText("  spaced  out  "), "spaced out");
});

Deno.test("compactText - handles combined HTML and whitespace", () => {
  assertEquals(
    compactText("<p>  Hello  </p>  <p>  World  </p>"),
    "Hello World",
  );
});

Deno.test("compactText - returns empty string for empty input", () => {
  assertEquals(compactText(""), "");
  assertEquals(compactText("   "), "");
});

// ---------------------------------------------------------------------------
// previewText
// ---------------------------------------------------------------------------

function makeFeedItem(overrides: Partial<FeedItem> = {}): FeedItem {
  return {
    id: 1,
    feed_id: 1,
    external_id: "ext-1",
    title: "Test Item",
    url: "https://example.com",
    inserted_at: "2024-01-01T00:00:00Z",
    ...overrides,
  };
}

Deno.test("previewText - returns compacted summary when available", () => {
  const item = makeFeedItem({ summary: "<p>A  summary</p>" });
  assertEquals(previewText(item), "A summary");
});

Deno.test("previewText - falls back to content when summary is empty", () => {
  const item = makeFeedItem({ summary: null, content: "<b>Content</b> here" });
  assertEquals(previewText(item), "Content here");
});

Deno.test("previewText - returns empty string when both are absent", () => {
  const item = makeFeedItem({ summary: null, content: null });
  assertEquals(previewText(item), "");
});

Deno.test("previewText - prefers summary over content", () => {
  const item = makeFeedItem({
    summary: "The summary",
    content: "The content",
  });
  assertEquals(previewText(item), "The summary");
});

// ---------------------------------------------------------------------------
// truncate
// ---------------------------------------------------------------------------

Deno.test("truncate - returns value unchanged when shorter than maxLength", () => {
  assertEquals(truncate("hello", 10), "hello");
  assertEquals(truncate("exact", 5), "exact");
});

Deno.test("truncate - truncates and adds ellipsis for long strings", () => {
  const result = truncate("hello world", 8);
  assertEquals(result, "hello w\u2026");
});

Deno.test("truncate - trims trailing whitespace before ellipsis", () => {
  // "hello world" with maxLength=7 -> slice(0,6) = "hello " -> trimEnd -> "hello" + "…"
  assertEquals(truncate("hello world", 7), "hello\u2026");
});

Deno.test("truncate - handles empty string", () => {
  assertEquals(truncate("", 5), "");
});
