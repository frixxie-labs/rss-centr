import { Head } from "fresh/runtime";
import { define } from "../utils.ts";
import { AppNav } from "../components/AppNav.tsx";
import AnalyticsTracker from "../islands/AnalyticsTracker.tsx";
import { fetchFeeds, fetchLatestItems } from "../api.ts";
import { Header } from "../components/Header.tsx";
import Timeline, { MAX_TIMELINE_ITEMS } from "../islands/Timeline.tsx";
import type { FeedItem } from "../types.ts";
import { getLogger } from "../logger.ts";

const log = getLogger("ssr");

export const handler = define.handlers({
  async GET(_ctx) {
    let items: FeedItem[] = [];
    let feedNames: Record<number, string> = {};
    let loadError = false;
    const initialNowIso = new Date().toISOString();
    try {
      const [itemsResult, feeds] = await Promise.all([
        fetchLatestItems({ limit: MAX_TIMELINE_ITEMS }),
        fetchFeeds(),
      ]);
      items = itemsResult;
      feedNames = Object.fromEntries(
        feeds.map((f) => [f.id, f.title ?? f.url]),
      );
    } catch (err) {
      log.error("Failed to fetch data for SSR", err);
      loadError = true;
    }
    return { data: { items, feedNames, loadError, initialNowIso } };
  },
});

export default define.page<typeof handler>(function Home({ data }) {
  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr</title>
      </Head>
      <Header>
        <AppNav currentPath="/" />
      </Header>
      <main class="mx-auto w-full min-w-0 max-w-2xl flex-1">
        {data.loadError && (
          <div class="mx-4 my-4 rounded-md border border-ronin-yellow/50 bg-winter-yellow/50 px-3 py-2 text-sm text-ronin-yellow">
            Could not load the latest news. Showing available data and waiting
            for live updates.
          </div>
        )}
        <Timeline
          initialItems={data.items}
          feedNames={data.feedNames}
          initialNowIso={data.initialNowIso}
        />
      </main>
      <AnalyticsTracker path="/" />
    </div>
  );
});
