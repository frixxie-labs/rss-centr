import { Head } from "fresh/runtime";
import { fetchFeeds } from "../api.ts";
import { Header } from "../components/Header.tsx";
import FeedManagement from "../islands/FeedManagement.tsx";
import { getLogger } from "../logger.ts";
import type { FeedSubscription } from "../types.ts";
import { define } from "../utils.ts";
import { AppNav } from "../components/AppNav.tsx";
import AnalyticsTracker from "../islands/AnalyticsTracker.tsx";

const log = getLogger("ssr");

export const handler = define.handlers({
  async GET(_ctx) {
    let feeds: FeedSubscription[] = [];
    let loadError = false;

    try {
      feeds = await fetchFeeds();
    } catch (err) {
      log.error("Failed to fetch sources for SSR", err);
      loadError = true;
    }

    return { data: { feeds, loadError } };
  },
});

export default define.page<typeof handler>(function SourcesPage({ data }) {
  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr - Sources</title>
      </Head>
      <Header>
        <AppNav currentPath="/sources" />
      </Header>
      <main class="mx-auto w-full min-w-0 max-w-3xl flex-1">
        <FeedManagement
          initialFeeds={data.feeds}
          initialLoadError={data.loadError}
        />
      </main>
      <AnalyticsTracker path="/sources" />
    </div>
  );
});
