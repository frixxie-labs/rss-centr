import type { FeedItem } from "./types.ts";

export function effectiveDate(item: FeedItem): number {
  const insertedAt = new Date(item.inserted_at).getTime();
  if (!item.published_at) {
    return insertedAt;
  }

  const publishedAt = new Date(item.published_at).getTime();
  return Math.min(publishedAt, insertedAt);
}

export function sortByNewest(items: FeedItem[]): FeedItem[] {
  return [...items].sort((a, b) => {
    const dateDiff = effectiveDate(b) - effectiveDate(a);
    return dateDiff !== 0 ? dateDiff : b.id - a.id;
  });
}
