import { Head } from "fresh/runtime";
import { fetchFeeds, fetchLatestItems } from "../api.ts";
import { Header } from "../components/Header.tsx";
import FeedItemsView from "../islands/FeedItemsView.tsx";
import { getLogger } from "../logger.ts";
import { parseSourceId } from "../newsFilters.ts";
import type { FeedItem } from "../types.ts";
import { define } from "../utils.ts";
import { AppNav } from "../components/AppNav.tsx";
import AnalyticsTracker from "../islands/AnalyticsTracker.tsx";

const log = getLogger("ssr");
const ITEMS_LIMIT = 500;

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
        <AppNav currentPath="/news" />
      </Header>
      <main class="mx-auto w-full min-w-0 max-w-3xl flex-1">
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
      <AnalyticsTracker path="/news" />
    </div>
  );
});
