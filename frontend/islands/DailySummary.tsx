import { useSignal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import { fetchDailySummary } from "../api.ts";
import type { DailySummary as DailySummaryData } from "../types.ts";

export default function DailySummary() {
  const data = useSignal<DailySummaryData | null>(null);
  const error = useSignal(false);
  const loading = useSignal(true);

  useEffect(() => {
    const controller = new AbortController();
    fetchDailySummary(controller.signal)
      .then((result) => {
        data.value = result;
        loading.value = false;
      })
      .catch(() => {
        if (!controller.signal.aborted) {
          error.value = true;
          loading.value = false;
        }
      });
    return () => controller.abort();
  }, []);

  return (
    <section
      aria-labelledby="daily-summary-title"
      class="mx-4 my-4 rounded-md border border-sumi-ink3 bg-sumi-ink1 p-4"
    >
      <h2
        id="daily-summary-title"
        class="text-sm font-semibold text-fuji-white"
      >
        Last 24 hours
      </h2>
      {loading.value && (
        <p role="status" class="mt-2 text-sm text-fuji-gray">
          Loading latest overview…
        </p>
      )}
      {error.value && (
        <p role="alert" class="mt-2 text-sm text-ronin-yellow">
          Could not load the overview.
        </p>
      )}
      {!loading.value && !error.value && !data.value && (
        <p role="status" class="mt-2 text-sm text-fuji-gray">
          No summary is available yet. The next scheduled generation will appear
          here.
        </p>
      )}
      {data.value && (
        <>
          <p class="mt-2 whitespace-pre-line text-sm leading-relaxed text-fuji-gray">
            {data.value.summary}
          </p>
          <p class="mt-3 text-xs text-katana-gray">
            Generated {new Date(data.value.generated_at).toLocaleString()} using
            {" "}
            {data.value.model} · {data.value.feed_ids.length} sources
          </p>
        </>
      )}
    </section>
  );
}
