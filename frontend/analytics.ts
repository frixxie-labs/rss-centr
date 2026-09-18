export type AnalyticsEventType =
  | "page_view"
  | "item_open"
  | "search_performed"
  | "feed_added"
  | "feed_fetch_requested";

export interface AnalyticsEventInput {
  eventType: AnalyticsEventType;
  path?: string;
  referrer?: string | null;
  visitorId?: string | null;
  sessionId?: string | null;
  feedId?: number;
  itemId?: number;
}

const VISITOR_ID_STORAGE_KEY = "rss:analytics:visitor-id";
const SESSION_ID_STORAGE_KEY = "rss:analytics:session-id";

export function shouldTrackAnalytics(): boolean {
  if (typeof window === "undefined") {
    return false;
  }

  const navigatorWithLegacyDnt = globalThis.navigator as Navigator & {
    msDoNotTrack?: string;
  };
  const globalWithDnt = globalThis as typeof globalThis & {
    doNotTrack?: string;
  };
  const doNotTrack = globalThis.navigator.doNotTrack ??
    navigatorWithLegacyDnt.msDoNotTrack ??
    globalWithDnt.doNotTrack;
  return doNotTrack !== "1";
}

export function normalizeAnalyticsPath(
  path: string | null | undefined,
): string {
  const trimmed = path?.trim();
  if (!trimmed || !trimmed.startsWith("/")) {
    return "/";
  }
  return trimmed;
}

export function sanitizeAnalyticsReferrer(
  referrer: string | null | undefined,
  currentUrl: URL,
): string | null {
  const trimmed = referrer?.trim();
  if (!trimmed) {
    return null;
  }

  try {
    const referrerUrl = new URL(trimmed);
    if (referrerUrl.origin === currentUrl.origin) {
      return normalizeAnalyticsPath(referrerUrl.pathname);
    }
    return referrerUrl.origin;
  } catch {
    return null;
  }
}

function createOpaqueId(): string | null {
  try {
    return globalThis.crypto.randomUUID();
  } catch {
    return null;
  }
}

function getOrCreateStorageId(storage: Storage, key: string): string | null {
  try {
    const existing = storage.getItem(key);
    if (existing) {
      return existing;
    }

    const created = createOpaqueId();
    if (!created) {
      return null;
    }

    storage.setItem(key, created);
    return created;
  } catch {
    return null;
  }
}

export function getAnalyticsVisitorId(): string | null {
  return getOrCreateStorageId(globalThis.localStorage, VISITOR_ID_STORAGE_KEY);
}

export function getAnalyticsSessionId(): string | null {
  return getOrCreateStorageId(
    globalThis.sessionStorage,
    SESSION_ID_STORAGE_KEY,
  );
}

export async function trackEvent(input: AnalyticsEventInput): Promise<void> {
  if (!shouldTrackAnalytics()) {
    return;
  }

  const currentUrl = new URL(globalThis.location.href);
  const path = normalizeAnalyticsPath(input.path ?? currentUrl.pathname);
  const referrer = sanitizeAnalyticsReferrer(
    input.referrer ?? globalThis.document.referrer,
    currentUrl,
  );

  try {
    await fetch("/api/analytics/events", {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
      keepalive: true,
      body: JSON.stringify({
        event_type: input.eventType,
        path,
        referrer,
        visitor_id: input.visitorId ?? getAnalyticsVisitorId(),
        session_id: input.sessionId ?? getAnalyticsSessionId(),
        feed_id: input.feedId,
        item_id: input.itemId,
      }),
    });
  } catch {
    // Best-effort only.
  }
}

export async function trackPageView(path?: string): Promise<void> {
  await trackEvent({ eventType: "page_view", path });
}
