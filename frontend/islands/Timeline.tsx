import { useComputed, useSignal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import type { FeedItem, NewFeedItemEvent } from "../types.ts";
import { FeedItemCard } from "../components/FeedItemCard.tsx";
import { getLogger } from "../logger.ts";

const log = getLogger("sse");

interface TimelineProps {
  initialItems: FeedItem[];
  feedNames: Record<number, string>;
}

export default function Timeline({ initialItems, feedNames }: TimelineProps) {
  const items = useSignal<FeedItem[]>(initialItems);
  const newItemIds = useSignal<Set<number>>(new Set());
  const connected = useSignal(false);
  const newCount = useSignal(0);
  const tick = useSignal(0);

  // Re-render every 60s so relative timestamps stay fresh
  useEffect(() => {
    const id = setInterval(() => {
      tick.value += 1;
    }, 60_000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    const eventSource = new EventSource("/api/items/stream");

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

        // Prepend to list, deduplicate by id
        items.value = [
          newItem,
          ...items.value.filter((i) => i.id !== newItem.id),
        ];
        newItemIds.value = new Set([...newItemIds.value, newItem.id]);
        newCount.value += 1;

        // Clear "new" highlight after 30 seconds
        setTimeout(() => {
          newItemIds.value = new Set(
            [...newItemIds.value].filter((id) => id !== newItem.id),
          );
        }, 30_000);
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

    return () => {
      eventSource.close();
    };
  }, []);

  const sortedItems = useComputed(() => {
    // Subscribe to tick so timestamps re-render periodically
    tick.value;
    return [...items.value].sort((a, b) => b.id - a.id);
  });

  return (
    <div>
      <div class="px-4 py-2 border-b border-neutral-800 flex items-center justify-between text-xs text-neutral-500">
        <span>{items.value.length} items</span>
        <div class="flex items-center gap-2">
          {newCount.value > 0 && (
            <span class="text-amber-500">{newCount.value} new</span>
          )}
          <span class="flex items-center gap-1">
            <span
              class={`inline-block w-1.5 h-1.5 rounded-full ${
                connected.value ? "bg-emerald-500" : "bg-neutral-600"
              }`}
            />
            {connected.value ? "live" : "connecting..."}
          </span>
        </div>
      </div>
      <div>
        {sortedItems.value.map((item) => (
          <FeedItemCard
            key={item.id}
            item={item}
            feedName={feedNames[item.feed_id]}
            isNew={newItemIds.value.has(item.id)}
          />
        ))}
        {sortedItems.value.length === 0 && (
          <div class="px-4 py-12 text-center text-neutral-600">
            No items yet. Add some feeds to get started.
          </div>
        )}
      </div>
    </div>
  );
}
