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
    }
    return { data: { items, feedNames } };
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
        <Timeline initialItems={data.items} feedNames={data.feedNames} />
      </main>
    </div>
  );
});
