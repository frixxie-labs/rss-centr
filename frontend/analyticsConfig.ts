function readAnalyticsEnabledFromEnv(): string | undefined {
  try {
    return typeof Deno !== "undefined" && "env" in Deno
      ? Deno.env.get("ANALYTICS_ENABLED")
      : undefined;
  } catch {
    return undefined;
  }
}

export const ANALYTICS_ENABLED =
  /^(1|true|yes|on)$/i.test(readAnalyticsEnabledFromEnv() ?? "");
