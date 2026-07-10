import { define } from "@/utils.ts";
import { BACKEND_URL } from "@/backendUrl.ts";
import { healthResponse } from "@/healthResponse.ts";

export const handler = define.handlers({
  GET(_ctx) {
    return healthResponse(BACKEND_URL);
  },
});
