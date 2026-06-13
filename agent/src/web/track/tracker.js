(function () {
  "use strict";

  var script = document.currentScript;
  var api = (script && script.getAttribute("data-api")) || "";

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

  function send(kind, extra) {
    var payload = {
      b: beacon,
      e: kind,
      u: location.href,
      t: timezone,
    };
    if (document.referrer) payload.r = document.referrer;
    if (extra) {
      for (var key in extra) {
        if (extra[key] !== undefined) payload[key] = extra[key];
      }
    }
    var body = JSON.stringify(payload);
    var url = endpoint("/track/hit");
    if (kind === "unload" && navigator.sendBeacon) {
      navigator.sendBeacon(url, body);
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
    send("unload", { m: Date.now() - startedAt });
  }
  window.addEventListener("pagehide", unload);
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "hidden") unload();
  });

  // SPA navigations (history API).
  var lastPath = location.pathname;
  function onNavigation() {
    if (location.pathname !== lastPath) {
      lastPath = location.pathname;
      beacon = newBeacon();
      startedAt = Date.now();
      reported = false;
      load();
    }
  }
  window.addEventListener("popstate", onNavigation);

  // Public API for manual custom events.
  window.analytics = {
    event: function (name, data) {
      send("custom", { n: name, d: data });
    },
  };
})();
