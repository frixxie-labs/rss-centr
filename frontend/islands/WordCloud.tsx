import { useSignal } from "@preact/signals";
import type { ScoredFeedTitleIndexEntry } from "../types.ts";
import { fetchRecentScoredIndex } from "../api.ts";

interface WordCloudProps {
  initialEntries: ScoredFeedTitleIndexEntry[];
  feedNames: Record<number, string>;
}

/** Map a value from [inMin, inMax] to [outMin, outMax]. */
function scale(
  value: number,
  inMin: number,
  inMax: number,
  outMin: number,
  outMax: number,
): number {
  if (inMax === inMin) return (outMin + outMax) / 2;
  return outMin + ((value - inMin) / (inMax - inMin)) * (outMax - outMin);
}

/** Pick a color class based on the highest TF-IDF score for a word. */
function colorForScore(topTfIdf: number, maxTfIdf: number): string {
  if (maxTfIdf === 0) return "text-fuji-gray";
  const ratio = topTfIdf / maxTfIdf;
  if (ratio > 0.7) return "text-carp-yellow";
  if (ratio > 0.4) return "text-spring-blue";
  if (ratio > 0.2) return "text-spring-green";
  return "text-old-white";
}

export default function WordCloud(
  { initialEntries, feedNames }: WordCloudProps,
) {
  const entries = useSignal<ScoredFeedTitleIndexEntry[]>(initialEntries);
  const selected = useSignal<ScoredFeedTitleIndexEntry | null>(null);
  const loading = useSignal(false);
  const error = useSignal<string | null>(null);

  async function refresh() {
    loading.value = true;
    error.value = null;
    try {
      entries.value = await fetchRecentScoredIndex();
      selected.value = null;
    } catch (err) {
      error.value = err instanceof Error ? err.message : String(err);
    } finally {
      loading.value = false;
    }
  }

  const data = entries.value;
  if (data.length === 0) {
    return (
      <div class="px-4 py-12 text-center text-sumi-ink4">
        No title index data available for the last 24 hours.
        <button
          type="button"
          onClick={refresh}
          disabled={loading.value}
          class="ml-2 text-spring-blue underline hover:text-crystal-blue"
        >
          {loading.value ? "Loading..." : "Refresh"}
        </button>
      </div>
    );
  }

  const maxOccurrences = Math.max(...data.map((e) => e.total_occurences));
  const minOccurrences = Math.min(...data.map((e) => e.total_occurences));
  const maxTfIdf = Math.max(
    ...data.flatMap((e) => e.items.map((i) => i.tf_idf)),
  );

  const minFontSize = 0.75;
  const maxFontSize = 3;

  return (
    <div>
      {/* Header bar */}
      <div class="px-4 py-2 border-b border-sumi-ink3 flex items-center justify-between text-xs text-fuji-gray">
        <span>{data.length} words indexed in the last 24 hours</span>
        <button
          type="button"
          onClick={refresh}
          disabled={loading.value}
          class="text-spring-blue hover:text-crystal-blue transition disabled:opacity-50"
        >
          {loading.value ? "Loading..." : "Refresh"}
        </button>
      </div>

      {error.value && (
        <div class="mx-4 my-2 rounded-md border border-autumn-red/70 bg-winter-red/40 px-3 py-2 text-sm text-wave-red">
          {error.value}
        </div>
      )}

      {/* Word cloud */}
      <div class="px-4 py-6 flex flex-wrap items-baseline justify-center gap-x-3 gap-y-1">
        {data.map((entry) => {
          const fontSize = scale(
            entry.total_occurences,
            minOccurrences,
            maxOccurrences,
            minFontSize,
            maxFontSize,
          );
          const topTfIdf = entry.items.length > 0 ? entry.items[0].tf_idf : 0;
          const color = colorForScore(topTfIdf, maxTfIdf);
          const isSelected = selected.value?.word === entry.word;

          return (
            <button
              key={entry.word}
              type="button"
              onClick={() => {
                selected.value = isSelected ? null : entry;
              }}
              class={`cursor-pointer transition-all duration-150 hover:opacity-80 ${color} ${
                isSelected
                  ? "underline decoration-carp-yellow underline-offset-4"
                  : ""
              }`}
              style={{ fontSize: `${fontSize}rem`, lineHeight: 1.3 }}
              title={`${entry.word}: ${entry.total_occurences} occurrences`}
            >
              {entry.word}
            </button>
          );
        })}
      </div>

      {/* Legend */}
      <div class="px-4 pb-2 flex items-center justify-center gap-4 text-xs text-fuji-gray">
        <span class="flex items-center gap-1">
          <span class="inline-block w-2 h-2 rounded-full bg-carp-yellow" />
          high TF-IDF
        </span>
        <span class="flex items-center gap-1">
          <span class="inline-block w-2 h-2 rounded-full bg-spring-blue" />
          medium
        </span>
        <span class="flex items-center gap-1">
          <span class="inline-block w-2 h-2 rounded-full bg-spring-green" />
          low
        </span>
        <span class="flex items-center gap-1">
          <span class="inline-block w-2 h-2 rounded-full bg-old-white" />
          common
        </span>
      </div>

      {/* Detail panel */}
      {selected.value && (
        <div class="mx-4 mb-4 rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4">
          <div class="flex items-baseline justify-between mb-3">
            <h3 class="text-lg font-semibold text-fuji-white">
              {selected.value.word}
            </h3>
            <span class="text-sm text-fuji-gray">
              {selected.value.total_occurences} total occurrences across{" "}
              {selected.value.items.length} feed
              {selected.value.items.length !== 1 ? "s" : ""}
            </span>
          </div>

          <div class="space-y-2">
            {selected.value.items.map((item) => {
              const barWidth = maxTfIdf > 0
                ? (item.tf_idf / maxTfIdf) * 100
                : 0;
              const feedName = feedNames[item.feed_src_id] ??
                `Feed #${item.feed_src_id}`;

              return (
                <div key={item.feed_src_id} class="space-y-1">
                  <div class="flex items-center justify-between text-sm">
                    <span class="text-old-white truncate max-w-[60%]">
                      {feedName}
                    </span>
                    <span class="text-fuji-gray font-mono text-xs">
                      {item.occurences}x &middot; TF-IDF{" "}
                      {item.tf_idf.toFixed(3)}
                    </span>
                  </div>
                  <div class="h-1.5 w-full rounded-full bg-sumi-ink3">
                    <div
                      class="h-1.5 rounded-full bg-carp-yellow transition-all duration-300"
                      style={{ width: `${Math.max(barWidth, 2)}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
