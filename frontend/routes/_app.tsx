import { define } from "../utils.ts";

export default define.page(function App({ Component, state }) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
        <link rel="alternate icon" href="/favicon.ico" />
        <title>{state.title}</title>
      </head>
      <body class="bg-sumi-ink1 text-fuji-white min-h-screen">
        <Component />
      </body>
    </html>
  );
});
