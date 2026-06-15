import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // The tracker talks to the DOM and browser globals, so run the suite against a
    // simulated browser environment with a stable, non-localhost origin.
    environment: "jsdom",
    environmentOptions: {
      jsdom: { url: "https://example.test/" },
    },
    include: ["test/**/*.test.js"],
    // Reset spies/mocks (fetch, sendBeacon, timers) between tests automatically.
    restoreMocks: true,
    clearMocks: true,
  },
});
