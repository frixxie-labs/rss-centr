import { assertEquals } from "@std/assert";
import {
  chooseResumeCursor,
  effectiveDate,
  MAX_TIMELINE_ITEMS,
  parseReplayDoneEvent,
  sortByNewest,
  upsertByNewest,
  upsertManyByNewest,
} from "./Timeline.tsx";
import type { FeedItem } from "../types.ts";

function makeFeedItem(overrides: Partial<FeedItem> = {}): FeedItem {
  return {
    id: 1,
    feed_id: 1,
    external_id: "ext-1",
    title: "Test Item",
    url: "https://example.com",
    inserted_at: "2025-01-01T00:00:00Z",
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// effectiveDate
// ---------------------------------------------------------------------------

Deno.test("effectiveDate - uses published_at when present", () => {
  const item = makeFeedItem({
    published_at: "2024-06-15T10:00:00Z",
    inserted_at: "2024-06-16T08:00:00Z",
  });
  assertEquals(effectiveDate(item), new Date("2024-06-15T10:00:00Z").getTime());
});

Deno.test("effectiveDate - clamps future published_at to inserted_at", () => {
  const item = makeFeedItem({
    published_at: "2030-01-01T00:00:00Z",
    inserted_at: "2024-06-14T08:00:00Z",
  });
  assertEquals(effectiveDate(item), new Date("2024-06-14T08:00:00Z").getTime());
});

Deno.test("effectiveDate - falls back to inserted_at when published_at is null", () => {
  const item = makeFeedItem({
    published_at: null,
    inserted_at: "2024-06-14T08:00:00Z",
  });
  assertEquals(effectiveDate(item), new Date("2024-06-14T08:00:00Z").getTime());
});

Deno.test("effectiveDate - falls back to inserted_at when published_at is undefined", () => {
  const item = makeFeedItem({
    inserted_at: "2024-06-14T08:00:00Z",
  });
  // published_at defaults to undefined from makeFeedItem
  assertEquals(effectiveDate(item), new Date("2024-06-14T08:00:00Z").getTime());
});

// ---------------------------------------------------------------------------
// sortByNewest
// ---------------------------------------------------------------------------

Deno.test("sortByNewest - sorts items by date descending", () => {
  const items = [
    makeFeedItem({ id: 1, published_at: "2024-01-01T00:00:00Z" }),
    makeFeedItem({ id: 2, published_at: "2024-06-01T00:00:00Z" }),
    makeFeedItem({ id: 3, published_at: "2024-03-01T00:00:00Z" }),
  ];
  const sorted = sortByNewest(items);
  assertEquals(sorted.map((i) => i.id), [2, 3, 1]);
});

Deno.test("sortByNewest - breaks date ties by id descending", () => {
  const sameDate = "2024-06-01T00:00:00Z";
  const items = [
    makeFeedItem({ id: 1, published_at: sameDate }),
    makeFeedItem({ id: 5, published_at: sameDate }),
    makeFeedItem({ id: 3, published_at: sameDate }),
  ];
  const sorted = sortByNewest(items);
  assertEquals(sorted.map((i) => i.id), [5, 3, 1]);
});

Deno.test("sortByNewest - does not mutate original array", () => {
  const items = [
    makeFeedItem({ id: 1, published_at: "2024-01-01T00:00:00Z" }),
    makeFeedItem({ id: 2, published_at: "2024-06-01T00:00:00Z" }),
  ];
  const sorted = sortByNewest(items);
  assertEquals(items.map((i) => i.id), [1, 2]);
  assertEquals(sorted.map((i) => i.id), [2, 1]);
});

Deno.test("sortByNewest - does not let future published_at sort first", () => {
  const items = [
    makeFeedItem({
      id: 1,
      published_at: "2030-01-01T00:00:00Z",
      inserted_at: "2024-01-01T00:00:00Z",
    }),
    makeFeedItem({
      id: 2,
      published_at: "2024-01-02T00:00:00Z",
      inserted_at: "2024-01-02T00:00:00Z",
    }),
  ];
  const sorted = sortByNewest(items);
  assertEquals(sorted.map((i) => i.id), [2, 1]);
});

Deno.test("sortByNewest - handles empty array", () => {
  assertEquals(sortByNewest([]), []);
});

Deno.test("sortByNewest - handles single item", () => {
  const items = [makeFeedItem({ id: 42 })];
  const sorted = sortByNewest(items);
  assertEquals(sorted.length, 1);
  assertEquals(sorted[0].id, 42);
});

// ---------------------------------------------------------------------------
// upsertByNewest
// ---------------------------------------------------------------------------

Deno.test("upsertByNewest - inserts new item in correct sorted position", () => {
  const items = sortByNewest([
    makeFeedItem({ id: 1, published_at: "2024-01-01T00:00:00Z" }),
    makeFeedItem({ id: 3, published_at: "2024-06-01T00:00:00Z" }),
  ]);
  const newItem = makeFeedItem({ id: 2, published_at: "2024-03-01T00:00:00Z" });
  const result = upsertByNewest(items, newItem);
  assertEquals(result.map((i) => i.id), [3, 2, 1]);
});

Deno.test("upsertByNewest - inserts item at the beginning when newest", () => {
  const items = sortByNewest([
    makeFeedItem({ id: 1, published_at: "2024-01-01T00:00:00Z" }),
    makeFeedItem({ id: 2, published_at: "2024-03-01T00:00:00Z" }),
  ]);
  const newItem = makeFeedItem({ id: 3, published_at: "2024-12-01T00:00:00Z" });
  const result = upsertByNewest(items, newItem);
  assertEquals(result.map((i) => i.id), [3, 2, 1]);
});

Deno.test("upsertByNewest - does not move future published_at above newer inserted item", () => {
  const items = sortByNewest([
    makeFeedItem({
      id: 2,
      published_at: "2024-01-02T00:00:00Z",
      inserted_at: "2024-01-02T00:00:00Z",
    }),
  ]);
  const newItem = makeFeedItem({
    id: 1,
    published_at: "2030-01-01T00:00:00Z",
    inserted_at: "2024-01-01T00:00:00Z",
  });
  const result = upsertByNewest(items, newItem);
  assertEquals(result.map((i) => i.id), [2, 1]);
});

Deno.test("upsertByNewest - inserts item at the end when oldest", () => {
  const items = sortByNewest([
    makeFeedItem({ id: 2, published_at: "2024-03-01T00:00:00Z" }),
    makeFeedItem({ id: 3, published_at: "2024-06-01T00:00:00Z" }),
  ]);
  const newItem = makeFeedItem({ id: 1, published_at: "2023-01-01T00:00:00Z" });
  const result = upsertByNewest(items, newItem);
  assertEquals(result.map((i) => i.id), [3, 2, 1]);
});

Deno.test("upsertByNewest - deduplicates existing item by id", () => {
  const items = sortByNewest([
    makeFeedItem({ id: 1, published_at: "2024-01-01T00:00:00Z", title: "Old" }),
    makeFeedItem({ id: 2, published_at: "2024-06-01T00:00:00Z" }),
  ]);
  const updatedItem = makeFeedItem({
    id: 1,
    published_at: "2024-01-01T00:00:00Z",
    title: "Updated",
  });
  const result = upsertByNewest(items, updatedItem);
  assertEquals(result.length, 2);
  assertEquals(result.find((i) => i.id === 1)!.title, "Updated");
});

Deno.test("upsertByNewest - inserts into empty list", () => {
  const newItem = makeFeedItem({ id: 1, published_at: "2024-06-01T00:00:00Z" });
  const result = upsertByNewest([], newItem);
  assertEquals(result.length, 1);
  assertEquals(result[0].id, 1);
});

Deno.test("upsertByNewest - caps result at MAX_TIMELINE_ITEMS", () => {
  // Create a list of MAX_TIMELINE_ITEMS items
  const items: FeedItem[] = [];
  for (let i = 0; i < MAX_TIMELINE_ITEMS; i++) {
    items.push(
      makeFeedItem({
        id: i + 1,
        published_at: new Date(2024, 0, 1, 0, 0, i).toISOString(),
      }),
    );
  }
  const sorted = sortByNewest(items);

  // Insert one more (newest) — should cap at MAX_TIMELINE_ITEMS
  const newItem = makeFeedItem({
    id: MAX_TIMELINE_ITEMS + 1,
    published_at: "2025-01-01T00:00:00Z",
  });
  const result = upsertByNewest(sorted, newItem);
  assertEquals(result.length, MAX_TIMELINE_ITEMS);
  assertEquals(result[0].id, MAX_TIMELINE_ITEMS + 1); // newest is first
});

Deno.test("upsertByNewest - handles same-date items using id tiebreaker", () => {
  const sameDate = "2024-06-01T00:00:00Z";
  const items = sortByNewest([
    makeFeedItem({ id: 1, published_at: sameDate }),
    makeFeedItem({ id: 5, published_at: sameDate }),
  ]);
  const newItem = makeFeedItem({ id: 3, published_at: sameDate });
  const result = upsertByNewest(items, newItem);
  assertEquals(result.map((i) => i.id), [5, 3, 1]);
});

// ---------------------------------------------------------------------------
// upsertManyByNewest
// ---------------------------------------------------------------------------

Deno.test("upsertManyByNewest - batches dedupe, sorting, and capping", () => {
  const items = sortByNewest([
    makeFeedItem({ id: 1, title: "Old", published_at: "2024-01-01T00:00:00Z" }),
    makeFeedItem({ id: 2, published_at: "2024-02-01T00:00:00Z" }),
  ]);

  const result = upsertManyByNewest(items, [
    makeFeedItem({
      id: 1,
      title: "Updated",
      published_at: "2024-01-01T00:00:00Z",
    }),
    makeFeedItem({ id: 3, published_at: "2024-03-01T00:00:00Z" }),
  ]);

  assertEquals(result.map((i) => i.id), [3, 2, 1]);
  assertEquals(result.find((i) => i.id === 1)!.title, "Updated");
});

Deno.test("upsertManyByNewest - caps batched replay at MAX_TIMELINE_ITEMS", () => {
  const replayedItems: FeedItem[] = [];
  for (let i = 0; i < MAX_TIMELINE_ITEMS + 10; i++) {
    replayedItems.push(
      makeFeedItem({
        id: i + 1,
        published_at: new Date(2024, 0, 1, 0, 0, i).toISOString(),
      }),
    );
  }

  const result = upsertManyByNewest([], replayedItems);

  assertEquals(result.length, MAX_TIMELINE_ITEMS);
  assertEquals(result[0].id, MAX_TIMELINE_ITEMS + 10);
});

// ---------------------------------------------------------------------------
// replay cursor helpers
// ---------------------------------------------------------------------------

Deno.test("chooseResumeCursor - prefers initial snapshot max id over stored cursor", () => {
  const items = [makeFeedItem({ id: 10 }), makeFeedItem({ id: 15 })];

  assertEquals(chooseResumeCursor(items, "2"), "15");
});

Deno.test("chooseResumeCursor - falls back to stored cursor when snapshot is empty", () => {
  assertEquals(chooseResumeCursor([], "42"), "42");
});

Deno.test("parseReplayDoneEvent - normalizes replay marker payload", () => {
  assertEquals(
    parseReplayDoneEvent(
      JSON.stringify({ replayed: 500, limited: true, last_event_id: 123 }),
    ),
    { replayed: 500, limited: true, last_event_id: 123 },
  );
});
