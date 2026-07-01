import { assertEquals } from "@std/assert";
import {
  canFetchNow,
  feedName,
  fetchableFeedIds,
  formatDateTime,
  formatPollInterval,
  sortByNewestId,
} from "./FeedManagement.tsx";
import type { FeedSubscription } from "../types.ts";

function makeFeed(overrides: Partial<FeedSubscription> = {}): FeedSubscription {
  return {
    id: 1,
    url: "https://example.com/feed.xml",
    title: null,
    site_url: null,
    etag: null,
    last_modified: null,
    poll_interval_seconds: 3600,
    is_enabled: true,
    last_checked_at: null,
    last_success_at: null,
    last_inserted_at: null,
    failure_count: 0,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// sortByNewestId
// ---------------------------------------------------------------------------

Deno.test("sortByNewestId - sorts feeds by id descending", () => {
  const feeds = [makeFeed({ id: 1 }), makeFeed({ id: 3 }), makeFeed({ id: 2 })];
  const sorted = sortByNewestId(feeds);
  assertEquals(sorted.map((f) => f.id), [3, 2, 1]);
});

Deno.test("sortByNewestId - does not mutate original array", () => {
  const feeds = [makeFeed({ id: 1 }), makeFeed({ id: 2 })];
  const sorted = sortByNewestId(feeds);
  assertEquals(feeds.map((f) => f.id), [1, 2]);
  assertEquals(sorted.map((f) => f.id), [2, 1]);
});

Deno.test("sortByNewestId - handles empty array", () => {
  assertEquals(sortByNewestId([]), []);
});

Deno.test("sortByNewestId - handles single element", () => {
  const feeds = [makeFeed({ id: 42 })];
  const sorted = sortByNewestId(feeds);
  assertEquals(sorted.length, 1);
  assertEquals(sorted[0].id, 42);
});

// ---------------------------------------------------------------------------
// formatDateTime
// ---------------------------------------------------------------------------

Deno.test("formatDateTime - returns 'Never' for null", () => {
  assertEquals(formatDateTime(null), "Never");
});

Deno.test("formatDateTime - returns 'Unknown' for invalid date string", () => {
  assertEquals(formatDateTime("not-a-date"), "Unknown");
});

Deno.test("formatDateTime - formats valid date string", () => {
  const result = formatDateTime("2024-06-15T10:30:00Z");
  // The exact format depends on locale, but it should contain the date parts
  assertEquals(typeof result, "string");
  assertEquals(result.length > 0, true);
  assertEquals(result !== "Never", true);
  assertEquals(result !== "Unknown", true);
});

Deno.test("formatDateTime - returns 'Never' for empty string", () => {
  // Empty string is falsy in JS
  assertEquals(formatDateTime(""), "Never");
});

// ---------------------------------------------------------------------------
// feedName
// ---------------------------------------------------------------------------

Deno.test("feedName - returns title when available", () => {
  const feed = makeFeed({
    title: "My Feed",
    site_url: "https://example.com",
    url: "https://example.com/rss",
  });
  assertEquals(feedName(feed), "My Feed");
});

Deno.test("feedName - falls back to site_url when title is null", () => {
  const feed = makeFeed({ title: null, site_url: "https://example.com" });
  assertEquals(feedName(feed), "https://example.com");
});

Deno.test("feedName - falls back to url when title and site_url are null", () => {
  const feed = makeFeed({
    title: null,
    site_url: null,
    url: "https://example.com/feed.xml",
  });
  assertEquals(feedName(feed), "https://example.com/feed.xml");
});

Deno.test("feedName - skips whitespace-only title", () => {
  const feed = makeFeed({ title: "   ", site_url: "https://site.com" });
  assertEquals(feedName(feed), "https://site.com");
});

Deno.test("feedName - skips whitespace-only site_url when title is null", () => {
  const feed = makeFeed({
    title: null,
    site_url: "   ",
    url: "https://example.com/rss",
  });
  assertEquals(feedName(feed), "https://example.com/rss");
});

// ---------------------------------------------------------------------------
// canFetchNow
// ---------------------------------------------------------------------------

Deno.test("canFetchNow - true for an enabled feed", () => {
  assertEquals(canFetchNow(makeFeed({ is_enabled: true })), true);
});

Deno.test("canFetchNow - false for a paused feed", () => {
  assertEquals(canFetchNow(makeFeed({ is_enabled: false })), false);
});

Deno.test("canFetchNow - true when the feed can't be found", () => {
  // Lets the API call happen anyway so it can surface its own error, rather
  // than the UI silently blocking a request it can't actually reason about.
  assertEquals(canFetchNow(undefined), true);
});

// ---------------------------------------------------------------------------
// fetchableFeedIds
// ---------------------------------------------------------------------------

Deno.test("fetchableFeedIds - includes only enabled feeds, in list order", () => {
  const feeds = [
    makeFeed({ id: 1, is_enabled: true }),
    makeFeed({ id: 2, is_enabled: false }),
    makeFeed({ id: 3, is_enabled: true }),
  ];
  assertEquals(fetchableFeedIds(feeds), [1, 3]);
});

Deno.test("fetchableFeedIds - empty when every feed is paused", () => {
  const feeds = [makeFeed({ id: 1, is_enabled: false })];
  assertEquals(fetchableFeedIds(feeds), []);
});

Deno.test("fetchableFeedIds - empty array in, empty array out", () => {
  assertEquals(fetchableFeedIds([]), []);
});

// ---------------------------------------------------------------------------
// formatPollInterval
// ---------------------------------------------------------------------------

Deno.test("formatPollInterval - formats seconds under 60", () => {
  assertEquals(formatPollInterval(30), "30s");
  assertEquals(formatPollInterval(1), "1s");
  assertEquals(formatPollInterval(59), "59s");
});

Deno.test("formatPollInterval - formats exact minutes", () => {
  assertEquals(formatPollInterval(60), "1m");
  assertEquals(formatPollInterval(300), "5m");
  assertEquals(formatPollInterval(2700), "45m");
});

Deno.test("formatPollInterval - formats minutes with remaining seconds", () => {
  assertEquals(formatPollInterval(90), "1m 30s");
  assertEquals(formatPollInterval(125), "2m 5s");
});

Deno.test("formatPollInterval - formats exact hours", () => {
  assertEquals(formatPollInterval(3600), "1h");
  assertEquals(formatPollInterval(7200), "2h");
});

Deno.test("formatPollInterval - formats hours with minutes", () => {
  assertEquals(formatPollInterval(3660), "1h 1m");
  assertEquals(formatPollInterval(5400), "1h 30m");
  assertEquals(formatPollInterval(9000), "2h 30m");
});

Deno.test("formatPollInterval - hours format omits remaining seconds", () => {
  // When hours > 0 and minutes > 0, remaining seconds are dropped
  assertEquals(formatPollInterval(3661), "1h 1m");
  // When hours > 0 and minutes = 0, seconds are dropped too
  assertEquals(formatPollInterval(3601), "1h");
});

Deno.test("formatPollInterval - handles zero seconds", () => {
  assertEquals(formatPollInterval(0), "0s");
});
