import { assertEquals } from "@std/assert";
import {
  DEFAULT_SEARCH_MODE,
  displayOrder,
  isRankedSearch,
  parseSearchMode,
} from "./newsFilters.ts";
import type { FeedItem } from "./types.ts";

function item(id: number, publishedAt: string): FeedItem {
  return {
    id,
    feed_id: 1,
    external_id: String(id),
    title: `Item ${id}`,
    url: `https://example.com/${id}`,
    inserted_at: "2026-10-16T12:00:00Z",
    published_at: publishedAt,
  } as FeedItem;
}

Deno.test("parseSearchMode - defaults to hybrid", () => {
  assertEquals(DEFAULT_SEARCH_MODE, "hybrid");
  assertEquals(
    parseSearchMode(new URL("https://example.com/news?q=rust")),
    "hybrid",
  );
});

Deno.test("parseSearchMode - reads valid modes", () => {
  for (const mode of ["keyword", "semantic", "hybrid"] as const) {
    const url = new URL(`https://example.com/news?mode=${mode}`);
    assertEquals(parseSearchMode(url), mode);
  }
});

Deno.test("parseSearchMode - ignores unknown modes", () => {
  const url = new URL("https://example.com/news?mode=fuzzy");
  assertEquals(parseSearchMode(url), "hybrid");
});

Deno.test("isRankedSearch - only non-keyword modes with a query", () => {
  assertEquals(isRankedSearch("hybrid", "rust"), true);
  assertEquals(isRankedSearch("semantic", "rust"), true);
  assertEquals(isRankedSearch("keyword", "rust"), false);
  assertEquals(isRankedSearch("hybrid", "   "), false);
  assertEquals(isRankedSearch("semantic", ""), false);
});

Deno.test("displayOrder - keeps server relevance order for ranked searches", () => {
  const older = item(1, "2026-10-01T00:00:00Z");
  const newer = item(2, "2026-10-15T00:00:00Z");

  assertEquals(
    displayOrder([older, newer], "hybrid", "rust").map((i) => i.id),
    [1, 2],
  );
  assertEquals(
    displayOrder([older, newer], "keyword", "rust").map((i) => i.id),
    [2, 1],
  );
  assertEquals(
    displayOrder([older, newer], "hybrid", "").map((i) => i.id),
    [2, 1],
  );
});
