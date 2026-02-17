import { createDefine } from "fresh";

export interface State {
  title: string;
}

export const define = createDefine<State>();

export const BACKEND_URL = Deno.env.get("BACKEND_URL") ||
  "http://localhost:8080";
