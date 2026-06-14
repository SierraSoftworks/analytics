import { describe, it, expect } from "vitest";
import { privacySignal } from "../src/privacy.js";

describe("privacySignal", () => {
  it("is false when no opt-out signal is present", () => {
    expect(privacySignal({ doNotTrack: null }, {})).toBe(false);
    expect(privacySignal({ doNotTrack: "0" }, {})).toBe(false);
    expect(privacySignal({ doNotTrack: "unspecified" }, {})).toBe(false);
  });

  it("honours navigator.doNotTrack", () => {
    expect(privacySignal({ doNotTrack: "1" }, {})).toBe(true);
    expect(privacySignal({ doNotTrack: "yes" }, {})).toBe(true);
  });

  it("honours the legacy window.doNotTrack", () => {
    expect(privacySignal({ doNotTrack: null }, { doNotTrack: "1" })).toBe(true);
  });

  it("honours Global Privacy Control", () => {
    expect(privacySignal({ globalPrivacyControl: true }, {})).toBe(true);
    expect(privacySignal({ globalPrivacyControl: false }, {})).toBe(false);
  });

  it("is false when navigator is unavailable", () => {
    expect(privacySignal(undefined, {})).toBe(false);
  });
});
