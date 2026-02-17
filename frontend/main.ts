import { App, staticFiles } from "fresh";
import type { State } from "./utils.ts";

export const app = new App<State>();

app.use(staticFiles());

app.use(async (ctx) => {
  ctx.state.title = "RSS Centr";
  return await ctx.next();
});

app.fsRoutes();
