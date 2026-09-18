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

const DEFAULT_SCRIPT_SRC = {
  plausible: "https://plausible.io/js/script.js",
  umami: "https://cloud.umami.is/script.js",
} as const;

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

function normalizeScriptSrc(scriptSrc: string | undefined): string | undefined {
  if (!scriptSrc) {
    return undefined;
  }

  try {
    if (scriptSrc.startsWith("/")) {
      const url = new URL(scriptSrc, "https://rss-centr.local");
      const isConcreteScriptPath = scriptSrc === url.pathname &&
        url.pathname.endsWith(".js");
      return url.origin === "https://rss-centr.local" && isConcreteScriptPath
        ? url.pathname
        : undefined;
    }

    const url = new URL(scriptSrc);
    return url.protocol === "https:" ? url.toString() : undefined;
  } catch {
    return undefined;
  }
}

export function getAnalyticsScriptConfig(
  env: (name: string) => string | undefined = readEnv,
): AnalyticsScriptConfig | undefined {
  const provider = env("ANALYTICS_PROVIDER")?.toLowerCase();
  const scriptSrc = normalizeScriptSrc(env("ANALYTICS_SCRIPT_SRC"));

  switch (provider) {
    case "plausible": {
      const domain = env("ANALYTICS_DOMAIN");
      if (!domain) {
        return undefined;
      }

      return {
        provider,
        domain,
        scriptSrc: scriptSrc ?? DEFAULT_SCRIPT_SRC.plausible,
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
        scriptSrc: scriptSrc ?? DEFAULT_SCRIPT_SRC.umami,
      };
    }
    default:
      return undefined;
  }
}

export const ANALYTICS_SCRIPT_CONFIG = getAnalyticsScriptConfig();
