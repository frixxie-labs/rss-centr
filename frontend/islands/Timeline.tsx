import { useSignal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import type { FeedItem, NewFeedItemEvent } from "../types.ts";
import { FeedItemCard } from "../components/FeedItemCard.tsx";
import { getLogger } from "../logger.ts";

const log = getLogger("sse");
const MAX_TIMELINE_ITEMS = 500;

interface TimelineProps {
  initialItems: FeedItem[];
  feedNames: Record<number, string>;
  initialNowIso: string;
}

function sortByNewestId(items: FeedItem[]): FeedItem[] {
  return [...items].sort((a, b) => b.id - a.id);
}

function upsertByNewestId(items: FeedItem[], nextItem: FeedItem): FeedItem[] {
  const deduped = items.filter((item) => item.id !== nextItem.id);

  let insertIndex = 0;
  while (
    insertIndex < deduped.length &&
    deduped[insertIndex].id > nextItem.id
  ) {
    insertIndex += 1;
  }

  const next = [
    ...deduped.slice(0, insertIndex),
    nextItem,
    ...deduped.slice(insertIndex),
  ];

  return next.slice(0, MAX_TIMELINE_ITEMS);
}

export default function Timeline(
  { initialItems, feedNames, initialNowIso }: TimelineProps,
) {
  const items = useSignal<FeedItem[]>(
    sortByNewestId(initialItems).slice(0, MAX_TIMELINE_ITEMS),
  );
  const newItemIds = useSignal<Set<number>>(new Set());
  const connected = useSignal(false);
  const keepAlivePulse = useSignal(false);
  const nowMs = useSignal(new Date(initialNowIso).getTime());

  // Re-render every 60s so relative timestamps stay fresh
  useEffect(() => {
    const id = setInterval(() => {
      nowMs.value = Date.now();
    }, 60_000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    const eventSource = new EventSource("/api/items/stream");
    const clearNewItemTimers = new Set<number>();
    let keepAlivePulseTimer: number | undefined;

    eventSource.addEventListener("open", () => {
      connected.value = true;
    });

    eventSource.addEventListener("feed_item", (e: MessageEvent) => {
      try {
        const event: NewFeedItemEvent = JSON.parse(e.data);
        const newItem: FeedItem = {
          id: event.id,
          feed_id: event.feed_id,
          external_id: event.external_id,
          title: event.title,
          url: event.url,
          inserted_at: event.inserted_at,
        };

        items.value = upsertByNewestId(items.value, newItem);
        newItemIds.value = new Set([...newItemIds.value, newItem.id]);

        // Clear "new" highlight after 30 seconds
        const timer = setTimeout(() => {
          newItemIds.value = new Set(
            [...newItemIds.value].filter((id) => id !== newItem.id),
          );
          clearNewItemTimers.delete(timer);
        }, 30_000);
        clearNewItemTimers.add(timer);
      } catch (err) {
        log.error("Failed to parse SSE event", err);
      }
    });

    eventSource.addEventListener("lagged", (e: MessageEvent) => {
      log.warn("SSE lagged, missed messages", { data: e.data });
    });

    eventSource.addEventListener("error", () => {
      connected.value = false;
    });

    eventSource.addEventListener("keep_alive", (_e: MessageEvent) => {
      keepAlivePulse.value = true;
      if (keepAlivePulseTimer !== undefined) {
        clearTimeout(keepAlivePulseTimer);
      }
      keepAlivePulseTimer = setTimeout(() => {
        keepAlivePulse.value = false;
      }, 450);
    });

    eventSource.addEventListener("message", (e: MessageEvent) => {
      if (e.data !== "keep-alive") {
        return;
      }

      keepAlivePulse.value = true;
      if (keepAlivePulseTimer !== undefined) {
        clearTimeout(keepAlivePulseTimer);
      }
      keepAlivePulseTimer = setTimeout(() => {
        keepAlivePulse.value = false;
      }, 450);
    });

    return () => {
      eventSource.close();
      for (const timer of clearNewItemTimers) {
        clearTimeout(timer);
      }
      if (keepAlivePulseTimer !== undefined) {
        clearTimeout(keepAlivePulseTimer);
      }
    };
  }, []);

  return (
    <div>
      <div class="px-4 py-2 border-b border-neutral-800 flex items-center justify-between text-xs text-neutral-500">
        <span>{items.value.length} items</span>
        <div class="flex items-center gap-2">
          {newItemIds.value.size > 0 && (
            <span class="text-amber-500">{newItemIds.value.size} new</span>
          )}
          <span class="flex items-center gap-1">
            <span
              class={`inline-block h-1.5 w-1.5 rounded-full transition-transform duration-300 ${
                connected.value ? "bg-emerald-500" : "bg-neutral-600"
              } ${keepAlivePulse.value ? "scale-150" : "scale-100"}`}
            />
            {connected.value ? "live" : "connecting..."}
          </span>
        </div>
      </div>
      <div>
        {items.value.map((item) => (
          <FeedItemCard
            key={item.id}
            item={item}
            feedName={feedNames[item.feed_id]}
            isNew={newItemIds.value.has(item.id)}
            nowMs={nowMs.value}
          />
        ))}
        {items.value.length === 0 && (
          <div class="px-4 py-12 text-center text-neutral-600">
            No items yet. Add some feeds to get started.
          </div>
        )}
      </div>
    </div>
  );
}
