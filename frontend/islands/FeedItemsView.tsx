import { useSignal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import { FeedItemCard } from "../components/FeedItemCard.tsx";
import type { FeedItem } from "../types.ts";

interface FeedItemsViewProps {
  initialItems: FeedItem[];
  feedNames: Record<number, string>;
  initialNowIso: string;
}

function sortByNewestId(items: FeedItem[]): FeedItem[] {
  return [...items].sort((a, b) => b.id - a.id);
}

export default function FeedItemsView(
  { initialItems, feedNames, initialNowIso }: FeedItemsViewProps,
) {
  const query = useSignal("");
  const selectedFeedId = useSignal<string>("all");
  const nowMs = useSignal(new Date(initialNowIso).getTime());

  useEffect(() => {
    const id = setInterval(() => {
      nowMs.value = Date.now();
    }, 60_000);
    return () => clearInterval(id);
  }, []);

  const items = sortByNewestId(initialItems);
  const normalizedQuery = query.value.trim().toLowerCase();
  const selectedId = selectedFeedId.value === "all"
    ? null
    : Number(selectedFeedId.value);

  const visibleItems = items.filter((item) => {
    if (selectedId !== null && item.feed_id !== selectedId) {
      return false;
    }
    if (!normalizedQuery) {
      return true;
    }
    const name = (feedNames[item.feed_id] || "").toLowerCase();
    const summary = (item.summary || "").toLowerCase();
    const author = (item.author || "").toLowerCase();
    return item.title.toLowerCase().includes(normalizedQuery) ||
      item.url.toLowerCase().includes(normalizedQuery) ||
      summary.includes(normalizedQuery) ||
      author.includes(normalizedQuery) ||
      name.includes(normalizedQuery);
  });

  const feedOptions = Object.entries(feedNames)
    .map(([id, name]) => ({ id, name }))
    .sort((a, b) => a.name.localeCompare(b.name));

  return (
    <div>
      <div class="mx-4 my-5 rounded-lg border border-neutral-800 bg-neutral-900/60 p-4 space-y-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h2 class="text-sm font-semibold text-neutral-100">Feed items</h2>
          <span class="text-xs text-neutral-500">
            {visibleItems.length} shown
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
            class="rounded-md border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-neutral-200 outline-none transition focus:border-amber-500"
            placeholder="Filter by title, summary, author, URL, or feed"
          />
          <select
            value={selectedFeedId.value}
            onChange={(event) => {
              const target = event.currentTarget as HTMLSelectElement;
              selectedFeedId.value = target.value;
            }}
            class="rounded-md border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-neutral-200 outline-none transition focus:border-amber-500"
          >
            <option value="all">All feeds</option>
            {feedOptions.map((feed) => (
              <option key={feed.id} value={feed.id}>{feed.name}</option>
            ))}
          </select>
        </div>
      </div>

      <div class="border-y border-neutral-800">
        {visibleItems.map((item) => (
          <FeedItemCard
            key={item.id}
            item={item}
            feedName={feedNames[item.feed_id]}
            nowMs={nowMs.value}
          />
        ))}
        {visibleItems.length === 0 && (
          <div class="px-4 py-12 text-center text-neutral-500">
            No items match the current filters.
          </div>
        )}
      </div>
    </div>
  );
}
