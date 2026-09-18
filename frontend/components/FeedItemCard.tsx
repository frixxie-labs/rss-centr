import type { FeedItem } from "../types.ts";
import { trackEvent } from "../analytics.ts";

export function timeAgo(dateStr: string, nowMs: number): string {
  const date = new Date(dateStr);
  const seconds = Math.floor((nowMs - date.getTime()) / 1000);

  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return date.toLocaleDateString();
}

export function hostname(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return "";
  }
}

export function compactText(value: string): string {
  return value.replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
}

export function previewText(item: FeedItem): string {
  const summary = item.summary ? compactText(item.summary) : "";
  if (summary) {
    return summary;
  }

  const content = item.content ? compactText(item.content) : "";
  if (content) {
    return content;
  }

  return "";
}

export function truncate(value: string, maxLength: number): string {
  if (value.length <= maxLength) {
    return value;
  }
  return `${value.slice(0, maxLength - 1).trimEnd()}…`;
}

function formatAbsoluteDate(dateStr: string): string {
  const date = new Date(dateStr);
  return date.toLocaleString();
}

interface FeedItemCardProps {
  item: FeedItem;
  feedName?: string;
  isNew?: boolean;
  nowMs: number;
}

export function FeedItemCard(
  { item, feedName, isNew, nowMs }: FeedItemCardProps,
) {
  const preview = truncate(previewText(item), 240);

  return (
    <a
      href={item.url}
      target="_blank"
      rel="noopener noreferrer"
      onClick={() => {
        void trackEvent({
          eventType: "item_open",
          path: globalThis.location?.pathname,
          feedId: item.feed_id,
          itemId: item.id,
        });
      }}
      class={`block min-w-0 px-4 py-3 border-b border-sumi-ink3 hover:bg-sumi-ink2 transition-colors ${
        isNew ? "bg-wave-blue1/50 border-l-2 border-l-carp-yellow" : ""
      }`}
    >
      <h3 class="break-words text-sm font-medium text-fuji-white leading-snug">
        {item.title}
      </h3>
      {preview && (
        <p class="mt-2 line-clamp-3 break-words text-sm leading-relaxed text-fuji-gray">
          {preview}
        </p>
      )}
      <div class="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-katana-gray">
        <span class="break-all">{hostname(item.url)}</span>
        {item.author && (
          <>
            <span>&middot;</span>
            <span class="break-words text-fuji-gray">{item.author}</span>
          </>
        )}
        {item.published_at && (
          <>
            <span>&middot;</span>
            <time
              datetime={item.published_at}
              title={formatAbsoluteDate(item.published_at)}
              class="text-fuji-gray"
            >
              published {timeAgo(item.published_at, nowMs)}
            </time>
          </>
        )}
        {feedName && (
          <>
            <span>&middot;</span>
            <span class="break-words text-fuji-gray">{feedName}</span>
          </>
        )}
        <span>&middot;</span>
        <time datetime={item.inserted_at}>
          {timeAgo(item.inserted_at, nowMs)}
        </time>
      </div>
    </a>
  );
}
