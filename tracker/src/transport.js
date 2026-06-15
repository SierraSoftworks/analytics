// Network transport for the beacon. All payloads are JSON: the agent parses
// `application/json` bodies, and the `/track/*` scope is permissively CORS-enabled
// server-side, so cross-origin beacons work without credentials.

/** Join an API base with an absolute path, trimming trailing slashes off the base. */
export function joinUrl(api, path) {
  if (!api) return path;
  return api.replace(/\/+$/, "") + path;
}

// Coerce an arbitrary object into the server's metadata shape: a flat map of
// string→string. Non-string values are stringified; null/undefined are dropped.
// Returns undefined when there is nothing to send so the key is omitted entirely.
export function stringifyMeta(data) {
  if (!data || typeof data !== "object") return undefined;
  var out = {};
  var count = 0;
  for (var key in data) {
    if (!Object.prototype.hasOwnProperty.call(data, key)) continue;
    var value = data[key];
    if (value === undefined || value === null) continue;
    out[key] = typeof value === "string" ? value : String(value);
    count++;
  }
  return count ? out : undefined;
}

// Build a transport bound to an API base. `post` delivers via `navigator.sendBeacon`
// when asked (so it survives page unload), otherwise `fetch` with `keepalive`. `get`
// is used for the cache-trick ping and must go through the HTTP cache, so it sets no
// special headers and omits credentials.
export function createTransport(api, env) {
  env = env || {};
  var fetchImpl =
    env.fetch || (typeof fetch !== "undefined" ? fetch.bind(globalThis) : null);
  var nav =
    env.navigator || (typeof navigator !== "undefined" ? navigator : undefined);

  function post(path, payload, useBeacon) {
    var url = joinUrl(api, path);
    var body = JSON.stringify(payload);
    if (useBeacon && nav && typeof nav.sendBeacon === "function") {
      try {
        // `text/plain` is a CORS-safelisted content type, so the beacon is delivered
        // without a preflight. The agent parses the JSON body regardless of type.
        var blob = new Blob([body], { type: "text/plain" });
        if (nav.sendBeacon(url, blob)) return;
      } catch (e) {
        // Fall through to fetch.
      }
    }
    if (!fetchImpl) return;
    fetchImpl(url, {
      method: "POST",
      body: body,
      // `text/plain` + `no-cors` keeps this a CORS "simple request": no preflight. We
      // don't read the response, so an opaque (no-cors) result is fine.
      headers: { "Content-Type": "text/plain" },
      keepalive: true,
      credentials: "omit",
      mode: "no-cors",
    }).catch(function () {});
  }

  function get(path) {
    if (!fetchImpl) return Promise.reject(new Error("no fetch"));
    return fetchImpl(joinUrl(api, path), { credentials: "omit", mode: "cors" });
  }

  return { post: post, get: get };
}
