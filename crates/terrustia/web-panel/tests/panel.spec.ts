import { test, expect } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync, readFileSync, existsSync, openSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The web panel, driven in a real browser against the real server.
//
// What makes this worth having rather than a frontend smoke test: the server is the shipped
// release binary with the panel embedded (`embed-web`), so a break in the embedding, the routes,
// the auth handshake or the JSON shape fails here. Serving the panel from Vite would prove only
// that Svelte renders.

// The panel is an ES module package, so `__dirname` does not exist here.
const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "../../../..");
const BIN = join(ROOT, "target/release/terrustia");

let server: ChildProcess | undefined;
let dir = "";
let panelUrl = "";

/** Wait for a line to appear in the server's log, or fail loudly with what it did say. */
async function waitForLog(logPath: string, needle: string, seconds = 60): Promise<string> {
  for (let i = 0; i < seconds * 10; i++) {
    if (existsSync(logPath)) {
      const text = readFileSync(logPath, "utf8");
      if (text.includes(needle)) return text;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  const text = existsSync(logPath) ? readFileSync(logPath, "utf8") : "(no log at all)";
  throw new Error(`server never logged ${JSON.stringify(needle)}. Log was:\n${text}`);
}

test.beforeAll(async () => {
  if (!existsSync(BIN)) {
    throw new Error(
      `no release binary at ${BIN}. Build it first: cargo build --release -p terrustia --bin terrustia --features embed-web`,
    );
  }
  dir = mkdtempSync(join(tmpdir(), "terrustia-panel-"));
  // Ports well clear of the defaults, so a server already running on this machine is untouched.
  const gamePort = 39871;
  const panelPort = 39872;
  writeFileSync(
    join(dir, "panel.toml"),
    [
      `world_name = "PanelTest"`,
      `save_file = "${join(dir, "panel.wld")}"`,
      `listen = "127.0.0.1:${gamePort}"`,
      `panel_enabled = true`,
      `panel_listen = "127.0.0.1:${panelPort}"`,
      `update_check_enabled = false`,
      `upnp_enabled = false`,
      `max_players = 4`,
      "",
    ].join("\n"),
  );

  const log = join(dir, "server.log");
  const out = openSync(log, "a");
  server = spawn(BIN, ["-c", join(dir, "panel.toml"), "--headless"], {
    stdio: ["ignore", out, out],
  });
  await waitForLog(log, "accepting connections");
  panelUrl = `http://127.0.0.1:${panelPort}`;
});

test.afterAll(async () => {
  server?.kill("SIGTERM");
  await new Promise((r) => setTimeout(r, 1500));
  server?.kill("SIGKILL");
  if (dir) rmSync(dir, { recursive: true, force: true });
});

test("the panel loads from the server's own embedded copy", async ({ page }) => {
  const response = await page.goto(panelUrl);
  expect(response?.status(), "the panel should be served, not 404").toBe(200);
  await expect(page).toHaveTitle(/terrustia/i);
  // The Svelte app really mounted, rather than the shell HTML being served alone.
  await expect(page.locator("#app")).not.toBeEmpty();
});

test("the panel guards its own API until the server is claimed", async ({ page }) => {
  // 401, not 200: `/api/status` carries live server state and is not readable by an unauthenticated
  // caller. The first version of this test asserted 200 and was simply wrong about the server -
  // which is worth keeping as an assertion, because a future change that opened this up would now
  // fail here rather than pass quietly.
  const res = await page.request.get(`${panelUrl}/api/status`, { failOnStatusCode: false });
  expect(res.status(), "an unauthenticated status read must be refused").toBe(401);
});

test("a fresh server reports itself unclaimed, which is what gates the first login", async ({
  page,
}) => {
  const res = await page.request.get(`${panelUrl}/api/unclaimed`);
  expect(res.status()).toBe(200);
  expect((await res.json()).unclaimed).toBe(true);
});

test("a path outside the embedded tree never serves a file from disk", async ({ page }) => {
  // The panel is a single-page app, so an unknown path legitimately answers 200 with the app's own
  // shell - that is routing, not traversal. What must never happen is the *contents of a file on
  // disk* coming back. The first version of this test asserted "not 200" and failed against
  // correct behaviour; probing by hand showed the body was the panel's own index.html, and that a
  // raw un-normalised `GET /../../../etc/passwd` is refused outright.
  const res = await page.request.get(`${panelUrl}/%2e%2e%2f%2e%2e%2fetc/passwd`, {
    failOnStatusCode: false,
  });
  const body = await res.text();
  expect(body, "a system file must never be served").not.toContain("root:");
  expect(body, "the SPA shell is the only acceptable answer").toContain("<!doctype html>");
});
