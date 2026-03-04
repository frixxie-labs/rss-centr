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

Deno.test("getLogger - debug outputs to console.debug with formatted prefix", () => {
  const debugSpy = spy(console, "debug");
  try {
    const log = getLogger("mymodule");
    log.debug("hello", { key: "value" });
    assertSpyCalls(debugSpy, 1);
    assertEquals(debugSpy.calls[0].args[0], "[DEBUG mymodule]");
    assertEquals(debugSpy.calls[0].args[1], "hello");
    assertEquals(debugSpy.calls[0].args[2], { key: "value" });
  } finally {
    debugSpy.restore();
  }
});

Deno.test("getLogger - info outputs to console.info with formatted prefix", () => {
  const infoSpy = spy(console, "info");
  try {
    const log = getLogger("ssr");
    log.info("fetched items", 42);
    assertSpyCalls(infoSpy, 1);
    assertEquals(infoSpy.calls[0].args[0], "[INFO ssr]");
    assertEquals(infoSpy.calls[0].args[1], "fetched items");
    assertEquals(infoSpy.calls[0].args[2], 42);
  } finally {
    infoSpy.restore();
  }
});

Deno.test("getLogger - warn outputs to console.warn with formatted prefix", () => {
  const warnSpy = spy(console, "warn");
  try {
    const log = getLogger("proxy");
    log.warn("something off");
    assertSpyCalls(warnSpy, 1);
    assertEquals(warnSpy.calls[0].args[0], "[WARN proxy]");
    assertEquals(warnSpy.calls[0].args[1], "something off");
  } finally {
    warnSpy.restore();
  }
});

Deno.test("getLogger - error outputs to console.error with formatted prefix", () => {
  const errorSpy = spy(console, "error");
  try {
    const log = getLogger("api");
    log.error("crash", new Error("boom"));
    assertSpyCalls(errorSpy, 1);
    assertEquals(errorSpy.calls[0].args[0], "[ERROR api]");
    assertEquals(errorSpy.calls[0].args[1], "crash");
  } finally {
    errorSpy.restore();
  }
});

Deno.test("getLogger - different logger names produce different prefixes", () => {
  const debugSpy = spy(console, "debug");
  try {
    const log1 = getLogger("alpha");
    const log2 = getLogger("beta");
    log1.debug("msg1");
    log2.debug("msg2");
    assertEquals(debugSpy.calls[0].args[0], "[DEBUG alpha]");
    assertEquals(debugSpy.calls[1].args[0], "[DEBUG beta]");
  } finally {
    debugSpy.restore();
  }
});
