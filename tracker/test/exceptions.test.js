import { describe, it, expect } from "vitest";
import {
  describeError,
  buildExceptionPayload,
  createExceptionReporter,
} from "../src/exceptions.js";

describe("describeError", () => {
  it("reads Error instances, including the stack", () => {
    const d = describeError(new TypeError("boom"));
    expect(d.name).toBe("TypeError");
    expect(d.message).toBe("boom");
    expect(typeof d.stack).toBe("string");
  });

  it("reads error-like objects and applies the fallback name", () => {
    expect(describeError({ message: "x" }, "Custom")).toEqual({
      name: "Custom",
      message: "x",
      stack: undefined,
    });
    expect(describeError({ name: "Foo", message: "x" })).toMatchObject({
      name: "Foo",
      message: "x",
    });
  });

  it("stringifies primitive and structured rejection reasons", () => {
    expect(describeError("nope", "UnhandledRejection")).toEqual({
      name: "UnhandledRejection",
      message: "nope",
      stack: undefined,
    });
    expect(describeError(undefined)).toEqual({
      name: "Error",
      message: "undefined",
      stack: undefined,
    });
    expect(describeError(null)).toEqual({ name: "Error", message: "null", stack: undefined });
    expect(describeError({ code: 42 })).toMatchObject({
      name: "Error",
      message: '{"code":42}',
    });
  });
});

describe("buildExceptionPayload", () => {
  it("includes required and optional fields with short keys", () => {
    const p = buildExceptionPayload(
      { name: "TypeError", message: "boom", stack: "at x" },
      {
        url: "https://a/x",
        beacon: "b1",
        handled: true,
        appVersion: "1.4.2",
        meta: { k: "v" },
      },
    );
    expect(p).toEqual({
      u: "https://a/x",
      ty: "TypeError",
      m: "boom",
      h: true,
      b: "b1",
      s: "at x",
      v: "1.4.2",
      d: { k: "v" },
    });
  });

  it("omits optional fields and falls back to the type for an empty message", () => {
    const p = buildExceptionPayload(
      { name: "Error", message: "", stack: undefined },
      { url: "u" },
    );
    expect(p).toEqual({ u: "u", ty: "Error", m: "Error", h: false });
  });

  it("truncates oversized stacks", () => {
    const p = buildExceptionPayload(
      { name: "E", message: "m", stack: "x".repeat(20000) },
      {},
    );
    expect(p.s.length).toBe(16000);
  });
});

describe("createExceptionReporter", () => {
  function setup(max) {
    const sent = [];
    const reporter = createExceptionReporter({
      send: (p) => sent.push(p),
      url: () => "https://a/x",
      beacon: () => "b1",
      max,
    });
    return { sent, reporter };
  }

  it("builds a report from the getters and handled flag", () => {
    const { sent, reporter } = setup();
    reporter.report(new Error("boom"), true, { k: "v" });
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({
      u: "https://a/x",
      b: "b1",
      ty: "Error",
      m: "boom",
      h: true,
      d: { k: "v" },
    });
  });

  it("attributes reports to the configured app version", () => {
    const sent = [];
    const reporter = createExceptionReporter({
      send: (p) => sent.push(p),
      url: () => "https://a/x",
      beacon: () => "b1",
      appVersion: "1.4.2",
    });
    reporter.report(new Error("boom"), false);
    expect(sent[0].v).toBe("1.4.2");
    expect(sent[0].a).toBeUndefined();
  });

  it("deduplicates identical occurrences", () => {
    const { sent, reporter } = setup();
    const err = new Error("boom");
    reporter.report(err, false);
    reporter.report(err, false);
    expect(sent).toHaveLength(1);
  });

  it("reports occurrences with distinct messages separately", () => {
    const { sent, reporter } = setup();
    reporter.report(new Error("a"), false);
    reporter.report(new Error("b"), false);
    expect(sent).toHaveLength(2);
  });

  it("caps the number of reports", () => {
    const { sent, reporter } = setup(2);
    reporter.report(new Error("a"), false);
    reporter.report(new Error("b"), false);
    reporter.report(new Error("c"), false);
    expect(sent).toHaveLength(2);
  });
});
