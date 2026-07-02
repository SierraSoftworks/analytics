// The privacy-preserving tracking beacon.
//
// Reports page views, time-on-page and (opt-in) client exceptions to an analytics
// agent. No cookies, no persistent identifiers: a fresh per-page-view id links a
// view's load/unload beacons, and daily-unique counts are derived server-side from
// the HTTP conditional-request cache trick rather than any stored id.
//
// Configured declaratively on the <script> tag:
//   data-api="https://analytics.example.com"   collection host (default: same origin)
//   data-auto-capture-exceptions="true"          hook window errors + rejections
//   data-hash                                    treat #hash changes as navigations
//   data-app-version="1.4.2"                     attribute exceptions to a release
//                                                (the app itself is the hostname)
//
// One build, no variants; behaviour is toggled by the attributes above at runtime.

import { privacySignal } from "./privacy.js";
import { createTransport, stringifyMeta } from "./transport.js";
import { createExceptionReporter } from "./exceptions.js";

function attr(el, name) {
  return el && el.getAttribute ? el.getAttribute(name) : null;
}

// A per-page-view id: base36 timestamp + random suffix. Not collision-proof and not
// meant to be — it only needs to link one view's beacons within one browser.
function newBeacon() {
  return Date.now().toString(36) + Math.random().toString(36).slice(2);
}

