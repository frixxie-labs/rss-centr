export function parseSourceId(url: URL): number | undefined {
  const value = url.searchParams.get("source_id") ??
    url.searchParams.get("feed_id");
  if (!value) {
    return undefined;
  }

  const id = Number(value);
  return Number.isInteger(id) && id > 0 ? id : undefined;
}
