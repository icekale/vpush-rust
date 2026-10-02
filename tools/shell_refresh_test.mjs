import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import * as html from "../static-assets/core/html.js";

const source = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const version = (text) => text.match(/(?:const APP_VERSION(?:: &str)? = )"([^"]+)"/)?.[1];

test("old 1.12.278 clients receive a newer matching server and shell version", () => {
  const server = version(source("../src/main.rs"));
  const shell = version(source("../static-assets/app.js"));
  assert.ok(server && server !== "1.12.278", `server still advertises ${server}`);
  assert.equal(shell, server);
});

test("shell revision changes for new imports, CSS or app module", () => {
  assert.equal(typeof html.shellRevision, "function");
  const doc = (imports, style, app) => ({
    querySelector: (selector) => ({
      'script[type="importmap"]': imports && { textContent: imports },
      'link[rel="stylesheet"][href^="/style."]': style && { getAttribute: () => style },
      'script[type="module"][src^="/app."]': app && { getAttribute: () => app },
    })[selector] || null,
  });
  const old = html.shellRevision(doc("watchlist.a.js", "/style.a.css", "/app.a.js"));
  for (const next of [
    doc("watchlist.b.js", "/style.a.css", "/app.a.js"),
    doc("watchlist.a.js", "/style.b.css", "/app.a.js"),
    doc("watchlist.a.js", "/style.a.css", "/app.b.js"),
  ]) assert.notEqual(html.shellRevision(next), old);
  assert.equal(html.shellRevision(doc("watchlist.a.js", "/style.a.css", "/app.a.js")), old);
  assert.equal(html.shellRevision(doc(null, null, null)), "");
});
