import { define } from "../utils.ts";

export default define.page(function App({ Component, state }) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <title>{state.title}</title>
      </head>
      <body class="bg-neutral-950 text-neutral-100 min-h-screen">
        <Component />
      </body>
    </html>
  );
});
