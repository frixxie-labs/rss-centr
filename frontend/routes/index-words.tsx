import { Head } from "fresh/runtime";
import { fetchFeeds, fetchRecentScoredIndex } from "../api.ts";
import { Header } from "../components/Header.tsx";
import WordCloud from "../islands/WordCloud.tsx";
import { getLogger } from "../logger.ts";
import type { ScoredFeedTitleIndexEntry } from "../types.ts";
import { define } from "../utils.ts";

const log = getLogger("ssr");

export const handler = define.handlers({
  async GET(_ctx) {
    let entries: ScoredFeedTitleIndexEntry[] = [];
    let feedNames: Record<number, string> = {};
    let loadError = false;

    try {
      const [indexResult, feeds] = await Promise.all([
        fetchRecentScoredIndex(),
        fetchFeeds(),
      ]);
      entries = indexResult;
      feedNames = Object.fromEntries(
        feeds.map((f) => [f.id, f.title ?? f.url]),
      );
    } catch (err) {
      log.error("Failed to fetch title index for SSR", err);
      loadError = true;
    }

    return { data: { entries, feedNames, loadError } };
  },
});

export default define.page<typeof handler>(function TitleIndexPage({ data }) {
  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr - Title Index</title>
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
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Feeds
        </a>
        <a
          href="/index-words"
          class="rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white"
        >
          Word Cloud
        </a>
      </Header>
      <main class="mx-auto w-full max-w-3xl flex-1">
        {data.loadError && (
          <div class="mx-4 my-4 rounded-md border border-ronin-yellow/50 bg-winter-yellow/50 px-3 py-2 text-sm text-ronin-yellow">
            Could not load title index from the backend.
          </div>
        )}
        <WordCloud
          initialEntries={data.entries}
          feedNames={data.feedNames}
        />
      </main>
    </div>
  );
});
