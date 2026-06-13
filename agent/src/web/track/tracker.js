(function () {
  "use strict";

  var script = document.currentScript;
  var api = (script && script.getAttribute("data-api")) || "";
  var captureExceptions =
    script && script.getAttribute("data-auto-capture-exceptions") === "true";

  // Respect Do-Not-Track / Global Privacy Control.
  if (
    navigator.doNotTrack === "1" ||
    window.doNotTrack === "1" ||
    navigator.globalPrivacyControl
  ) {
    return;
  }

  function endpoint(path) {
    return (api ? api.replace(/\/$/, "") : "") + path;
  }

  function newBeacon() {
    return Date.now().toString(36) + Math.random().toString(36).slice(2);
  }

  var beacon = newBeacon();
  var startedAt = Date.now();
  var timezone = "";
  try {
    timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  } catch (e) {}

  // POST JSON, using sendBeacon (with a JSON Blob so the server sees the right
  // content type) when firing during unload, else fetch with keepalive.
  function post(path, payload, useBeacon) {
    var body = JSON.stringify(payload);
    var url = endpoint(path);
    if (useBeacon && navigator.sendBeacon) {
      navigator.sendBeacon(url, new Blob([body], { type: "application/json" }));
    } else {
      fetch(url, {
        method: "POST",
        body: body,
        headers: { "Content-Type": "application/json" },
        keepalive: true,
        credentials: "omit",
      }).catch(function () {});
    }
  }

  function send(kind, extra, useBeacon) {
    var payload = { b: beacon, e: kind, u: location.href, t: timezone };
    if (document.referrer) payload.r = document.referrer;
    if (extra) {
      for (var key in extra) {
        if (extra[key] !== undefined) payload[key] = extra[key];
      }
    }
    post("/track/hit", payload, useBeacon);
  }

  function sendException(error, handled, meta) {
    var name = (error && error.name) || "Error";
    var message = (error && error.message) || String(error);
    var payload = { u: location.href, b: beacon, ty: name, m: message, h: !!handled };
    if (error && error.stack) payload.s = String(error.stack);
    if (meta) payload.d = meta;
    post("/track/exception", payload, true);
  }

  function load() {
    fetch(endpoint("/track/ping?h=" + encodeURIComponent(location.hostname)), {
      credentials: "omit",
    })
      .then(function (response) {
        return response.json();
      })
      .then(function (result) {
        var unique = !!(result && result.unique);
        send("load", { q: unique, p: unique });
      })
      .catch(function () {
        send("load", { q: false, p: false });
      });
  }

  if (document.readyState === "complete" || document.readyState === "interactive") {
    load();
  } else {
    document.addEventListener("DOMContentLoaded", load);
  }

  // Time-on-page when the page is hidden or unloaded.
  var reported = false;
  function unload() {
    if (reported) return;
    reported = true;
    send("unload", { m: Date.now() - startedAt }, true);
  }
  window.addEventListener("pagehide", unload);
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "hidden") unload();
  });

  // SPA navigations (history API).
  var lastPath = location.pathname;
  window.addEventListener("popstate", function () {
    if (location.pathname !== lastPath) {
      lastPath = location.pathname;
      beacon = newBeacon();
      startedAt = Date.now();
      reported = false;
      load();
    }
  });

  // Optional automatic capture of unhandled errors and promise rejections.
  if (captureExceptions) {
    window.addEventListener("error", function (event) {
      sendException(event.error || { name: "Error", message: event.message }, false);
    });
    window.addEventListener("unhandledrejection", function (event) {
      var reason = event.reason;
      sendException(
        reason instanceof Error ? reason : { name: "UnhandledRejection", message: String(reason) },
        false
      );
    });
  }

  // Public API for manual events and exception reporting.
  window.analytics = {
    event: function (name, data) {
      send("custom", { n: name, d: data });
    },
    captureException: function (error, meta) {
      sendException(error, true, meta);
    },
  };
})();
