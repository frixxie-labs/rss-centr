import { define } from "../utils.ts";

export const handler = define.handlers({
  GET(ctx) {
    const url = new URL(ctx.req.url);
    url.pathname = "/news";
    return Response.redirect(url, 308);
  },
});
