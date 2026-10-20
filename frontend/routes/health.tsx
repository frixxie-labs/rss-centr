// Alias for /status/health for monitors that probe /health.
import { handler as statusHealthHandler } from "./status/health.tsx";

export const handler = statusHealthHandler;
