import { renderAnalyticsEventType } from "./analyticsFormatting.ts";

function assertEquals<T>(actual: T, expected: T) {
  if (actual !== expected) {
    throw new Error(`Expected ${expected} but got ${actual}`);
  }
}

Deno.test("renderAnalyticsEventType replaces underscores with spaces", () => {
  assertEquals(
    renderAnalyticsEventType("feed_fetch_requested"),
    "feed fetch requested",
  );
});
