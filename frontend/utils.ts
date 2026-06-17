import { createDefine } from "fresh";

export { BACKEND_URL } from "./backendUrl.ts";

export interface State {
  title: string;
}

export const define = createDefine<State>();
