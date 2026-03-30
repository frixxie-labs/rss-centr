import { define } from "@/utils.ts";

export const handler = define.handlers({
  GET(_ctx) {
    return new Response(
      JSON.stringify({
        status: "ok",
        timestamp: new Date().toISOString(),
      }),
      {
        headers: { "Content-Type": "application/json" },
      },
    );
  },
});
