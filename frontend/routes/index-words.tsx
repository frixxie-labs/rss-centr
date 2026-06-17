import { define } from "../utils.ts";

export const handler = define.handlers({
  GET(ctx) {
    const url = new URL(ctx.req.url);
    url.pathname = "/topics";
    return Response.redirect(url, 308);
  },
});
