import { useSignal } from "@preact/signals";
import { useEffect } from "preact/hooks";
import type { FeedItem, NewFeedItemEvent } from "../types.ts";
import { FeedItemCard } from "../components/FeedItemCard.tsx";
import { getLogger } from "../logger.ts";
import { fetchItemWithDetail } from "../api.ts";
import { effectiveDate, sortByNewest } from "../feedItemOrdering.ts";

const log = getLogger("sse");
export const MAX_TIMELINE_ITEMS = 500;
const LAST_EVENT_ID_STORAGE_KEY = "rss:last-event-id";

interface TimelineProps {
  initialItems: FeedItem[];
  feedNames: Record<number, string>;
  initialNowIso: string;
}

export { effectiveDate, sortByNewest };

export function upsertByNewest(
  items: FeedItem[],
  nextItem: FeedItem,
): FeedItem[] {
  const deduped = items.filter((item) => item.id !== nextItem.id);
  const nextDate = effectiveDate(nextItem);

  let insertIndex = 0;
  while (insertIndex < deduped.length) {
    const d = effectiveDate(deduped[insertIndex]);
    if (
      d > nextDate ||
      (d === nextDate && deduped[insertIndex].id > nextItem.id)
    ) {
      insertIndex += 1;
    } else {
      break;
    }
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
    sortByNewest(initialItems).slice(0, MAX_TIMELINE_ITEMS),
  );
  const newItemIds = useSignal<Set<number>>(new Set());
  const connected = useSignal(false);
  const keepAlivePulse = useSignal(false);
  const replayCursor = useSignal<string | null>(null);
  const nowMs = useSignal(new Date(initialNowIso).getTime());

  // Re-render every 60s so relative timestamps stay fresh
  useEffect(() => {
    const id = setInterval(() => {
      nowMs.value = Date.now();
    }, 60_000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    const lastEventId = globalThis.localStorage.getItem(
      LAST_EVENT_ID_STORAGE_KEY,
    );
    replayCursor.value = lastEventId;

    const sseUrl = lastEventId
      ? `/api/items/stream?last_event_id=${encodeURIComponent(lastEventId)}`
      : "/api/items/stream";

    const eventSource = new EventSource(sseUrl);
    const clearNewItemTimers = new Set<ReturnType<typeof setTimeout>>();
    let keepAlivePulseTimer: ReturnType<typeof setTimeout> | undefined;
    let isDisposed = false;

    const refreshItem = async (itemId: number) => {
      try {
        const fullItem = await fetchItemWithDetail(itemId);
        if (isDisposed) {
          return;
        }
        items.value = upsertByNewest(items.value, fullItem);
      } catch (err) {
        log.warn("Failed to refresh new item details", { itemId, err });
      }
    };

    eventSource.addEventListener("open", () => {
      connected.value = true;
    });

    eventSource.addEventListener("feed_item", (e: MessageEvent) => {
      try {
        if (e.lastEventId) {
          globalThis.localStorage.setItem(
            LAST_EVENT_ID_STORAGE_KEY,
            e.lastEventId,
          );
          replayCursor.value = e.lastEventId;
        }

        const event: NewFeedItemEvent = JSON.parse(e.data);
        const newItem: FeedItem = {
          id: event.id,
          feed_id: event.feed_id,
          external_id: event.external_id,
          title: event.title,
          url: event.url,
          inserted_at: event.inserted_at,
        };

        items.value = upsertByNewest(items.value, newItem);
        void refreshItem(newItem.id);
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
      isDisposed = true;
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
      <div class="px-4 py-2 border-b border-sumi-ink3 flex items-center justify-between text-xs text-katana-gray">
        <span>
          {items.value.length} {items.value.length === 1 ? "story" : "stories"}
        </span>
        <div class="flex items-center gap-2">
          {replayCursor.value && (
            <span class="text-sumi-ink4">cursor {replayCursor.value}</span>
          )}
          {newItemIds.value.size > 0 && (
            <span class="text-ronin-yellow">{newItemIds.value.size} new</span>
          )}
          <span class="flex items-center gap-1">
            <span
              class={`inline-block h-1.5 w-1.5 rounded-full transition-transform duration-300 ${
                connected.value ? "bg-spring-green" : "bg-sumi-ink4"
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
          <div class="px-4 py-12 text-center text-sumi-ink4">
            No news yet. Add some sources to get started.
          </div>
        )}
      </div>
    </div>
  );
}
