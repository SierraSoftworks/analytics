import { describe, it, expect, vi } from "vitest";
import { joinUrl, stringifyMeta, createTransport } from "../src/transport.js";

describe("joinUrl", () => {
  it("returns the path unchanged when there is no API base", () => {
    expect(joinUrl("", "/track/hit")).toBe("/track/hit");
  });

  it("joins the base and path, trimming trailing slashes", () => {
    expect(joinUrl("https://a.example", "/track/hit")).toBe("https://a.example/track/hit");
    expect(joinUrl("https://a.example/", "/track/hit")).toBe("https://a.example/track/hit");
    expect(joinUrl("https://a.example///", "/track/hit")).toBe("https://a.example/track/hit");
  });
});

describe("stringifyMeta", () => {
  it("returns undefined for empty or non-object input", () => {
    expect(stringifyMeta(undefined)).toBeUndefined();
    expect(stringifyMeta(null)).toBeUndefined();
    expect(stringifyMeta("x")).toBeUndefined();
    expect(stringifyMeta({})).toBeUndefined();
    expect(stringifyMeta({ a: null, b: undefined })).toBeUndefined();
  });

  it("coerces values to strings and drops null/undefined", () => {
    expect(stringifyMeta({ a: 1, b: true, c: "x", d: null, e: undefined })).toEqual({
      a: "1",
      b: "true",
      c: "x",
    });
  });
});

describe("createTransport.post", () => {
  it("posts preflight-free via no-cors text/plain fetch with keepalive", () => {
    const fetch = vi.fn(() => Promise.resolve({ ok: true }));
    const t = createTransport("https://a.example", { fetch, navigator: {} });

    t.post("/track/hit", { b: "1", e: "load" }, false);

    expect(fetch).toHaveBeenCalledTimes(1);
    const [url, opts] = fetch.mock.calls[0];
    expect(url).toBe("https://a.example/track/hit");
    expect(opts.method).toBe("POST");
    expect(opts.keepalive).toBe(true);
    expect(opts.credentials).toBe("omit");
    // text/plain + no-cors keeps it a CORS "simple request" (no preflight).
    expect(opts.mode).toBe("no-cors");
    expect(opts.headers["Content-Type"]).toBe("text/plain");
    expect(JSON.parse(opts.body)).toEqual({ b: "1", e: "load" });
  });

  it("prefers a text/plain sendBeacon when asked and it succeeds", () => {
    const fetch = vi.fn();
    const sendBeacon = vi.fn(() => true);
    const t = createTransport("", { fetch, navigator: { sendBeacon } });

    t.post("/track/hit", { e: "unload" }, true);

    expect(sendBeacon).toHaveBeenCalledTimes(1);
    expect(sendBeacon.mock.calls[0][0]).toBe("/track/hit");
    expect(sendBeacon.mock.calls[0][1].type).toBe("text/plain");
    expect(fetch).not.toHaveBeenCalled();
  });

  it("falls back to fetch when sendBeacon reports failure", () => {
    const fetch = vi.fn(() => Promise.resolve({ ok: true }));
    const sendBeacon = vi.fn(() => false);
    const t = createTransport("", { fetch, navigator: { sendBeacon } });

    t.post("/track/hit", { e: "unload" }, true);

    expect(sendBeacon).toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("swallows fetch rejections", async () => {
    const fetch = vi.fn(() => Promise.reject(new Error("network")));
    const t = createTransport("", { fetch, navigator: {} });
    expect(() => t.post("/track/hit", {}, false)).not.toThrow();
    await Promise.resolve();
  });
});

describe("createTransport.get", () => {
  it("issues a credential-less GET through the HTTP cache", () => {
    const fetch = vi.fn(() => Promise.resolve({ ok: true }));
    const t = createTransport("https://a.example", { fetch, navigator: {} });

    t.get("/track/ping?h=x");

    const [url, opts] = fetch.mock.calls[0];
    expect(url).toBe("https://a.example/track/ping?h=x");
    expect(opts.credentials).toBe("omit");
    expect(opts.method).toBeUndefined();
  });
});
