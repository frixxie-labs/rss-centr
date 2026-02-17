import { BACKEND_URL } from "./utils.ts";
import type { FeedItem, FeedSubscription } from "./types.ts";

export async function fetchLatestItems(limit = 100): Promise<FeedItem[]> {
  const res = await fetch(`${BACKEND_URL}/api/items/latest?limit=${limit}`);
  if (!res.ok) {
    throw new Error(`Failed to fetch latest items: ${res.status}`);
  }
  return await res.json();
}

export async function fetchFeeds(): Promise<FeedSubscription[]> {
  const res = await fetch(`${BACKEND_URL}/api/feeds`);
  if (!res.ok) {
    throw new Error(`Failed to fetch feeds: ${res.status}`);
  }
  return await res.json();
}
