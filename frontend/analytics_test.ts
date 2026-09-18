import { assertEquals } from "@std/assert";
import { getAnalyticsScriptConfig } from "./analytics.ts";

Deno.test("getAnalyticsScriptConfig - returns undefined when analytics is disabled", () => {
  assertEquals(getAnalyticsScriptConfig(() => undefined), undefined);
});

Deno.test("getAnalyticsScriptConfig - returns Plausible config with default script", () => {
  const config = getAnalyticsScriptConfig((name) => {
    switch (name) {
      case "ANALYTICS_PROVIDER":
        return "Plausible";
      case "ANALYTICS_DOMAIN":
        return "rss.example.com";
      default:
        return undefined;
    }
  });

  assertEquals(config, {
    provider: "plausible",
    domain: "rss.example.com",
    scriptSrc: "https://plausible.io/js/script.js",
  });
});

Deno.test("getAnalyticsScriptConfig - returns Umami config with custom script", () => {
  const config = getAnalyticsScriptConfig((name) => {
    switch (name) {
      case "ANALYTICS_PROVIDER":
        return "umami";
      case "ANALYTICS_WEBSITE_ID":
        return "site-123";
      case "ANALYTICS_SCRIPT_SRC":
        return "https://analytics.example.com/script.js";
      default:
        return undefined;
    }
  });

  assertEquals(config, {
    provider: "umami",
    websiteId: "site-123",
    scriptSrc: "https://analytics.example.com/script.js",
  });
});

Deno.test("getAnalyticsScriptConfig - ignores incomplete provider config", () => {
  const plausible = getAnalyticsScriptConfig((name) =>
    name === "ANALYTICS_PROVIDER" ? "plausible" : undefined
  );
  const umami = getAnalyticsScriptConfig((name) =>
    name === "ANALYTICS_PROVIDER" ? "umami" : undefined
  );

  assertEquals(plausible, undefined);
  assertEquals(umami, undefined);
});

Deno.test("getAnalyticsScriptConfig - falls back when custom script URL is unsafe", () => {
  const config = getAnalyticsScriptConfig((name) => {
    switch (name) {
      case "ANALYTICS_PROVIDER":
        return "UMAMI";
      case "ANALYTICS_WEBSITE_ID":
        return "site-123";
      case "ANALYTICS_SCRIPT_SRC":
        return "http://analytics.example.com/script.js";
      default:
        return undefined;
    }
  });

  assertEquals(config, {
    provider: "umami",
    websiteId: "site-123",
    scriptSrc: "https://cloud.umami.is/script.js",
  });
});

Deno.test("getAnalyticsScriptConfig - accepts a normalized root-relative script path", () => {
  const config = getAnalyticsScriptConfig((name) => {
    switch (name) {
      case "ANALYTICS_PROVIDER":
        return "umami";
      case "ANALYTICS_WEBSITE_ID":
        return "site-123";
      case "ANALYTICS_SCRIPT_SRC":
        return "/js/analytics.js";
      default:
        return undefined;
    }
  });

  assertEquals(config, {
    provider: "umami",
    websiteId: "site-123",
    scriptSrc: "/js/analytics.js",
  });
});

Deno.test("getAnalyticsScriptConfig - rejects protocol-relative and non-normalized script paths", () => {
  const protocolRelative = getAnalyticsScriptConfig((name) => {
    switch (name) {
      case "ANALYTICS_PROVIDER":
        return "plausible";
      case "ANALYTICS_DOMAIN":
        return "rss.example.com";
      case "ANALYTICS_SCRIPT_SRC":
        return "//evil.example.com/script.js";
      default:
        return undefined;
    }
  });
  const nonNormalized = getAnalyticsScriptConfig((name) => {
    switch (name) {
      case "ANALYTICS_PROVIDER":
        return "plausible";
      case "ANALYTICS_DOMAIN":
        return "rss.example.com";
      case "ANALYTICS_SCRIPT_SRC":
        return "/../evil.js";
      default:
        return undefined;
    }
  });

  assertEquals(protocolRelative, {
    provider: "plausible",
    domain: "rss.example.com",
    scriptSrc: "https://plausible.io/js/script.js",
  });
  assertEquals(nonNormalized, {
    provider: "plausible",
    domain: "rss.example.com",
    scriptSrc: "https://plausible.io/js/script.js",
  });
});
