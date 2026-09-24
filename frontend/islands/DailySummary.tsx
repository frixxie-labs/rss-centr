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
          Generating overview…
        </p>
      )}
      {error.value && (
        <p role="alert" class="mt-2 text-sm text-ronin-yellow">
          Could not load the overview.
        </p>
      )}
      {data.value && (
        <>
          <p class="mt-2 whitespace-pre-line text-sm leading-relaxed text-fuji-gray">
            {data.value.summary}
          </p>
          {data.value.articles_sampled > 0 && (
            <p class="mt-3 text-xs text-katana-gray">
              AI overview based on {data.value.articles_sampled}{" "}
              articles collected in the last 24 hours.
            </p>
          )}
        </>
      )}
    </section>
  );
}
