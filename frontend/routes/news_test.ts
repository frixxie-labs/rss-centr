import { assertEquals } from "@std/assert";
import { parseSourceId } from "./news.tsx";

Deno.test("parseSourceId - reads source_id", () => {
  const url = new URL("https://example.com/news?q=rust&source_id=42");

  assertEquals(parseSourceId(url), 42);
});

Deno.test("parseSourceId - falls back to feed_id", () => {
  const url = new URL("https://example.com/news?q=rust&feed_id=42");

  assertEquals(parseSourceId(url), 42);
});

Deno.test("parseSourceId - source_id takes precedence over feed_id", () => {
  const url = new URL(
    "https://example.com/news?q=rust&source_id=7&feed_id=42",
  );

  assertEquals(parseSourceId(url), 7);
});

Deno.test("parseSourceId - ignores invalid source ids", () => {
  const cases = [
    "https://example.com/news",
    "https://example.com/news?source_id=",
    "https://example.com/news?source_id=abc",
    "https://example.com/news?source_id=1.5",
    "https://example.com/news?source_id=0",
    "https://example.com/news?source_id=-1",
  ];

  for (const value of cases) {
    assertEquals(parseSourceId(new URL(value)), undefined);
  }
});
