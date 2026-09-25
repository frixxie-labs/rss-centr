import { Head } from "fresh/runtime";
import { define } from "../utils.ts";
import { Header } from "../components/Header.tsx";
import DailySummary from "../islands/DailySummary.tsx";

export default define.page(function Home() {
  return (
    <div class="min-h-screen flex flex-col">
      <Head>
        <title>RSS Centr - AI Summary</title>
      </Head>
      <Header>
        <a
          href="/"
          class="rounded-md bg-sumi-ink3 px-2 py-1 text-sm text-fuji-white"
        >
          AI Summary
        </a>
        <a
          href="/timeline"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Timeline
        </a>
        <a
          href="/news"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          News
        </a>
        <a
          href="/sources"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Sources
        </a>
        <a
          href="/topics"
          class="rounded-md px-2 py-1 text-sm text-fuji-gray transition hover:bg-sumi-ink3 hover:text-fuji-white"
        >
          Topics
        </a>
      </Header>
      <main class="mx-auto w-full min-w-0 max-w-2xl flex-1">
        <DailySummary />
      </main>
    </div>
  );
});
