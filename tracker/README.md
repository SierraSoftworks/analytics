# Tracking beacon

The privacy-preserving tracking snippet served by the analytics agent at
`/tracker.js`. It is pure, dependency-free JavaScript, built into a **single**
heavily-minified artifact with [esbuild](https://esbuild.github.io) — one build, no
feature-flag variants. Behaviour is toggled at runtime by `data-*` attributes on the
`<script>` tag.

It is modelled on the [medama](https://github.com/medama-io/medama) tracker's
page-view mechanics (the HTTP conditional-request "cache trick" for cookieless daily
uniques, History-API SPA tracking, and the `pagehide`/`visibilitychange` unload
beacon) and adds opt-in, Sentry-style client exception reporting.

## What it collects

- **Page views** — a per-page-view beacon id links the `load` and `unload` events;
  there is no cookie or persistent identifier.
- **Daily uniques** — derived server-side from two cache-trick pings (one per site for
  `q`, one per page for `p`); no client-side storage.
- **Time on page** — sent on `unload` via `navigator.sendBeacon`.
- **Exceptions** (opt-in) — unhandled errors and promise rejections, plus anything
  reported through the public API. Deduplicated and capped per view.

It honours Do-Not-Track and Global Privacy Control (collecting nothing, while still
exposing a no-op API so host pages don't break).

Beacons are delivered **preflight-free**: hits and exceptions are CORS "simple
requests" — posted as `text/plain` via `fetch` with `mode: "no-cors"` (or
`navigator.sendBeacon`), so there is no `OPTIONS` round-trip. The agent parses the
JSON body regardless of content type. The `/track/ping` GET stays a normal CORS
request because the page reads its JSON response.

## Configuration (`<script>` attributes)

| Attribute                          | Effect                                                       |
| ---------------------------------- | ------------------------------------------------------------ |
| `data-api`                         | Collection host (defaults to the script's own origin).       |
| `data-auto-capture-exceptions`     | `"true"` to hook `window` errors and promise rejections.     |
| `data-hash`                        | Treat URL-hash changes as navigations (hash-routed SPAs).    |

## Public API

```js
window.analytics.event("signup", { plan: "pro" });        // custom event
window.analytics.captureException(err, { context: "..." }); // manual exception
```

## Develop

```bash
npm install
npm test          # vitest (jsdom)
npm run build     # -> dist/tracker.js (minified, embedded by the agent)
npm run watch     # rebuild on change
```

`dist/` is git-ignored; CI builds it and the agent embeds it via `include_str!`.
