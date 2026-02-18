import { Head } from "fresh/runtime";
import { define } from "../utils.ts";
import { fetchFeeds, fetchLatestItems } from "../api.ts";
import { Header } from "../components/Header.tsx";
import Timeline from "../islands/Timeline.tsx";
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
        fetchLatestItems(100),
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
      <Header />
      <main class="flex-1 max-w-2xl mx-auto w-full">
        {data.loadError && (
          <div class="mx-4 my-4 rounded-md border border-amber-700/50 bg-amber-950/50 px-3 py-2 text-sm text-amber-200">
            Could not load the latest items from the backend. Showing available
            data and waiting for live updates.
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
