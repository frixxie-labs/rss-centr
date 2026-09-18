import { Head } from "fresh/runtime";
import { fetchFeeds, fetchRecentIndex } from "../api.ts";
import { Header } from "../components/Header.tsx";
import WordCloud from "../islands/WordCloud.tsx";
import { getLogger } from "../logger.ts";
import type { FeedTitleIndexEntry } from "../types.ts";
import { define } from "../utils.ts";
import { AppNav } from "../components/AppNav.tsx";
import AnalyticsTracker from "../islands/AnalyticsTracker.tsx";

const log = getLogger("ssr");

export const handler = define.handlers({
  async GET(_ctx) {
    let entries: FeedTitleIndexEntry[] = [];
    let feedNames: Record<number, string> = {};
    let loadError = false;

    try {
      const [indexResult, feeds] = await Promise.all([
        fetchRecentIndex(),
        fetchFeeds(),
      ]);
      entries = indexResult;
      feedNames = Object.fromEntries(
        feeds.map((f) => [f.id, f.title ?? f.url]),
      );
    } catch (err) {
      log.error("Failed to fetch topics for SSR", err);
      loadError = true;
    }

    return { data: { entries, feedNames, loadError } };
  },
});

export default define.page<typeof handler>(function TopicsPage({ data }) {
  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr - Topics</title>
      </Head>
      <Header>
        <AppNav currentPath="/topics" />
      </Header>
      <main class="mx-auto w-full min-w-0 max-w-3xl flex-1">
        {data.loadError && (
          <div class="mx-4 my-4 rounded-md border border-ronin-yellow/50 bg-winter-yellow/50 px-3 py-2 text-sm text-ronin-yellow">
            Could not load topics.
          </div>
        )}
        <WordCloud
          initialEntries={data.entries}
          feedNames={data.feedNames}
        />
      </main>
      <AnalyticsTracker path="/topics" />
    </div>
  );
});
