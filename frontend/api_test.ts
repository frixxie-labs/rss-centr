import { assertEquals } from "@std/assert";
import { latestItemsSearchParams } from "./api.ts";

Deno.test("latestItemsSearchParams - empty options produce no params", () => {
  assertEquals(latestItemsSearchParams({}).toString(), "");
});

Deno.test("latestItemsSearchParams - serializes query and mode", () => {
  const params = latestItemsSearchParams({
    limit: 50,
    feedId: 3,
    query: "  electric cars ",
    mode: "semantic",
  });
  assertEquals(
    params.toString(),
    "limit=50&feed_id=3&q=electric+cars&mode=semantic",
  );
});

Deno.test("latestItemsSearchParams - omits mode without a query", () => {
  const params = latestItemsSearchParams({ query: "  ", mode: "hybrid" });
  assertEquals(params.toString(), "");
});
