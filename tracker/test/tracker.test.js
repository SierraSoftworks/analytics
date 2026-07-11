import { describe, it, expect, vi, beforeEach } from "vitest";
import { init } from "../src/tracker.js";

// The native history methods, captured before any init() patches them.
const origPush = window.history.pushState;
const origReplace = window.history.replaceState;

// Flush microtasks + the 0ms timer so the ping→json→hit promise chain settles.
function tick() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

// A fetch double: GET (no method) answers the ping oracle; POST is a 204 sink.
function makeFetch(unique = true) {
  return vi.fn((url, opts) => {
    if (!opts || !opts.method) {
      return Promise.resolve({ ok: true, text: () => Promise.resolve(unique ? "1" : "0") });
    }
    return Promise.resolve({ ok: true, status: 204 });
  });
}

function postBodies(fetchMock, path) {
  return fetchMock.mock.calls
    .filter(([url, opts]) => opts && opts.method === "POST" && url.includes(path))
    .map(([, opts]) => JSON.parse(opts.body));
}

function getUrls(fetchMock, path) {
  return fetchMock.mock.calls
    .filter(([url, opts]) => (!opts || !opts.method) && url.includes(path))
    .map(([url]) => url);
}

// jsdom's Blob doesn't implement .text(); fall back to FileReader to read the body.
function blobText(blob) {
  if (typeof blob.text === "function") return blob.text();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(blob);
  });
}

async function beaconBodies(sendBeacon, path) {
  const bodies = [];
  for (const [url, blob] of sendBeacon.mock.calls) {
    if (url.includes(path)) bodies.push(JSON.parse(await blobText(blob)));
  }
  return bodies;
}

// Fire whichever unload event the tracker actually wired up in this environment.
function fireUnload() {
  if ("onpagehide" in window) window.dispatchEvent(new Event("pagehide"));
  else window.dispatchEvent(new Event("beforeunload"));
}

// A window "error" event without relying on the ErrorEvent constructor.
function fireError(error, message) {
  const ev = new Event("error");
  ev.error = error;
  ev.message = message;
  window.dispatchEvent(ev);
}

let fetchMock;
let navMock;

beforeEach(() => {
  // Un-patch history (a previous init wrapped it) and reset to the root path.
  window.history.pushState = origPush;
  window.history.replaceState = origReplace;
  window.history.replaceState({}, "", "/");
  delete window.analytics;
  // Each test starts a fresh "tab": no session id carried over.
  window.sessionStorage.clear();

  fetchMock = makeFetch(true);
  navMock = { doNotTrack: null, sendBeacon: vi.fn(() => true) };
});

describe("init — privacy", () => {
  it("collects nothing under Do-Not-Track but still exposes a no-op API", async () => {
    const api = init({ fetch: fetchMock, navigator: { doNotTrack: "1" } });
    await tick();

    expect(fetchMock).not.toHaveBeenCalled();
    // Not even the tab-scoped session id is written under DNT.
    expect(window.sessionStorage.getItem("analytics-session")).toBeNull();
    expect(typeof api.event).toBe("function");
    expect(() => api.event("x")).not.toThrow();
    expect(() => api.captureException(new Error("x"))).not.toThrow();
  });
});

describe("init — page load", () => {
  it("pings per-site and per-page, then posts a load hit", async () => {
    init({ fetch: fetchMock, navigator: navMock });
    await tick();

    const pings = getUrls(fetchMock, "/track/ping");
    expect(pings).toHaveLength(2);
    expect(pings[0]).toContain("h=");
    expect(pings.some((u) => u.includes("&p="))).toBe(true);

    const loads = postBodies(fetchMock, "/track/hit");
    expect(loads).toHaveLength(1);
    expect(loads[0]).toMatchObject({ e: "load", q: true, p: true });
    expect(loads[0].u).toContain("example.test");
    expect(typeof loads[0].b).toBe("string");
    expect(typeof loads[0].i).toBe("string");
  });

  it("reports a non-unique visit when the oracle says so", async () => {
    const fetch = makeFetch(false);
    init({ fetch, navigator: navMock });
    await tick();

    expect(postBodies(fetch, "/track/hit")[0]).toMatchObject({ q: false, p: false });
  });
});

describe("init — unload", () => {
  it("sends a duration beacon via sendBeacon", async () => {
    init({ fetch: fetchMock, navigator: navMock });
    await tick();

    fireUnload();

    const bodies = await beaconBodies(navMock.sendBeacon, "/track/hit");
    expect(bodies).toHaveLength(1);
    expect(bodies[0]).toMatchObject({ e: "unload" });
    expect(typeof bodies[0].m).toBe("number");
  });

  it("sends the unload beacon at most once", async () => {
    init({ fetch: fetchMock, navigator: navMock });
    await tick();

    fireUnload();
    fireUnload();

    expect(await beaconBodies(navMock.sendBeacon, "/track/hit")).toHaveLength(1);
  });
});

