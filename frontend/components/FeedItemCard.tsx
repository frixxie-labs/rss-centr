import type { FeedItem } from "../types.ts";

function timeAgo(dateStr: string, nowMs: number): string {
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

function hostname(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return "";
  }
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
  return (
    <a
      href={item.url}
      target="_blank"
      rel="noopener noreferrer"
      class={`block px-4 py-3 border-b border-neutral-800 hover:bg-neutral-900 transition-colors ${
        isNew ? "bg-neutral-900/50 border-l-2 border-l-amber-500" : ""
      }`}
    >
      <h3 class="text-sm font-medium text-neutral-100 leading-snug">
        {item.title}
      </h3>
      <div class="mt-1 flex items-center gap-2 text-xs text-neutral-500">
        <span>{hostname(item.url)}</span>
        {feedName && (
          <>
            <span>&middot;</span>
            <span class="text-neutral-400">{feedName}</span>
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
