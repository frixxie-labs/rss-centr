import { App, staticFiles } from "fresh";
import { getLogger } from "./logger.ts";
import type { State } from "./utils.ts";

export const app = new App<State>();
const log = getLogger("http");

app.use(async (ctx) => {
  const url = new URL(ctx.req.url);
  const started = performance.now();

  try {
    const res = await ctx.next();
    log.info("Request completed", {
      method: ctx.req.method,
      path: url.pathname,
      status: res.status,
      duration_ms: Number((performance.now() - started).toFixed(1)),
    });
    return res;
  } catch (err) {
    log.error("Request failed", {
      method: ctx.req.method,
      path: url.pathname,
      duration_ms: Number((performance.now() - started).toFixed(1)),
    }, err);
    throw err;
  }
});

app.use(staticFiles());

app.use(async (ctx) => {
  ctx.state.title = "RSS Centr";
  return await ctx.next();
});

app.fsRoutes();
