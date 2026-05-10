import { useSignal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import { fetchLatestItems } from "../api.ts";
import { FeedItemCard } from "../components/FeedItemCard.tsx";
import type { FeedItem } from "../types.ts";

interface FeedItemsViewProps {
  initialItems: FeedItem[];
  feedNames: Record<number, string>;
  initialNowIso: string;
  limit: number;
}

function effectiveDate(item: FeedItem): number {
  return new Date(item.published_at ?? item.inserted_at).getTime();
}

function sortByNewest(items: FeedItem[]): FeedItem[] {
  return [...items].sort((a, b) => {
    const dateDiff = effectiveDate(b) - effectiveDate(a);
    return dateDiff !== 0 ? dateDiff : b.id - a.id;
  });
}

export default function FeedItemsView(
  { initialItems, feedNames, initialNowIso, limit }: FeedItemsViewProps,
) {
  const query = useSignal("");
  const selectedFeedId = useSignal<string>("all");
  const items = useSignal(sortByNewest(initialItems));
  const isLoading = useSignal(false);
  const loadError = useSignal(false);
  const nowMs = useSignal(new Date(initialNowIso).getTime());

  useEffect(() => {
    const id = setInterval(() => {
      nowMs.value = Date.now();
    }, 60_000);
    return () => clearInterval(id);
  }, []);

  const normalizedQuery = query.value.trim().toLowerCase();
  const selectedId = selectedFeedId.value === "all"
    ? null
    : Number(selectedFeedId.value);

  useEffect(() => {
    const controller = new AbortController();
    const timeoutId = setTimeout(async () => {
      isLoading.value = true;
      loadError.value = false;

      try {
        const result = await fetchLatestItems({
          limit,
          feedId: selectedId ?? undefined,
          query: normalizedQuery || undefined,
          signal: controller.signal,
        });
        items.value = sortByNewest(result);
      } catch (err) {
        if (err instanceof DOMException && err.name === "AbortError") {
          return;
        }
        loadError.value = true;
      } finally {
        if (!controller.signal.aborted) {
          isLoading.value = false;
        }
      }
    }, 250);

    return () => {
      clearTimeout(timeoutId);
      controller.abort();
    };
  }, [limit, normalizedQuery, selectedId]);

  const feedOptions = Object.entries(feedNames)
    .map(([id, name]) => ({ id, name }))
    .sort((a, b) => a.name.localeCompare(b.name));

  return (
    <div>
      <div class="mx-4 my-5 rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4 space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h2 class="text-sm font-semibold text-fuji-white">Feed items</h2>
          <span class="text-xs text-katana-gray">
            {isLoading.value ? "Loading..." : `${items.value.length} shown`}
          </span>
        </div>
        <div class="grid gap-2 sm:grid-cols-[1fr_180px]">
          <input
            type="search"
            value={query.value}
            onInput={(event) => {
              const target = event.currentTarget as HTMLInputElement;
              query.value = target.value;
            }}
            class="rounded-md border border-sumi-ink4 bg-sumi-ink0 px-3 py-2 text-sm text-old-white outline-none transition focus:border-carp-yellow"
            placeholder="Filter by title, summary, author, URL, or feed"
          />
          <select
            value={selectedFeedId.value}
            onChange={(event) => {
              const target = event.currentTarget as HTMLSelectElement;
              selectedFeedId.value = target.value;
            }}
            class="rounded-md border border-sumi-ink4 bg-sumi-ink0 px-3 py-2 text-sm text-old-white outline-none transition focus:border-carp-yellow"
          >
            <option value="all">All feeds</option>
            {feedOptions.map((feed) => (
              <option key={feed.id} value={feed.id}>{feed.name}</option>
            ))}
          </select>
        </div>
      </div>

      <div class="border-y border-sumi-ink3">
        {loadError.value && (
          <div class="border-b border-sumi-ink3 px-4 py-3 text-sm text-ronin-yellow">
            Could not load matching items.
          </div>
        )}
        {items.value.map((item) => (
          <FeedItemCard
            key={item.id}
            item={item}
            feedName={feedNames[item.feed_id]}
            nowMs={nowMs.value}
          />
        ))}
        {items.value.length === 0 && !isLoading.value && (
          <div class="px-4 py-12 text-center text-katana-gray">
            No items match the current filters.
          </div>
        )}
      </div>
    </div>
  );
}
