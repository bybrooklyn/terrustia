import { defineConfig } from "@playwright/test";

// The panel is verified in a real browser, against a real server.
//
// `AGENTS.md` has claimed that since long before any of this existed: there was no Playwright
// dependency, no config, no spec and no test script in the repository, while `TODO.md`'s own
// final-verification list required "the admin overhaul verified against a real client and
// Playwright" before tagging a release, and `docs/release-blockers.md` did not carry the clause at
// all. This file is the first half of making the sentence true.
//
// No `webServer` block: the spec starts the real `terrustia` binary itself, with the panel
// embedded, so what the browser talks to is the shipped server rather than a dev proxy. Serving
// the panel from Vite would test the frontend against nothing.
export default defineConfig({
  testDir: "./tests",
  // One worker: every spec drives a real server process bound to a real port.
  workers: 1,
  fullyParallel: false,
  reporter: [["list"]],
  timeout: 60_000,
  expect: { timeout: 15_000 },
  use: {
    // Filled in per-test from the port the server actually got.
    headless: true,
    trace: "retain-on-failure",
  },
});
