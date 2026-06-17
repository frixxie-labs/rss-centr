import { Head } from "fresh/runtime";
import { fetchFeeds, fetchLatestItems } from "../api.ts";
import { Header } from "../components/Header.tsx";
import FeedItemsView from "../islands/FeedItemsView.tsx";
import { getLogger } from "../logger.ts";
import type { FeedItem } from "../types.ts";
import { define } from "../utils.ts";

const log = getLogger("ssr");
const ITEMS_LIMIT = 500;

export function parseSourceId(url: URL): number | undefined {
  const value = url.searchParams.get("source_id") ??
    url.searchParams.get("feed_id");
  if (!value) {
    return undefined;
  }

  const id = Number(value);
  return Number.isInteger(id) && id > 0 ? id : undefined;
}

export const handler = define.handlers({
  async GET(ctx) {
    const url = new URL(ctx.req.url);
    const initialQuery = url.searchParams.get("q")?.trim() ?? "";
    const initialSourceId = parseSourceId(url);
    let items: FeedItem[] = [];
    let feedNames: Record<number, string> = {};
    let loadError = false;
    const initialNowIso = new Date().toISOString();

    try {
      const [itemsResult, feeds] = await Promise.all([
        fetchLatestItems({
          limit: ITEMS_LIMIT,
          feedId: initialSourceId,
          query: initialQuery || undefined,
        }),
        fetchFeeds(),
      ]);
      items = itemsResult;
      feedNames = Object.fromEntries(
        feeds.map((f) => [f.id, f.title ?? f.url]),
      );
    } catch (err) {
      log.error("Failed to fetch news for SSR", err);
      loadError = true;
    }

    return {
      data: {
        items,
        feedNames,
        loadError,
        initialNowIso,
        initialQuery,
        initialSourceId,
      },
    };
  },
});

export default define.page<typeof handler>(function NewsPage({ data }) {
  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr - All News</title>
      </Head>
      <Header>
        <a
          href="/"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Timeline
        </a>
        <a
          href="/news"
          class="rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white"
        >
          News
        </a>
        <a
          href="/sources"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Sources
        </a>
        <a
          href="/topics"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Topics
        </a>
      </Header>
      <main class="mx-auto w-full max-w-3xl flex-1">
        {data.loadError && (
          <div class="mx-4 my-4 rounded-md border border-ronin-yellow/50 bg-winter-yellow/50 px-3 py-2 text-sm text-ronin-yellow">
            Could not load news.
          </div>
        )}
        <FeedItemsView
          initialItems={data.items}
          feedNames={data.feedNames}
          initialNowIso={data.initialNowIso}
          initialQuery={data.initialQuery}
          initialFeedId={data.initialSourceId}
          limit={ITEMS_LIMIT}
        />
      </main>
    </div>
  );
});
