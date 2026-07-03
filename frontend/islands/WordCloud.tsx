import { useSignal } from "@preact/signals";
import type { FeedTitleIndexEntry } from "../types.ts";
import { fetchRecentIndex } from "../api.ts";

interface WordCloudProps {
  initialEntries: FeedTitleIndexEntry[];
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

/** Pick a color class based on a word's share of the highest TF-IDF score. */
function colorForScore(score: number, maxScore: number): string {
  if (maxScore === 0) return "text-fuji-gray";
  const ratio = score / maxScore;
  if (ratio > 0.7) return "text-carp-yellow";
  if (ratio > 0.4) return "text-spring-blue";
  if (ratio > 0.2) return "text-spring-green";
  return "text-old-white";
}

export function newsUrlForTopicSource(word: string, feedSrcId: number): string {
  const params = new URLSearchParams();
  params.set("q", word);
  params.set("source_id", String(feedSrcId));
  return `/news?${params}`;
}

export default function WordCloud(
  { initialEntries, feedNames }: WordCloudProps,
) {
  const entries = useSignal<FeedTitleIndexEntry[]>(initialEntries);
  const selected = useSignal<FeedTitleIndexEntry | null>(null);
  const loading = useSignal(false);
  const error = useSignal<string | null>(null);

  async function refresh() {
    loading.value = true;
    error.value = null;
    try {
      entries.value = await fetchRecentIndex();
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
        No topic data available for the last 24 hours.
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

  const maxTfIdf = Math.max(...data.map((e) => e.tf_idf));
  const minTfIdf = Math.min(...data.map((e) => e.tf_idf));

  const minFontSize = 0.75;
  const maxFontSize = 3;
  const selectedEntry = selected.value;
  const selectedMaxOccurrences = selectedEntry
    ? Math.max(...selectedEntry.items.map((i) => i.occurrences))
    : 0;

  return (
    <div>
      {/* Header bar */}
      <div class="px-4 py-2 border-b border-sumi-ink3 flex items-center justify-between text-xs text-fuji-gray">
        <span>{data.length} topics found in the last 24 hours</span>
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
            entry.tf_idf,
            minTfIdf,
            maxTfIdf,
            minFontSize,
            maxFontSize,
          );
          const color = colorForScore(entry.tf_idf, maxTfIdf);
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
              title={`${entry.word}: ${entry.total_occurrences} occurrences, tf-idf ${
                entry.tf_idf.toFixed(2)
              }`}
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
          most distinctive
        </span>
        <span class="flex items-center gap-1">
          <span class="inline-block w-2 h-2 rounded-full bg-spring-blue" />
          distinctive
        </span>
        <span class="flex items-center gap-1">
          <span class="inline-block w-2 h-2 rounded-full bg-spring-green" />
          somewhat distinctive
        </span>
        <span class="flex items-center gap-1">
          <span class="inline-block w-2 h-2 rounded-full bg-old-white" />
          common
        </span>
      </div>

      {/* Detail panel */}
      {selectedEntry && (
        <div class="mx-4 mb-4 rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4">
          <div class="flex items-baseline justify-between mb-3">
            <h3 class="text-lg font-semibold text-fuji-white">
              {selectedEntry.word}
            </h3>
            <div class="text-right text-sm text-fuji-gray">
              <div>
                {selectedEntry.total_occurrences} occurrence{selectedEntry
                    .total_occurrences !== 1
                  ? "s"
                  : ""} in {selectedEntry.document_frequency}{" "}
                title{selectedEntry.document_frequency !== 1 ? "s" : ""} across
                {" "}
                {selectedEntry.items.length} source
                {selectedEntry.items.length !== 1 ? "s" : ""}
              </div>
              <div class="text-xs text-fuji-gray/70">
                tf-idf score: {selectedEntry.tf_idf.toFixed(2)}
              </div>
            </div>
          </div>

          <div class="space-y-2">
            {selectedEntry.items.map((item) => {
              const barWidth = selectedMaxOccurrences > 0
                ? (item.occurrences / selectedMaxOccurrences) * 100
                : 0;
              const feedName = feedNames[item.feed_src_id] ??
                `Source #${item.feed_src_id}`;

              return (
                <a
                  key={item.feed_src_id}
                  href={newsUrlForTopicSource(
                    selectedEntry.word,
                    item.feed_src_id,
                  )}
                  class="block space-y-1 rounded-md px-2 py-1 transition hover:bg-sumi-ink3"
                  title={`Show ${selectedEntry.word} news from ${feedName}`}
                >
                  <div class="flex items-center justify-between text-sm">
                    <span class="text-old-white truncate max-w-[60%]">
                      {feedName}
                    </span>
                    <span class="text-fuji-gray font-mono text-xs">
                      {item.occurrences}x
                    </span>
                  </div>
                  <div class="h-1.5 w-full rounded-full bg-sumi-ink3">
                    <div
                      class="h-1.5 rounded-full bg-carp-yellow transition-all duration-300"
                      style={{ width: `${Math.max(barWidth, 2)}%` }}
                    />
                  </div>
                </a>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
