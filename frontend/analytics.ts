export type AnalyticsScriptConfig =
  | {
    provider: "plausible";
    scriptSrc: string;
    domain: string;
  }
  | {
    provider: "umami";
    scriptSrc: string;
    websiteId: string;
  };

function readEnv(name: string): string | undefined {
  try {
    const value = typeof Deno !== "undefined" && "env" in Deno
      ? Deno.env.get(name)
      : undefined;
    return value?.trim() || undefined;
  } catch {
    return undefined;
  }
}

export function getAnalyticsScriptConfig(
  env: (name: string) => string | undefined = readEnv,
): AnalyticsScriptConfig | undefined {
  const provider = env("ANALYTICS_PROVIDER")?.toLowerCase();
  const scriptSrc = env("ANALYTICS_SCRIPT_SRC");

  switch (provider) {
    case "plausible": {
      const domain = env("ANALYTICS_DOMAIN");
      if (!domain) {
        return undefined;
      }

      return {
        provider,
        domain,
        scriptSrc: scriptSrc ?? "https://plausible.io/js/script.js",
      };
    }
    case "umami": {
      const websiteId = env("ANALYTICS_WEBSITE_ID");
      if (!websiteId) {
        return undefined;
      }

      return {
        provider,
        websiteId,
        scriptSrc: scriptSrc ?? "https://cloud.umami.is/script.js",
      };
    }
    default:
      return undefined;
  }
}

export const ANALYTICS_SCRIPT_CONFIG = getAnalyticsScriptConfig();
