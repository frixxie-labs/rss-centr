import { sortByNewest } from "./feedItemOrdering.ts";
import type { FeedItem } from "./types.ts";

export function parseSourceId(url: URL): number | undefined {
  const value = url.searchParams.get("source_id") ??
    url.searchParams.get("feed_id");
  if (!value) {
    return undefined;
  }

  const id = Number(value);
  return Number.isInteger(id) && id > 0 ? id : undefined;
}

export type SearchMode = "keyword" | "semantic" | "hybrid";

export const SEARCH_MODES: readonly SearchMode[] = [
  "hybrid",
  "semantic",
  "keyword",
];

export const DEFAULT_SEARCH_MODE: SearchMode = "hybrid";

export function parseSearchMode(url: URL): SearchMode {
  const value = url.searchParams.get("mode");
  return SEARCH_MODES.find((mode) => mode === value) ?? DEFAULT_SEARCH_MODE;
}

/** Ranked searches are ordered by relevance on the server, not by date. */
export function isRankedSearch(mode: SearchMode, query: string): boolean {
  return mode !== "keyword" && query.trim() !== "";
}

export function displayOrder(
  items: FeedItem[],
  mode: SearchMode,
  query: string,
): FeedItem[] {
  return isRankedSearch(mode, query) ? items : sortByNewest(items);
}
