# RSS Centr Frontend

Fresh 2 frontend for RSS Centr.

Primary pages:

- `/` - live timeline
- `/news` - searchable news list
- `/sources` - RSS source management
- `/topics` - recent title topics

### Usage

Make sure to install Deno:
https://docs.deno.com/runtime/getting_started/installation

Then start the project in development mode:

```
deno task dev
```

This will watch the project directory and restart as necessary.

### Optional analytics

The frontend can include a privacy-focused analytics script when configured with
environment variables. This is disabled by default.

- `ANALYTICS_PROVIDER=plausible` with `ANALYTICS_DOMAIN=your-domain`
- `ANALYTICS_PROVIDER=umami` with `ANALYTICS_WEBSITE_ID=your-site-id`
- `ANALYTICS_SCRIPT_SRC=...` to override the default hosted script URL for
  self-hosting with either an `https://...` URL or a root-relative path such as
  `/js/script.js`

Because the script is injected into the shared app shell, page views are tracked
across the main RSS Centr pages without changing backend behavior.
