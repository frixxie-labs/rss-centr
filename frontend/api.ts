import { BACKEND_URL } from "./utils.ts";
import type {
  FeedItem,
  FeedItemDetail,
  FeedSubscription,
  FeedTitleIndexEntry,
} from "./types.ts";

interface BackendFeedTitleIndexItem {
  feed_src_id: number;
  occurences: number;
}

interface BackendFeedTitleIndexEntry {
  word: string;
  total_occurences: number;
  items: BackendFeedTitleIndexItem[];
}

function apiUrl(path: string): string {
  if (typeof window === "undefined") {
    return `${BACKEND_URL}/api/${path}`;
  }
  return `/api/${path}`;
}

export interface LatestItemsOptions {
  limit?: number;
  feedId?: number;
  query?: string;
  signal?: AbortSignal;
}

async function throwRequestError(
  prefix: string,
  res: Response,
): Promise<never> {
  const body = await res.text();
  const suffix = body ? ` (${body})` : "";
  throw new Error(`${prefix}: ${res.status}${suffix}`);
}

export async function fetchLatestItems(
  options: LatestItemsOptions = {},
): Promise<FeedItem[]> {
  const params = new URLSearchParams();
  if (options.limit !== undefined) {
    params.set("limit", String(options.limit));
  }
  if (options.feedId !== undefined) {
    params.set("feed_id", String(options.feedId));
  }
  const query = options.query?.trim();
  if (query) {
    params.set("q", query);
  }

  const suffix = params.size > 0 ? `?${params}` : "";
  const res = await fetch(`${apiUrl("items/latest")}${suffix}`, {
    signal: options.signal,
  });
  if (!res.ok) {
    await throwRequestError("Failed to fetch latest news", res);
  }
  return await res.json();
}

export async function fetchItemWithDetail(itemId: number): Promise<FeedItem> {
  const [itemRes, detailRes] = await Promise.all([
    fetch(apiUrl(`items/${itemId}`)),
    fetch(apiUrl(`items/${itemId}/detail`)),
  ]);

  if (!itemRes.ok) {
    await throwRequestError(`Failed to fetch news item ${itemId}`, itemRes);
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
    await throwRequestError("Failed to fetch sources", res);
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
    await throwRequestError("Failed to create source", res);
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
    await throwRequestError("Failed to update source", retry);
  }

  await throwRequestError("Failed to update source", res);
}

export async function queueFeedIngest(feedId: number): Promise<void> {
  const res = await fetch(apiUrl(`feeds/${feedId}/ingest`), {
    method: "POST",
  });
  if (!res.ok) {
    await throwRequestError("Failed to queue fetch", res);
  }
}

export async function deleteFeed(feedId: number): Promise<void> {
  const res = await fetch(apiUrl(`feeds/${feedId}`), {
    method: "DELETE",
  });
  if (!res.ok) {
    await throwRequestError("Failed to delete source", res);
  }
}

export async function fetchRecentIndex(): Promise<FeedTitleIndexEntry[]> {
  const res = await fetch(apiUrl("feeds/index/today"));
  if (!res.ok) {
    await throwRequestError("Failed to fetch topics", res);
  }
  const entries = await res.json() as BackendFeedTitleIndexEntry[];
  return entries.map((entry) => ({
    word: entry.word,
    total_occurrences: entry.total_occurences,
    items: entry.items.map((item) => ({
      feed_src_id: item.feed_src_id,
      occurrences: item.occurences,
    })),
  }));
}
