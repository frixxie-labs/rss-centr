import { Head } from "fresh/runtime";
import { define } from "../utils.ts";
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
        <a
          href="/"
          class="rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white"
        >
          Timeline
        </a>
        <a
          href="/news"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
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
      <main class="flex-1 max-w-2xl mx-auto w-full">
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
    </div>
  );
});
