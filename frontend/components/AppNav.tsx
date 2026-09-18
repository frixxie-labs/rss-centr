import { ANALYTICS_ENABLED } from "../analyticsConfig.ts";

interface AppNavProps {
  currentPath: "/" | "/news" | "/sources" | "/topics" | "/analytics";
}

function linkClass(isActive: boolean): string {
  return isActive
    ? "rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white"
    : "rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white";
}

export function AppNav({ currentPath }: AppNavProps) {
  return (
    <>
      <a href="/" class={linkClass(currentPath === "/")}>Timeline</a>
      <a href="/news" class={linkClass(currentPath === "/news")}>News</a>
      <a href="/sources" class={linkClass(currentPath === "/sources")}>
        Sources
      </a>
      <a href="/topics" class={linkClass(currentPath === "/topics")}>Topics</a>
      {ANALYTICS_ENABLED && (
        <a href="/analytics" class={linkClass(currentPath === "/analytics")}>
          Analytics
        </a>
      )}
    </>
  );
}
