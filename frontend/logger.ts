/**
 * Structured logger built on top of `console.*` methods.
 *
 * Locally you get human-readable leveled output:
 *   [INFO ssr] Fetched 42 news stories
 *   [ERROR api-proxy] Backend unreachable { status: 502, path: "/api/feeds" }
 *
 * When running with `OTEL_DENO=true`, every `console.*` call is automatically
 * exported as an OpenTelemetry log record with the correct severity, span
 * context, and any structured data you pass — no extra dependencies needed.
 *
 * Usage:
 *   import { getLogger } from "@/logger.ts";
 *   const log = getLogger("ssr");
 *   log.info("Page rendered", { route: "/", newsCount: 42 });
 *   log.error("Fetch failed", { status: 500 }, err);
 */

const LEVELS = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
} as const;

type Level = keyof typeof LEVELS;

/** Minimum log level. Set via `LOG_LEVEL` env var (default: "debug"). */
function getMinLevel(): Level {
  try {
    const env =
      (typeof Deno !== "undefined" ? Deno.env.get("LOG_LEVEL") : undefined)
        ?.toLowerCase();
    if (env && env in LEVELS) return env as Level;
  } catch {
    // Deno.env may throw if permission is not granted; default to debug.
  }
  return "debug";
}

export interface Logger {
  debug(msg: string, ...args: unknown[]): void;
  info(msg: string, ...args: unknown[]): void;
  warn(msg: string, ...args: unknown[]): void;
  error(msg: string, ...args: unknown[]): void;
}

/**
 * Returns a named logger for the given concern.
 *
 * Recommended concern names:
 *  - `"ssr"` — server-side rendering (routes, data fetching)
 *  - `"api-proxy"` — the /api/* reverse proxy
 *  - `"sse"` — EventSource / streaming
 */
export function getLogger(name: string): Logger {
  const minLevel = getMinLevel();

  function shouldLog(level: Level): boolean {
    return LEVELS[level] >= LEVELS[minLevel];
  }

  return {
    debug(msg: string, ...args: unknown[]) {
      if (shouldLog("debug")) {
        console.debug(`[DEBUG ${name}]`, msg, ...args);
      }
    },
    info(msg: string, ...args: unknown[]) {
      if (shouldLog("info")) {
        console.info(`[INFO ${name}]`, msg, ...args);
      }
    },
    warn(msg: string, ...args: unknown[]) {
      if (shouldLog("warn")) {
        console.warn(`[WARN ${name}]`, msg, ...args);
      }
    },
    error(msg: string, ...args: unknown[]) {
      if (shouldLog("error")) {
        console.error(`[ERROR ${name}]`, msg, ...args);
      }
    },
  };
}
