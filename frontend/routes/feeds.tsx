import { Head } from "fresh/runtime";
import { fetchFeeds } from "../api.ts";
import { Header } from "../components/Header.tsx";
import FeedManagement from "../islands/FeedManagement.tsx";
import { getLogger } from "../logger.ts";
import type { FeedSubscription } from "../types.ts";
import { define } from "../utils.ts";

const log = getLogger("ssr");

export const handler = define.handlers({
  async GET(_ctx) {
    let feeds: FeedSubscription[] = [];
    let loadError = false;

    try {
      feeds = await fetchFeeds();
    } catch (err) {
      log.error("Failed to fetch feeds for SSR", err);
      loadError = true;
    }

    return { data: { feeds, loadError } };
  },
});

export default define.page<typeof handler>(function FeedsPage({ data }) {
  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr - Feed Management</title>
      </Head>
      <Header>
        <a
          href="/"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Timeline
        </a>
        <a
          href="/items"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Items
        </a>
        <a
          href="/feeds"
          class="rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white"
        >
          Feeds
        </a>
        <a
          href="/index-words"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Word Cloud
        </a>
      </Header>
      <main class="mx-auto w-full max-w-3xl flex-1">
        <FeedManagement
          initialFeeds={data.feeds}
          initialLoadError={data.loadError}
        />
      </main>
    </div>
  );
});
