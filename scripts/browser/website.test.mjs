// Uses the pages of a project made from the website template in Google Chrome, as a visitor
// would: the menu, the form with mistakes and without, the page for a missing address, and the
// reload after the server restarts. scripts/check-templates.sh --browser runs it, with the
// project's directory in WEBSITE_PROJECT. The project runs with `cargo run`, so that its
// .cargo/config.toml applies.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { after, before, test } from "node:test";
import { chromium } from "playwright-core";

const project = process.env.WEBSITE_PROJECT;
if (!project) throw new Error("set WEBSITE_PROJECT to the generated website project");

/** How long the project may take to build and to begin serving. */
const START_LIMIT = 300_000;
/** How long it may take to stop. A reload stream that held up the shutdown would take 30 s. */
const STOP_LIMIT = 10_000;

/** Start the project, and resolve once it logs the address to open. */
function start() {
  const env = { ...process.env, NO_COLOR: "1", WARDED_HTTPS_ADDR: "127.0.0.1:0" };
  delete env.WARDED_DEV_ADDR;
  const child = spawn("cargo", ["run", "--quiet"], {
    cwd: project,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const server = { child, log: "", exited: new Promise((done) => child.once("exit", done)) };
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`no address to open after ${START_LIMIT} ms:\n${server.log}`));
    }, START_LIMIT);
    const read = (chunk) => {
      server.log += chunk;
      const open = /open (http:\/\/\S+) in a browser/.exec(server.log);
      if (open && !server.url) {
        clearTimeout(timer);
        server.url = open[1];
        resolve(server);
      }
    };
    child.stdout.setEncoding("utf8").on("data", read);
    child.stderr.setEncoding("utf8").on("data", read);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    server.exited.then((code) => {
      clearTimeout(timer);
      reject(new Error(`the server ended with ${code}:\n${server.log}`));
    });
  });
}

/** Stop the server, as Ctrl+C or a watcher does, and return how long it took. */
async function stop(server) {
  const began = Date.now();
  server.child.kill("SIGTERM");
  const stopped = await Promise.race([
    server.exited.then(() => true),
    new Promise((done) => setTimeout(done, STOP_LIMIT, false)),
  ]);
  if (!stopped) {
    server.child.kill("SIGKILL");
    throw new Error(`the server did not stop within ${STOP_LIMIT} ms:\n${server.log}`);
  }
  return Date.now() - began;
}

let server;
let browser;
let page;

before(async () => {
  server = await start();
  // The Chrome that is installed, so that nothing is downloaded.
  browser = await chromium.launch({ channel: "chrome" });
  page = await browser.newPage();
});

after(async () => {
  await browser?.close();
  if (server) await stop(server);
});

const at = (path) => new URL(path, server.url).href;
const heading = () => page.getByRole("heading", { level: 1 }).textContent();

test("the menu leads to every page and marks the current one", async () => {
  const response = await page.goto(server.url);
  assert.equal(response.status(), 200);
  assert.equal(await page.title(), "Welcome");
  for (const [name, path, title] of [
    ["About", "/about", "About"],
    ["Contact", "/contact", "Contact"],
    ["Home", "/", "Welcome"],
  ]) {
    const menu = page.getByRole("navigation", { name: "Main" });
    await menu.getByRole("link", { name, exact: true }).click();
    await page.waitForURL(at(path));
    assert.equal(await heading(), title);
    assert.equal(await menu.locator('[aria-current="page"]').textContent(), name);
  }
});

test("the styles and the icon arrive", async () => {
  await page.goto(server.url);
  const display = await page.evaluate(
    () => getComputedStyle(document.querySelector("nav ul")).display,
  );
  assert.equal(display, "flex");
  const icon = await page.evaluate(async () => (await fetch("/static/favicon.svg")).status);
  assert.equal(icon, 200);
});

test("the form shows its mistakes, and redirects once it is right", async () => {
  await page.goto(at("/contact"));
  const field = (label) => page.getByLabel(label, { exact: true });
  const send = () => page.getByRole("button", { name: "Send" }).click();

  await field("E-mail address").fill("someone");
  await field("Message").fill("Hello");
  const [refused] = await Promise.all([
    page.waitForResponse((response) => response.request().method() === "POST"),
    send(),
  ]);
  assert.equal(refused.status(), 422);
  await page.waitForLoadState();
  assert.equal(await page.title(), "Error: Contact");
  assert.equal(await field("E-mail address").inputValue(), "someone");
  assert.equal(await field("Message").inputValue(), "Hello");
  for (const [label, invalid] of [
    ["Name", "true"],
    ["E-mail address", "true"],
    ["Message", null],
  ]) {
    assert.equal(await field(label).getAttribute("aria-invalid"), invalid, label);
  }
  // Each problem is announced with its field, and linked from the list at the top.
  const described = await field("Name").getAttribute("aria-describedby");
  assert.equal(await page.locator(`#${described}`).textContent(), "Enter your name.");
  await page.getByRole("link", { name: "Enter your name." }).click();
  assert.equal(await page.evaluate(() => location.hash), "#name");

  await field("Name").fill("Ada");
  await field("E-mail address").fill("ada@example.com");
  await Promise.all([page.waitForURL(at("/contact/sent")), send()]);
  assert.equal(await heading(), "Thank you");
  // Reloading the page after the redirect does not send the form again.
  const reloaded = await page.reload();
  assert.equal(reloaded.request().method(), "GET");
  assert.equal(reloaded.status(), 200);
});

test("an address without a page gets the page for it", async () => {
  const response = await page.goto(at("/no/such/page"));
  assert.equal(response.status(), 404);
  assert.equal(await heading(), "Page not found");
  await page.getByRole("link", { name: "Go to the start page" }).click();
  await page.waitForURL(server.url);
});

test("a page reloads by itself when the server restarts", async () => {
  const listening = page.waitForResponse((response) =>
    response.url().endsWith("/_tower-livereload/event-stream"),
  );
  await page.goto(server.url);
  assert.equal((await listening).status(), 200);
  await page.evaluate(() => {
    window.beforeTheRestart = true;
  });

  const took = await stop(server);
  // On Unix this is a graceful shutdown, which the open stream must not hold up.
  assert.ok(took < STOP_LIMIT, `stopping took ${took} ms`);
  server = await start();
  await page.waitForFunction(() => window.beforeTheRestart === undefined, null, {
    timeout: 30_000,
  });
  assert.equal(await heading(), "Welcome");
});
