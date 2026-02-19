import { createDefine } from "fresh";

export interface State {
  title: string;
}

export const define = createDefine<State>();

const backendFromEnv = typeof Deno !== "undefined" && "env" in Deno
  ? Deno.env.get("BACKEND_URL")
  : undefined;

export const BACKEND_URL = backendFromEnv || "http://localhost:8080";
