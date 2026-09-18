export function renderAnalyticsEventType(eventType: string): string {
  return eventType.replaceAll("_", " ");
}
