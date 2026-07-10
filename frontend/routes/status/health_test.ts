import { assertEquals } from "@std/assert";
import { healthResponse } from "../../healthResponse.ts";

Deno.test("health checks the backend ping endpoint", async () => {
  let requestedUrl = "";
  const response = await healthResponse(
    "http://backend:8080",
    (input) => {
      requestedUrl = input;
      return Promise.resolve(new Response(null, { status: 200 }));
    },
  );

  assertEquals(requestedUrl, "http://backend:8080/status/ping");
  assertEquals(response.status, 200);
});
