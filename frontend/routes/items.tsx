import { Head } from "fresh/runtime";
import { fetchFeeds, fetchLatestItems } from "../api.ts";
import { Header } from "../components/Header.tsx";
import FeedItemsView from "../islands/FeedItemsView.tsx";
import { getLogger } from "../logger.ts";
import type { FeedItem } from "../types.ts";
import { define } from "../utils.ts";

const log = getLogger("ssr");

export const handler = define.handlers({
  async GET(_ctx) {
    let items: FeedItem[] = [];
    let feedNames: Record<number, string> = {};
    let loadError = false;
    const initialNowIso = new Date().toISOString();

    try {
      const [itemsResult, feeds] = await Promise.all([
        fetchLatestItems(),
        fetchFeeds(),
      ]);
      items = itemsResult;
      feedNames = Object.fromEntries(
        feeds.map((f) => [f.id, f.title ?? f.url]),
      );
    } catch (err) {
      log.error("Failed to fetch items for SSR", err);
      loadError = true;
    }

    return { data: { items, feedNames, loadError, initialNowIso } };
  },
});

export default define.page<typeof handler>(function ItemsPage({ data }) {
  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr - All Items</title>
      </Head>
      <Header>
        <a
          href="/"
          class="rounded-md px-2 py-1 text-sm text-neutral-400 transition hover:bg-neutral-800 hover:text-neutral-100"
        >
          Timeline
        </a>
        <a
          href="/items"
          class="rounded-md bg-neutral-800 px-2 py-1 text-sm text-neutral-100"
        >
          Items
        </a>
        <a
          href="/feeds"
          class="rounded-md px-2 py-1 text-sm text-neutral-400 transition hover:bg-neutral-800 hover:text-neutral-100"
        >
          Feeds
        </a>
      </Header>
      <main class="mx-auto w-full max-w-3xl flex-1">
        {data.loadError && (
          <div class="mx-4 my-4 rounded-md border border-amber-700/50 bg-amber-950/50 px-3 py-2 text-sm text-amber-200">
            Could not load items from the backend.
          </div>
        )}
        <FeedItemsView
          initialItems={data.items}
          feedNames={data.feedNames}
          initialNowIso={data.initialNowIso}
        />
      </main>
    </div>
  );
});
