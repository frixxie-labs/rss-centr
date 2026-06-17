function readBackendUrlFromEnv(): string | undefined {
  try {
    return typeof Deno !== "undefined" && "env" in Deno
      ? Deno.env.get("BACKEND_URL")
      : undefined;
  } catch {
    return undefined;
  }
}

const backendFromEnv = readBackendUrlFromEnv();

export const BACKEND_URL = backendFromEnv || "http://localhost:8080";
