import { useSignal } from "@preact/signals";
import {
  createFeed,
  deleteFeed,
  fetchFeeds,
  queueFeedIngest,
  updateFeedEnabled,
} from "../api.ts";
import { Button } from "../components/Button.tsx";
import type { FeedSubscription } from "../types.ts";

interface FeedManagementProps {
  initialFeeds: FeedSubscription[];
  initialLoadError: boolean;
}

export function sortByNewestId(feeds: FeedSubscription[]): FeedSubscription[] {
  return [...feeds].sort((a, b) => b.id - a.id);
}

export function formatDateTime(value: string | null): string {
  if (!value) {
    return "Never";
  }

  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "Unknown";
  }

  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

export function feedName(feed: FeedSubscription): string {
  return feed.title?.trim() || feed.site_url?.trim() || feed.url;
}

export function formatPollInterval(seconds: number): string {
  if (seconds < 60) {
    return `${seconds}s`;
  }

  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = seconds % 60;

  if (hours > 0) {
    if (minutes > 0) {
      return `${hours}h ${minutes}m`;
    }

    return `${hours}h`;
  }

  if (secs === 0) {
    return `${minutes}m`;
  }

  return `${minutes}m ${secs}s`;
}

export default function FeedManagement(
  { initialFeeds, initialLoadError }: FeedManagementProps,
) {
  const feeds = useSignal<FeedSubscription[]>(sortByNewestId(initialFeeds));
  const urlInput = useSignal("");
  const submitting = useSignal(false);
  const refreshing = useSignal(false);
  const busyIds = useSignal<Set<number>>(new Set());
  const errorMessage = useSignal<string | null>(
    initialLoadError ? "Could not load sources." : null,
  );
  const successMessage = useSignal<string | null>(null);

  async function refreshFeeds() {
    refreshing.value = true;
    errorMessage.value = null;
    try {
      feeds.value = sortByNewestId(await fetchFeeds());
    } catch (_err) {
      errorMessage.value = "Could not refresh sources.";
    } finally {
      refreshing.value = false;
    }
  }

  async function handleAddFeed() {
    const url = urlInput.value.trim();
    if (!url) {
      errorMessage.value = "Source URL is required.";
      return;
    }

    submitting.value = true;
    errorMessage.value = null;
    successMessage.value = null;

    try {
      await createFeed(url);
      urlInput.value = "";
      await refreshFeeds();
      successMessage.value = "Source saved.";
    } catch (err) {
      errorMessage.value = err instanceof Error
        ? err.message
        : "Failed to save source.";
    } finally {
      submitting.value = false;
    }
  }

  async function handleToggle(feed: FeedSubscription) {
    busyIds.value = new Set([...busyIds.value, feed.id]);
    errorMessage.value = null;
    successMessage.value = null;

    try {
      const nextEnabled = !feed.is_enabled;
      await updateFeedEnabled(feed.id, nextEnabled);
      await refreshFeeds();
      successMessage.value = nextEnabled ? "Source enabled." : "Source paused.";
    } catch (err) {
      errorMessage.value = err instanceof Error
        ? err.message
        : "Failed to update source.";
    } finally {
      busyIds.value = new Set(
        [...busyIds.value].filter((id) => id !== feed.id),
      );
    }
  }

  async function handleIngest(feedId: number) {
    busyIds.value = new Set([...busyIds.value, feedId]);
    errorMessage.value = null;
    successMessage.value = null;

    try {
      await queueFeedIngest(feedId);
      successMessage.value = "Fetch queued.";
    } catch (err) {
      errorMessage.value = err instanceof Error
        ? err.message
        : "Failed to queue fetch.";
    } finally {
      busyIds.value = new Set(
        [...busyIds.value].filter((id) => id !== feedId),
      );
    }
  }

  async function handleDelete(feed: FeedSubscription) {
    if (!confirm(`Delete source?\n\n${feed.url}`)) {
      return;
    }

    busyIds.value = new Set([...busyIds.value, feed.id]);
    errorMessage.value = null;
    successMessage.value = null;

    try {
      await deleteFeed(feed.id);
      await refreshFeeds();
      successMessage.value = "Source deleted.";
    } catch (err) {
      errorMessage.value = err instanceof Error
        ? err.message
        : "Failed to delete source.";
    } finally {
      busyIds.value = new Set(
        [...busyIds.value].filter((id) => id !== feed.id),
      );
    }
  }

  return (
    <div class="mx-4 my-5 space-y-4">
      <section class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60 p-4 space-y-3">
        <h2 class="text-sm font-semibold text-fuji-white">Add source</h2>
        <form
          class="flex flex-col gap-2 sm:flex-row"
          onSubmit={(event) => {
            event.preventDefault();
            void handleAddFeed();
          }}
        >
          <input
            type="url"
            value={urlInput.value}
            onInput={(event) => {
              const target = event.currentTarget as HTMLInputElement;
              urlInput.value = target.value;
            }}
            class="flex-1 rounded-md border border-sumi-ink4 bg-sumi-ink0 px-3 py-2 text-sm text-old-white outline-none transition focus:border-carp-yellow"
            placeholder="https://example.com/feed.xml"
            required
          />
          <Button
            type="submit"
            variant="primary"
            disabled={submitting.value || refreshing.value}
          >
            {submitting.value ? "Saving..." : "Save source"}
          </Button>
        </form>
      </section>

      {errorMessage.value && (
        <div class="rounded-md border border-autumn-red/70 bg-winter-red/40 px-3 py-2 text-sm text-wave-red">
          {errorMessage.value}
        </div>
      )}

      {successMessage.value && (
        <div class="rounded-md border border-autumn-green/70 bg-winter-green/40 px-3 py-2 text-sm text-spring-green">
          {successMessage.value}
        </div>
      )}

      <section class="rounded-lg border border-sumi-ink3 bg-sumi-ink2/60">
        <div class="flex items-center justify-between border-b border-sumi-ink3 px-4 py-3">
          <h2 class="text-sm font-semibold text-fuji-white">
            Sources ({feeds.value.length})
          </h2>
          <Button
            type="button"
            variant="ghost"
            onClick={() => {
              void refreshFeeds();
            }}
            disabled={refreshing.value}
          >
            {refreshing.value ? "Refreshing..." : "Refresh"}
          </Button>
        </div>

        {feeds.value.length === 0
          ? (
            <div class="px-4 py-8 text-sm text-katana-gray">
              No sources yet. Add one above to start collecting news.
            </div>
          )
          : (
            <div class="divide-y divide-sumi-ink3">
              {feeds.value.map((feed) => {
                const isBusy = busyIds.value.has(feed.id);
                return (
                  <article key={feed.id} class="px-4 py-4 space-y-3">
                    <div class="flex flex-wrap items-center gap-2">
                      <h3 class="text-sm font-semibold text-fuji-white">
                        {feedName(feed)}
                      </h3>
                      <span
                        class={`rounded px-2 py-0.5 text-xs font-medium ${
                          feed.is_enabled
                            ? "bg-winter-green text-spring-green"
                            : "bg-sumi-ink3 text-old-white"
                        }`}
                      >
                        {feed.is_enabled ? "Enabled" : "Paused"}
                      </span>
                    </div>

                    <div class="space-y-1 text-xs text-fuji-gray">
                      <p class="break-all text-old-white">{feed.url}</p>
                      <p>
                        Last checked: {formatDateTime(feed.last_checked_at)}
                        {" "}
                        | Last success: {formatDateTime(feed.last_success_at)}
                      </p>
                      <p>
                        Check every{" "}
                        {formatPollInterval(feed.poll_interval_seconds)}{" "}
                        | Failures: {feed.failure_count}
                      </p>
                    </div>

                    <div class="flex flex-wrap gap-2">
                      <Button
                        type="button"
                        variant={feed.is_enabled ? "secondary" : "primary"}
                        onClick={() => {
                          void handleToggle(feed);
                        }}
                        disabled={isBusy}
                      >
                        {feed.is_enabled ? "Pause" : "Enable"}
                      </Button>
                      <Button
                        type="button"
                        variant="secondary"
                        onClick={() => {
                          void handleIngest(feed.id);
                        }}
                        disabled={isBusy}
                      >
                        Fetch now
                      </Button>
                      <Button
                        type="button"
                        variant="danger"
                        onClick={() => {
                          void handleDelete(feed);
                        }}
                        disabled={isBusy}
                      >
                        Delete
                      </Button>
                    </div>
                  </article>
                );
              })}
            </div>
          )}
      </section>
    </div>
  );
}
