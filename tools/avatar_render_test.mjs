import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";
import test from "node:test";
import { escapeHtml, imgSrcFor, imgOnError } from "../static-assets/core/html.js";

const app = await readFile(new URL("../static-assets/app.js", import.meta.url), "utf8");
const avatarHtml = vm.runInNewContext(`${app.slice(app.indexOf("function avatarText("), app.indexOf("// ---------- 壳 ----------"))}; avatarHtml`, { escapeHtml, imgSrcFor });

test("avatars use the image proxy and upgrade insecure source URLs", () => {
  const source = "https://pbs.twimg.com/profile_images/123/avatar.jpg";
  const html = avatarHtml("Alice", source);
  assert.match(html, /src="\/api\/img-proxy\?url=/);
  assert.match(html, /onerror="imgOnError\(this\)"/);
  assert.match(html, /data-avatar-fallback="A"/);
  assert.match(avatarHtml("雪球", "http://xavatar.imedao.com/a.png"), /src="https:\/\/xavatar.imedao.com\/a.png"/);
  assert.match(avatarHtml("Local", "/avatars/90.jpg", "truth"), /src="\/avatars\/90.jpg"/);
  assert.match(avatarHtml("Local", "/avatars/90.jpg", "truth"), /avatar-verified/);
  assert.match(avatarHtml("<script>", ""), /&lt;/);
});

test("failed avatar requests retry remotely once, then render a safe initial", () => {
  const previousDocument = globalThis.document;
  globalThis.document = { createElement: () => ({}) };
  const image = {
    dataset: { avatarFallback: "<" }, src: "https://xavatar.imedao.com/missing.png",
    getAttribute() { return this.src; }, replaceWith(node) { this.replacement = node; },
  };
  try {
    imgOnError(image);
    assert.match(image.src, /^\/api\/img-proxy\?url=/);
    assert.equal(typeof image.onerror, "function");
    image.onerror();
    assert.equal(image.replacement.className, "kol-avatar");
    assert.equal(image.replacement.textContent, "<");
    const local = { ...image, dataset: { avatarFallback: "L" }, src: "/avatars/missing.jpg", replacement: null };
    imgOnError(local);
    assert.equal(local.replacement.textContent, "L");
    const photo = { ...image, dataset: {}, src: "https://xqimg.imedao.com/a.png", onerror() {} };
    imgOnError(photo);
    assert.equal(photo.onerror, null);
  } finally { globalThis.document = previousDocument; }
});
