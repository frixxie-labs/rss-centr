/**
 * Structured logger that emits one JSON object per line for VictoriaLogs.
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null &&
    !Array.isArray(value) && !(value instanceof Error);
}

function stringify(entry: Record<string, unknown>): string {
  const seen = new WeakSet<object>();

  return JSON.stringify(entry, (_key, value: unknown) => {
    if (value instanceof Error) {
      return {
        name: value.name,
        message: value.message,
        stack: value.stack,
      };
    }
    if (typeof value === "bigint") return value.toString();
    if (typeof value === "object" && value !== null) {
      if (seen.has(value)) return "[Circular]";
      seen.add(value);
    }
    return value;
  });
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

  function write(level: Level, message: string, args: unknown[]): void {
    const fields: Record<string, unknown> = {};
    const details: unknown[] = [];
    let error: Error | undefined;

    for (const arg of args) {
      if (arg instanceof Error && error === undefined) {
        error = arg;
      } else if (isRecord(arg)) {
        Object.assign(fields, arg);
      } else {
        details.push(arg);
      }
    }

    const entry: Record<string, unknown> = {
      ...fields,
      timestamp: new Date().toISOString(),
      level: level.toUpperCase(),
      target: name,
      message,
    };
    if (error !== undefined) entry.error = error;
    if (details.length > 0) entry.details = details;

    console[level](stringify(entry));
  }

  return {
    debug(msg: string, ...args: unknown[]) {
      if (shouldLog("debug")) {
        write("debug", msg, args);
      }
    },
    info(msg: string, ...args: unknown[]) {
      if (shouldLog("info")) {
        write("info", msg, args);
      }
    },
    warn(msg: string, ...args: unknown[]) {
      if (shouldLog("warn")) {
        write("warn", msg, args);
      }
    },
    error(msg: string, ...args: unknown[]) {
      if (shouldLog("error")) {
        write("error", msg, args);
      }
    },
  };
}
