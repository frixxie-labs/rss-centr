import { assertEquals } from "@std/assert";
import { assertSpyCalls, spy } from "@std/testing/mock";
import { getLogger } from "./logger.ts";

Deno.test("getLogger - returns a Logger with debug, info, warn, error methods", () => {
  const log = getLogger("test");
  assertEquals(typeof log.debug, "function");
  assertEquals(typeof log.info, "function");
  assertEquals(typeof log.warn, "function");
  assertEquals(typeof log.error, "function");
});

Deno.test("getLogger - debug outputs structured JSON to console.debug", () => {
  const debugSpy = spy(console, "debug");
  try {
    const log = getLogger("mymodule");
    log.debug("hello", { key: "value" });
    assertSpyCalls(debugSpy, 1);
    const entry = JSON.parse(debugSpy.calls[0].args[0]);
    assertEquals(entry.level, "DEBUG");
    assertEquals(entry.target, "mymodule");
    assertEquals(entry.message, "hello");
    assertEquals(entry.key, "value");
    assertEquals(typeof entry.timestamp, "string");
    assertEquals(debugSpy.calls[0].args.length, 1);
  } finally {
    debugSpy.restore();
  }
});

Deno.test("getLogger - info preserves additional scalar details", () => {
  const infoSpy = spy(console, "info");
  try {
    const log = getLogger("ssr");
    log.info("fetched news", 42);
    assertSpyCalls(infoSpy, 1);
    const entry = JSON.parse(infoSpy.calls[0].args[0]);
    assertEquals(entry.level, "INFO");
    assertEquals(entry.target, "ssr");
    assertEquals(entry.message, "fetched news");
    assertEquals(entry.details, [42]);
  } finally {
    infoSpy.restore();
  }
});

Deno.test("getLogger - warn outputs to console.warn", () => {
  const warnSpy = spy(console, "warn");
  try {
    const log = getLogger("proxy");
    log.warn("something off");
    assertSpyCalls(warnSpy, 1);
    const entry = JSON.parse(warnSpy.calls[0].args[0]);
    assertEquals(entry.level, "WARN");
    assertEquals(entry.target, "proxy");
    assertEquals(entry.message, "something off");
  } finally {
    warnSpy.restore();
  }
});

Deno.test("getLogger - error serializes error details", () => {
  const errorSpy = spy(console, "error");
  try {
    const log = getLogger("api");
    log.error("crash", new Error("boom"));
    assertSpyCalls(errorSpy, 1);
    const entry = JSON.parse(errorSpy.calls[0].args[0]);
    assertEquals(entry.level, "ERROR");
    assertEquals(entry.target, "api");
    assertEquals(entry.message, "crash");
    assertEquals(entry.error.name, "Error");
    assertEquals(entry.error.message, "boom");
    assertEquals(typeof entry.error.stack, "string");
  } finally {
    errorSpy.restore();
  }
});

Deno.test("getLogger - different logger names produce different targets", () => {
  const debugSpy = spy(console, "debug");
  try {
    const log1 = getLogger("alpha");
    const log2 = getLogger("beta");
    log1.debug("msg1");
    log2.debug("msg2");
    assertEquals(JSON.parse(debugSpy.calls[0].args[0]).target, "alpha");
    assertEquals(JSON.parse(debugSpy.calls[1].args[0]).target, "beta");
  } finally {
    debugSpy.restore();
  }
});

Deno.test("getLogger - serializes nested errors and circular values", () => {
  const warnSpy = spy(console, "warn");
  try {
    const circular: Record<string, unknown> = {};
    circular.self = circular;
    const log = getLogger("test");
    log.warn("context", { err: new Error("nested"), circular });

    const entry = JSON.parse(warnSpy.calls[0].args[0]);
    assertEquals(entry.err.message, "nested");
    assertEquals(entry.circular.self, "[Circular]");
  } finally {
    warnSpy.restore();
  }
});