// Initialise the tracker against the ambient browser (or the injected `overrides`,
// used by the tests). Returns the public `analytics` API object, or undefined in a
// non-browser environment.
export function init(overrides) {
  overrides = overrides || {};
  const doc =
    overrides.document || (typeof document !== "undefined" ? document : undefined);
  const win =
    overrides.window || (typeof window !== "undefined" ? window : undefined);
  const nav =
    overrides.navigator || (typeof navigator !== "undefined" ? navigator : undefined);
  if (!doc || !win) return undefined;

  const script = overrides.script || doc.currentScript;
  const api = overrides.api != null ? overrides.api : attr(script, "data-api") || "";
  const captureExceptions =
    overrides.captureExceptions != null
      ? overrides.captureExceptions
      : attr(script, "data-auto-capture-exceptions") === "true";
  const hashMode =
    overrides.hashMode != null
      ? overrides.hashMode
      : !!(script && script.hasAttribute && script.hasAttribute("data-hash"));
  const appVersion =
    overrides.appVersion != null
      ? overrides.appVersion
      : attr(script, "data-app-version") || "";

  // Honour Do-Not-Track / Global Privacy Control: collect nothing, but still expose a
  // no-op API so sites that call `analytics.event(...)` don't throw.
  if (privacySignal(nav, win)) {
    win.analytics = win.analytics || { event: noop, captureException: noop };
    return win.analytics;
  }

  const loc = win.location;
  const transport = createTransport(api, { fetch: overrides.fetch, navigator: nav });

  let beacon = newBeacon();
  let startedAt = now();
  // The URL of the view currently being measured. Captured at view start so the
  // unload beacon attributes its duration to the right page even after a popstate has
  // already changed `location`.
  let viewUrl = loc.href;
  let unloaded = false;

  let timezone = "";
  try {
    timezone = Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  } catch (e) {
    /* Intl unavailable */
  }

  // Send a hit. `url` defaults to the live location (correct for load/custom); the
  // unload path passes the captured view URL instead.
  function send(kind, extra, useBeacon, url) {
    const payload = { b: beacon, e: kind, u: url || loc.href };
    if (timezone) payload.t = timezone;
    if (doc.referrer) payload.r = doc.referrer;
    if (extra) {
      for (const key in extra) {
        if (extra[key] !== undefined) payload[key] = extra[key];
      }
    }
    transport.post("/track/hit", payload, useBeacon);
  }

  // Daily-unique oracle via the cache trick. The agent applies the
  // If-Modified-Since/UTC-midnight logic regardless of the query string, so two
  // distinct query strings give us independent per-site and per-page signals from one
  // endpoint, with no server change.
  function ping(query) {
    return transport
      .get("/track/ping?" + query)
      .then(function (response) {
        return response.text();
      })
      .then(function (text) {
        // "1" = first visit today, "0" = returning.
        return text === "1";
      })
      .catch(function () {
        return false;
      });
  }

  function load() {
    const host = encodeURIComponent(loc.hostname);
    const path = encodeURIComponent(loc.pathname);
    Promise.all([ping("h=" + host), ping("h=" + host + "&p=" + path)]).then(function (
      flags,
    ) {
      // q = first visit to the site today; p = first view of this page today.
      send("load", { q: flags[0], p: flags[1] });
    });
  }

  function unload() {
    if (unloaded) return;
    unloaded = true;
    send("unload", { m: now() - startedAt }, true, viewUrl);
  }

  // Begin measuring a new page view (initial load and each SPA navigation).
  function startView() {
    beacon = newBeacon();
    startedAt = now();
    viewUrl = loc.href;
    unloaded = false;
    load();
  }

  // --- Lifecycle ----------------------------------------------------------------

  if (doc.readyState === "complete" || doc.readyState === "interactive") {
    load();
  } else {
    doc.addEventListener("DOMContentLoaded", load);
  }

  // Prefer pagehide (bfcache-friendly); fall back to beforeunload+unload on browsers
  // without it. visibilitychange→hidden also flushes, to catch tab switches/closes.
  if ("onpagehide" in win) {
    win.addEventListener("pagehide", unload, { capture: true });
  } else {
    win.addEventListener("beforeunload", unload, { capture: true });
    win.addEventListener("unload", unload, { capture: true });
  }
  doc.addEventListener(
    "visibilitychange",
    function () {
      if (doc.visibilityState === "hidden") unload();
    },
    { capture: true },
  );

  // --- SPA navigation -----------------------------------------------------------

  if (hashMode) {
    win.addEventListener(
      "hashchange",
      function () {
        unload();
        startView();
      },
      { capture: true },
    );
  } else {
    const history = win.history;
    const wrap = function (original) {
      return function (state, title, url) {
        // Only treat it as a navigation when the path actually changes; same-path
        // replaceState (query/hash churn) shouldn't emit a fresh page view.
        const changed =
          url != null && new URL(url, loc.href).pathname !== loc.pathname;
        if (changed) unload();
        const result = original.apply(this, arguments);
        if (changed) startView();
        return result;
      };
    };
    if (history && history.pushState) history.pushState = wrap(history.pushState);
    if (history && history.replaceState) {
      history.replaceState = wrap(history.replaceState);
    }
    win.addEventListener(
      "popstate",
      function () {
        // location has already changed here; viewUrl still holds the old page.
        unload();
        startView();
      },
      { capture: true },
    );
  }

  // --- Exceptions ---------------------------------------------------------------

  const reporter = createExceptionReporter({
    send: function (payload) {
      transport.post("/track/exception", payload, true);
    },
    url: function () {
      return loc.href;
    },
    beacon: function () {
      return beacon;
    },
    appVersion: appVersion || undefined,
  });

  if (captureExceptions) {
    win.addEventListener("error", function (event) {
      reporter.report(
        event.error || { name: "Error", message: event.message },
        false,
      );
    });
    win.addEventListener("unhandledrejection", function (event) {
      reporter.report(event.reason, false, undefined, "UnhandledRejection");
    });
  }

  // --- Public API ---------------------------------------------------------------

  const analytics = {
    // Record a custom event with an optional string→string metadata map.
    event: function (name, data) {
      send("custom", { n: name, d: stringifyMeta(data) });
    },
    // Manually report a handled exception with optional metadata.
    captureException: function (error, meta) {
      reporter.report(error, true, stringifyMeta(meta));
    },
  };
  win.analytics = analytics;
  return analytics;
}

function now() {
  return Date.now();
}

function noop() {}
