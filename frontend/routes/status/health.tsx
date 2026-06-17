import { define } from "@/utils.ts";
import { BACKEND_URL } from "@/backendUrl.ts";

export const handler = define.handlers({
  async GET(_ctx) {
    const uptime = Math.floor(performance.now() / 1000);

    let backendStatus: "ok" | "unreachable" = "unreachable";
    try {
      const res = await fetch(`${BACKEND_URL}/api/status/ping`, {
        signal: AbortSignal.timeout(3000),
      });
      if (res.ok) {
        backendStatus = "ok";
      }
    } catch {
      // backend unreachable
    }

    const status = backendStatus === "ok" ? "ok" : "degraded";
    const httpStatus = backendStatus === "ok" ? 200 : 503;

    return new Response(
      JSON.stringify({
        status,
        uptime,
        backend: backendStatus,
        timestamp: new Date().toISOString(),
      }),
      {
        status: httpStatus,
        headers: { "Content-Type": "application/json" },
      },
    );
  },
});
