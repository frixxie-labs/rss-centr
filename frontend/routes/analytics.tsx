import { Head } from "fresh/runtime";

import { fetchAnalyticsSummary } from "../api.ts";
import { Header } from "../components/Header.tsx";
import type { AnalyticsSummary } from "../types.ts";
import { define } from "../utils.ts";

function renderEventType(eventType: string): string {
  return eventType.replaceAll("_", " ");
}

export const handler = define.handlers({
  async GET(_ctx) {
    let summary: AnalyticsSummary = {
      enabled: false,
      window_days: 7,
      totals: {
        page_views: 0,
        unique_visitors: 0,
        unique_sessions: 0,
      },
      daily_page_views: [],
      top_pages: [],
      event_breakdown: [],
    };
    let loadError = false;

    try {
      summary = await fetchAnalyticsSummary();
    } catch {
      loadError = true;
    }

    return { data: { summary, loadError } };
  },
});

export default define.page<typeof handler>(function AnalyticsPage({ data }) {
  const { summary } = data;

  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr - Analytics</title>
      </Head>
      <Header>
        <a
          href="/"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
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
        <a
          href="/analytics"
          class="rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white"
        >
          Analytics
        </a>
      </Header>
      <main class="mx-auto w-full min-w-0 max-w-4xl flex-1 px-4 py-5">
        {data.loadError && (
          <div class="mb-4 rounded-md border border-ronin-yellow/50 bg-winter-yellow/50 px-3 py-2 text-sm text-ronin-yellow">
            Could not load analytics.
          </div>
        )}
        {!summary.enabled
          ? (
            <div class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4 text-sm text-fuji-gray">
              Analytics is disabled. Set <code>ANALYTICS_ENABLED=true</code> on
              the backend to collect visitor metrics.
            </div>
          )
          : (
            <div class="space-y-4">
              <section class="grid gap-3 sm:grid-cols-3">
                <div class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4">
                  <div class="text-xs uppercase tracking-wide text-katana-gray">
                    Last {summary.window_days} days
                  </div>
                  <div class="mt-2 text-3xl font-semibold text-fuji-white">
                    {summary.totals.page_views}
                  </div>
                  <div class="text-sm text-fuji-gray">Page views</div>
                </div>
                <div class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4">
                  <div class="text-xs uppercase tracking-wide text-katana-gray">
                    Last {summary.window_days} days
                  </div>
                  <div class="mt-2 text-3xl font-semibold text-fuji-white">
                    {summary.totals.unique_visitors}
                  </div>
                  <div class="text-sm text-fuji-gray">Unique visitors</div>
                </div>
                <div class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4">
                  <div class="text-xs uppercase tracking-wide text-katana-gray">
                    Last {summary.window_days} days
                  </div>
                  <div class="mt-2 text-3xl font-semibold text-fuji-white">
                    {summary.totals.unique_sessions}
                  </div>
                  <div class="text-sm text-fuji-gray">Unique sessions</div>
                </div>
              </section>

              <section class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60">
                <div class="border-b border-sumi-ink3 px-4 py-3 text-sm font-semibold text-fuji-white">
                  Daily page views
                </div>
                <div class="overflow-x-auto">
                  <table class="min-w-full text-left text-sm">
                    <thead class="text-katana-gray">
                      <tr>
                        <th class="px-4 py-2 font-medium">Date</th>
                        <th class="px-4 py-2 font-medium">Page views</th>
                        <th class="px-4 py-2 font-medium">Unique visitors</th>
                      </tr>
                    </thead>
                    <tbody>
                      {summary.daily_page_views.map((point) => (
                        <tr key={point.date} class="border-t border-sumi-ink3">
                          <td class="px-4 py-2 text-fuji-white">{point.date}</td>
                          <td class="px-4 py-2 text-fuji-gray">
                            {point.page_views}
                          </td>
                          <td class="px-4 py-2 text-fuji-gray">
                            {point.unique_visitors}
                          </td>
                        </tr>
                      ))}
                      {summary.daily_page_views.length === 0 && (
                        <tr>
                          <td
                            colSpan={3}
                            class="px-4 py-8 text-center text-fuji-gray"
                          >
                            No page views recorded yet.
                          </td>
                        </tr>
                      )}
                    </tbody>
                  </table>
                </div>
              </section>

              <section class="grid gap-4 lg:grid-cols-2">
                <div class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60">
                  <div class="border-b border-sumi-ink3 px-4 py-3 text-sm font-semibold text-fuji-white">
                    Top pages
                  </div>
                  <div class="overflow-x-auto">
                    <table class="min-w-full text-left text-sm">
                      <thead class="text-katana-gray">
                        <tr>
                          <th class="px-4 py-2 font-medium">Path</th>
                          <th class="px-4 py-2 font-medium">Views</th>
                          <th class="px-4 py-2 font-medium">Visitors</th>
                        </tr>
                      </thead>
                      <tbody>
                        {summary.top_pages.map((page) => (
                          <tr key={page.path} class="border-t border-sumi-ink3">
                            <td class="px-4 py-2 text-fuji-white">{page.path}</td>
                            <td class="px-4 py-2 text-fuji-gray">
                              {page.page_views}
                            </td>
                            <td class="px-4 py-2 text-fuji-gray">
                              {page.unique_visitors}
                            </td>
                          </tr>
                        ))}
                        {summary.top_pages.length === 0 && (
                          <tr>
                            <td
                              colSpan={3}
                              class="px-4 py-8 text-center text-fuji-gray"
                            >
                              No page views recorded yet.
                            </td>
                          </tr>
                        )}
                      </tbody>
                    </table>
                  </div>
                </div>

                <div class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60">
                  <div class="border-b border-sumi-ink3 px-4 py-3 text-sm font-semibold text-fuji-white">
                    Feature usage
                  </div>
                  <div class="overflow-x-auto">
                    <table class="min-w-full text-left text-sm">
                      <thead class="text-katana-gray">
                        <tr>
                          <th class="px-4 py-2 font-medium">Event</th>
                          <th class="px-4 py-2 font-medium">Count</th>
                          <th class="px-4 py-2 font-medium">Visitors</th>
                        </tr>
                      </thead>
                      <tbody>
                        {summary.event_breakdown.map((event) => (
                          <tr
                            key={event.event_type}
                            class="border-t border-sumi-ink3"
                          >
                            <td class="px-4 py-2 text-fuji-white capitalize">
                              {renderEventType(event.event_type)}
                            </td>
                            <td class="px-4 py-2 text-fuji-gray">
                              {event.count}
                            </td>
                            <td class="px-4 py-2 text-fuji-gray">
                              {event.unique_visitors}
                            </td>
                          </tr>
                        ))}
                        {summary.event_breakdown.length === 0 && (
                          <tr>
                            <td
                              colSpan={3}
                              class="px-4 py-8 text-center text-fuji-gray"
                            >
                              No analytics events recorded yet.
                            </td>
                          </tr>
                        )}
                      </tbody>
                    </table>
                  </div>
                </div>
              </section>
            </div>
          )}
      </main>
    </div>
  );
});
