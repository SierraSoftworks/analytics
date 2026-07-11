# Analytics

**Lightweight, privacy-preserving analytics for your websites and applications.**

A self-hosted analytics service written in Rust. It collects useful product
analytics without compromising your users' privacy — no cookies, no IP addresses,
no personally identifiable information. Multiple sources (a marketing site, its
docs, a paired application) can be grouped into a **project** and viewed in
aggregate, alongside a global overview across every project.

It also doubles as a lightweight, privacy-preserving **error tracker**, grouping
client-side exceptions much like Sentry.

## Privacy model

Inspired by [medama](https://oss.medama.io/methodology/overview), the service
deliberately collects only broad, non-identifying signals:

- **No cookies, no IP storage, no PII.** Client IPs are used transiently only as
  rate-limit keys and are never logged or persisted.
- **Daily unique visitors** are counted with the HTTP conditional-request cache
  trick (`If-Modified-Since` vs UTC midnight), so uniqueness resets every day
  without any client-side identifier.
- The **User-Agent** and **Accept-Language** headers are parsed into broad classes
  (app / version / OS family / device kind, primary language) at the edge — the
  raw values are never stored. Browsers and pure application clients are both
  recognized; bots are dropped.
- **Country** is derived from the browser's reported timezone, not IP geolocation.
- `DNT` / `Sec-GPC` signals are honored.

## Features

- **Projects & sources** — group multiple hostnames (and applications) into a
  project; filter to subsets; auto-register new reporting hostnames.
- **Metrics** — visitors, page views, bounce rate, median time on page, time
  series, and breakdowns by page, referrer, application (browser or client
  app) and its version, OS, device, country, language, and source.
- **Session traces** — the dashboard samples the most recent visits matching the
  active filters, and each opens a timeline of the pages, custom events, and
  exceptions that visit reported. The linking id is tab-scoped (`sessionStorage`,
  never a cookie): navigations within a tab share it, the browser clears it when
  the tab closes, and separate tabs and return visits stay uncorrelatable.
- **Tracking pixels** — admin-created, project-bound tracking GIFs (e.g. for email
  opens) with attached metadata. Unknown pixel ids are rejected — there is no open
  pixel endpoint.
- **Exception tracking** — capture unhandled errors and rejections, grouped by a
  Sentry-style fingerprint, with triage state (unresolved / resolved / ignored).
- **OIDC authentication** — the dashboard and management API are gated by a
  server-driven OIDC flow with a configurable
  [filter-expression](https://github.com/SierraSoftworks/filters) ACL. The public
  tracking endpoints need no authentication.
- **Rate limiting** — per-IP token-bucket limits on both the public tracking
  endpoints and unauthenticated hits to protected endpoints.
- **Append-only, write-optimized storage** — events are appended to an
  [redb](https://github.com/cberner/redb) hot store, compacted into date-partitioned
  Parquet, and queried with [polars](https://pola.rs).

## Architecture

A Cargo workspace, mirroring [grey](https://github.com/SierraSoftworks/grey) and
[automate](https://github.com/SierraSoftworks/automate):

- **`api/`** — framework-free serde DTOs shared by the server and the WebAssembly
  frontend.
- **`agent/`** — the `actix-web` server: clap CLI, YAML config, OIDC auth, the
  ingest pipeline, storage, and the polars query layer. The compiled frontend is
  embedded into the binary via `include_dir!`.
- **`ui/`** — a client-side-rendered [Yew](https://yew.rs) dashboard, built with
  [Trunk](https://trunkrs.dev).
- **`tracker/`** — the tracking beacon: a dependency-free, pure-JavaScript snippet
  built into a single heavily-minified artifact with
  [esbuild](https://esbuild.github.io) and served at `/tracker.js`. One build, no
  variants — behaviour is toggled by `data-*` attributes at runtime. Unit-tested with
  [Vitest](https://vitest.dev).

## Quick start

```bash
# 1. Build the tracking beacon (embedded into the server binary).
cd tracker && npm install && npm run build && cd ..

# 2. Build the frontend bundle (embedded into the server binary).
cd ui && trunk build --release && cd ..

# 3. Build and run the server.
cargo build --release -p analytics
cp config.example.yaml config.yaml   # then edit to taste
./target/release/analytics --config config.yaml
```

The dashboard is served at the configured address (default `http://127.0.0.1:8080`).

### Tracking a website

Add the tracker script to your pages. It defaults to reporting back to the origin
it was served from, so pointing `src` at your server is enough — set `data-api`
explicitly only to override that host:

```html
<script
  async
  src="https://analytics.example.com/tracker.js"
  data-auto-capture-exceptions="true"
  data-app-version="1.4.2"
></script>
```

The script reports page views (and, with `data-auto-capture-exceptions`, unhandled
errors and promise rejections). Exceptions are attributed to the reporting
hostname — the application — and `data-app-version` additionally pins them to a
specific release, so the dashboard can break failures down by version. It follows
SPA navigations automatically by intercepting the History API; add `data-hash` if
your app routes with the URL hash instead. It also exposes
`window.analytics.event(name, data)` and
`window.analytics.captureException(error, meta)` for manual reporting — `meta` is
a string→string map stored with the report and surfaced on the exception's
distinct examples. Sources are identified purely by their hostname — no per-site
key to embed.

### Tracking pixels

Create a pixel in the dashboard (under a project) to get an embeddable URL such as
`https://analytics.example.com/track/gif/<id>.gif` for contexts where JavaScript
can't run (email opens, RSS, docs).

## Configuration

All configuration lives in a YAML file (see
[`config.example.yaml`](config.example.yaml)). Secrets can be injected from the
environment with `${{ env.VAR_NAME }}` placeholders.

Omitting the `web.admin.oidc` block disables the sign-in flow, but the dashboard is
still gated by the `web.admin.acl` filter expression — which **defaults to `"false"`
(deny all)**. To run locally without authentication, omit OIDC *and* set an
allow-all ACL so the API is reachable:

```yaml
web:
  admin:
    acl: "true"   # local development only — grants everyone full access
```

With the default deny-all ACL and no OIDC, the dashboard cannot be signed into (the
sign-in page explains this rather than looping).

## API

- **Public (no auth):** `GET /tracker.js`, `GET /track/ping`, `POST /track/hit`,
  `POST /track/exception`, `GET /track/gif/{id}.gif`, `GET /api/v1/health`.
- **Protected (OIDC + ACL):** everything else under `/api/v1` — projects, sources,
  pixels, the filterable dashboard statistics (`GET /api/v1/stats`), and exception
  groups/triage. Statistics and exception listings accept a `q` parameter carrying
  a [filt-rs](https://github.com/SierraSoftworks/filters) expression, e.g.
  `q=browser == "Chrome" && (country == "DE" || path like "/docs/*")` — the same
  syntax the dashboard's query bar uses.

## License

MIT — see [LICENSE](LICENSE).
