import { BACKEND_URL } from "./utils.ts";
import type { FeedItem, FeedItemDetail, FeedSubscription } from "./types.ts";

function apiUrl(path: string): string {
  if (typeof window === "undefined") {
    return `${BACKEND_URL}/api/${path}`;
  }
  return `/api/${path}`;
}

async function throwRequestError(
  prefix: string,
  res: Response,
): Promise<never> {
  const body = await res.text();
  const suffix = body ? ` (${body})` : "";
  throw new Error(`${prefix}: ${res.status}${suffix}`);
}

export async function fetchLatestItems(limit?: number): Promise<FeedItem[]> {
  const url = limit === undefined
    ? apiUrl("items/latest")
    : `${apiUrl("items/latest")}?limit=${limit}`;
  const res = await fetch(url);
  if (!res.ok) {
    await throwRequestError("Failed to fetch latest items", res);
  }
  return await res.json();
}

export async function fetchItemWithDetail(itemId: number): Promise<FeedItem> {
  const [itemRes, detailRes] = await Promise.all([
    fetch(apiUrl(`items/${itemId}`)),
    fetch(apiUrl(`items/${itemId}/detail`)),
  ]);

  if (!itemRes.ok) {
    await throwRequestError(`Failed to fetch item ${itemId}`, itemRes);
  }

  const item = await itemRes.json() as FeedItem;

  if (!detailRes.ok) {
    return item;
  }

  const detail = await detailRes.json() as FeedItemDetail;

  return {
    ...item,
    summary: detail.summary,
    content: detail.content,
    author: detail.author,
    published_at: detail.published_at,
  };
}

export async function fetchFeeds(): Promise<FeedSubscription[]> {
  const res = await fetch(apiUrl("feeds"));
  if (!res.ok) {
    await throwRequestError("Failed to fetch feeds", res);
  }
  return await res.json();
}

export async function createFeed(url: string): Promise<FeedSubscription> {
  const res = await fetch(apiUrl("feeds"), {
    method: "POST",
    headers: {
      "content-type": "application/json",
    },
    body: JSON.stringify({ url }),
  });
  if (!res.ok) {
    await throwRequestError("Failed to create feed", res);
  }
  return await res.json();
}

export async function updateFeedEnabled(
  feedId: number,
  isEnabled: boolean,
): Promise<void> {
  const url = apiUrl(`feeds/${feedId}`);

  const res = await fetch(url, {
    method: "PUT",
    headers: {
      "content-type": "application/json",
    },
    body: JSON.stringify({ is_enabled: isEnabled }),
  });

  if (res.ok) {
    return;
  }

  if (res.status === 400) {
    const retry = await fetch(url, {
      method: "PUT",
      headers: {
        "content-type": "application/json",
      },
      body: JSON.stringify({ isEnabled }),
    });
    if (retry.ok) {
      return;
    }
    await throwRequestError("Failed to update feed", retry);
  }

  await throwRequestError("Failed to update feed", res);
}

export async function queueFeedIngest(feedId: number): Promise<void> {
  const res = await fetch(apiUrl(`feeds/${feedId}/ingest`), {
    method: "POST",
  });
  if (!res.ok) {
    await throwRequestError("Failed to queue ingest", res);
  }
}

export async function deleteFeed(feedId: number): Promise<void> {
  const res = await fetch(apiUrl(`feeds/${feedId}`), {
    method: "DELETE",
  });
  if (!res.ok) {
    await throwRequestError("Failed to delete feed", res);
  }
}
