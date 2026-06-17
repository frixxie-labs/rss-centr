import { assertEquals } from "@std/assert";
import { newsUrlForTopicSource } from "./WordCloud.tsx";

Deno.test("newsUrlForTopicSource - links to news with topic and source filters", () => {
  assertEquals(newsUrlForTopicSource("rust", 42), "/news?q=rust&source_id=42");
});

Deno.test("newsUrlForTopicSource - URL encodes topic words", () => {
  assertEquals(
    newsUrlForTopicSource("ai safety", 7),
    "/news?q=ai+safety&source_id=7",
  );
});