describe("init — SPA navigation", () => {
  it("unloads the old path and loads the new one on pushState", async () => {
    init({ fetch: fetchMock, navigator: navMock });
    await tick();
    fetchMock.mockClear();
    navMock.sendBeacon.mockClear();

    window.history.pushState({}, "", "/next");
    await tick();

    const unloads = await beaconBodies(navMock.sendBeacon, "/track/hit");
    expect(unloads).toHaveLength(1);
    expect(unloads[0]).toMatchObject({ e: "unload" });
    expect(unloads[0].u).not.toContain("/next"); // attributed to the previous view

    const loads = postBodies(fetchMock, "/track/hit");
    expect(loads).toHaveLength(1);
    expect(loads[0]).toMatchObject({ e: "load" });
    expect(loads[0].u).toContain("/next");
  });

  it("keeps the session id across navigations but rotates the beacon id", async () => {
    init({ fetch: fetchMock, navigator: navMock });
    await tick();
    const first = postBodies(fetchMock, "/track/hit")[0];
    fetchMock.mockClear();

    window.history.pushState({}, "", "/next");
    await tick();

    const second = postBodies(fetchMock, "/track/hit")[0];
    expect(second.i).toBe(first.i);
    expect(second.b).not.toBe(first.b);
  });

  it("keeps the session id across full page loads within a tab", async () => {
    // Two inits simulate a traditional multi-page navigation: the JS context is
    // rebuilt, but sessionStorage carries the tab's session id across.
    init({ fetch: fetchMock, navigator: navMock });
    await tick();
    const first = postBodies(fetchMock, "/track/hit")[0];

    const fetch2 = makeFetch(false);
    init({ fetch: fetch2, navigator: navMock });
    await tick();

    const second = postBodies(fetch2, "/track/hit")[0];
    expect(second.i).toBe(first.i);
  });

  it("falls back to an in-memory session id when storage is unavailable", async () => {
    const denied = vi
      .spyOn(Storage.prototype, "getItem")
      .mockImplementation(() => {
        throw new Error("storage disabled");
      });
    try {
      init({ fetch: fetchMock, navigator: navMock });
      await tick();

      const load = postBodies(fetchMock, "/track/hit")[0];
      expect(typeof load.i).toBe("string");
      expect(load.i.length).toBeGreaterThan(0);
    } finally {
      denied.mockRestore();
    }
  });

  it("ignores a same-path replaceState", async () => {
    init({ fetch: fetchMock, navigator: navMock });
    await tick();
    fetchMock.mockClear();
    navMock.sendBeacon.mockClear();

    window.history.replaceState({}, "", "/");
    await tick();

    expect(navMock.sendBeacon).not.toHaveBeenCalled();
    expect(postBodies(fetchMock, "/track/hit")).toHaveLength(0);
  });
});

describe("init — exceptions", () => {
  it("auto-captures window errors when enabled", async () => {
    init({ fetch: fetchMock, navigator: navMock, captureExceptions: true });
    await tick();

    fireError(new TypeError("kaboom"), "kaboom");

    const bodies = await beaconBodies(navMock.sendBeacon, "/track/exception");
    expect(bodies).toHaveLength(1);
    expect(bodies[0]).toMatchObject({ ty: "TypeError", m: "kaboom", h: false });
    expect(bodies[0].u).toContain("example.test");
    // The report is linked to the same session as the page views.
    expect(bodies[0].i).toBe(postBodies(fetchMock, "/track/hit")[0].i);
  });

  it("auto-captures unhandled promise rejections", async () => {
    init({ fetch: fetchMock, navigator: navMock, captureExceptions: true });
    await tick();

    const ev = new Event("unhandledrejection");
    ev.reason = new Error("rejected");
    window.dispatchEvent(ev);

    const bodies = await beaconBodies(navMock.sendBeacon, "/track/exception");
    expect(bodies[0]).toMatchObject({ ty: "Error", m: "rejected", h: false });
  });

  it("does not capture when the attribute is off", async () => {
    init({ fetch: fetchMock, navigator: navMock, captureExceptions: false });
    await tick();

    fireError(new Error("x"), "x");

    expect(await beaconBodies(navMock.sendBeacon, "/track/exception")).toHaveLength(0);
  });
});

describe("init — public API", () => {
  it("sends custom events with stringified metadata", async () => {
    const api = init({ fetch: fetchMock, navigator: navMock });
    await tick();
    fetchMock.mockClear();

    api.event("signup", { plan: "pro", count: 3 });

    const events = postBodies(fetchMock, "/track/hit");
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({
      e: "custom",
      n: "signup",
      d: { plan: "pro", count: "3" },
    });
  });

  it("captures manual exceptions as handled", async () => {
    const api = init({ fetch: fetchMock, navigator: navMock });
    await tick();

    api.captureException(new Error("manual"), { context: "checkout" });

    const bodies = await beaconBodies(navMock.sendBeacon, "/track/exception");
    expect(bodies).toHaveLength(1);
    expect(bodies[0]).toMatchObject({
      ty: "Error",
      m: "manual",
      h: true,
      d: { context: "checkout" },
    });
  });

  it("reads its configuration from the script element", async () => {
    const script = document.createElement("script");
    script.setAttribute("data-api", "https://collect.example");
    script.setAttribute("data-auto-capture-exceptions", "true");
    document.body.appendChild(script);

    init({ fetch: fetchMock, navigator: navMock, script });
    await tick();

    expect(getUrls(fetchMock, "/track/ping")[0]).toContain(
      "https://collect.example/track/ping",
    );
  });

  it("derives the collection host from its own src when data-api is absent", async () => {
    const script = document.createElement("script");
    script.src = "https://collect.example/tracker.js";
    document.body.appendChild(script);

    init({ fetch: fetchMock, navigator: navMock, script });
    await tick();

    expect(getUrls(fetchMock, "/track/ping")[0]).toContain(
      "https://collect.example/track/ping",
    );
  });
});
