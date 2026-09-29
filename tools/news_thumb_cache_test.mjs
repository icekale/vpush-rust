// 用真实 createNewsView 核对列表缩略图负缓存。
// 90 篇 × 3 个失败下标：第二轮图片请求必须是 0。
// 超时记入 60 秒负缓存；离开资讯页后整表清空。
import { spawn } from "node:child_process";
import http from "node:http";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const staticDir = path.join(root, "static-assets");
const pageName = "_thumb_cache_test.html";
const pagePath = path.join(staticDir, pageName);

const page = `<!doctype html>
<meta charset="utf-8">
<div id="main"></div>
<script type="module">
window.addEventListener("error", (event) => {
  window.__done = { failures: ["page-error: " + (event.message || event.error)] };
});
window.addEventListener("unhandledrejection", (event) => {
  const reason = event.reason;
  window.__done = { failures: ["rejection: " + (reason && reason.stack ? reason.stack : reason)] };
});
const realSetTimeout = window.setTimeout.bind(window);
const realClearTimeout = window.clearTimeout.bind(window);
let now = 1_000_000_000;
const timers = new Map();
let timerSeq = 1;
Date.now = () => now;
window.setTimeout = (fn, ms) => {
  const id = timerSeq++;
  timers.set(id, { fn, at: now + Number(ms || 0) });
  return id;
};
window.clearTimeout = (id) => { timers.delete(id); };
window.setInterval = () => 0;
window.clearInterval = () => {};
async function advance(ms) {
  now += ms;
  for (let guard = 0; guard < 10000; guard += 1) {
    let chosen = null;
    for (const [id, timer] of timers) {
      if (timer.at <= now && (!chosen || timer.at < chosen.timer.at)) chosen = { id, timer };
    }
    if (!chosen) break;
    timers.delete(chosen.id);
    chosen.timer.fn();
    await Promise.resolve();
  }
}
async function drain() {
  for (let i = 0; i < 20000; i += 1) await Promise.resolve();
}

const imageCalls = [];
let imageMode = "fail";
const { createNewsView } = await import("./views/news.js");

function articles(count, idBase) {
  return Array.from({ length: count }, (_, index) => ({
    id: idBase + index,
    title: "标题",
    summary: "摘要",
    source_name: "财新",
    source_platform: "caixin",
    published_at: "2026-09-29T02:04:00Z",
    topics: ["宏观", "国际", "科技", "公司"],
    has_image: true,
    is_read: false,
  }));
}

let newsItems = articles(90, 100);
const state = {
  newsItems: [],
  newsSources: [],
  newsOffset: 0,
  newsHasMore: false,
  newsRequestSeq: 0,
  newsScrollY: 0,
  newsListKey: "",
  newsQuery: "",
  newsUnreadOnly: false,
  newsTopic: "",
  newsFilterSourceId: "",
  newsUnreadCount: 0,
  newsImageUrls: new Map(),
  newsMagazine: false,
  newsCollectionEnabled: true,
  newsObserver: null,
  user: {},
};
let routeSeq = 1;
let routeOk = true;

function apiBlob(url, options = {}) {
  imageCalls.push(url);
  if (imageMode === "fail") return Promise.reject(Object.assign(new Error("fail"), { status: 404 }));
  if (imageMode === "ok") {
    return Promise.resolve(new Blob([new Uint8Array([1, 2, 3, 4])]));
  }
  return new Promise((_, reject) => {
    const fail = () => reject(Object.assign(new Error("aborted"), { name: "AbortError" }));
    if (options.signal?.aborted) fail();
    else options.signal?.addEventListener("abort", fail, { once: true });
  });
}

const originalDecode = HTMLImageElement.prototype.decode;
HTMLImageElement.prototype.decode = function decode() {
  if (String(this.src || "").startsWith("blob:")) {
    Object.defineProperty(this, "naturalWidth", { configurable: true, value: 80 });
    Object.defineProperty(this, "naturalHeight", { configurable: true, value: 60 });
    return Promise.resolve();
  }
  return originalDecode ? originalDecode.call(this) : Promise.resolve();
};

const view = createNewsView({
  $: (sel) => document.querySelector(sel),
  state,
  api: async (url) => {
    if (url.startsWith("/api/news/sources")) {
      return {
        items: [{ id: 1, name: "财新", selected: true, enabled: true, platform: "caixin", unread_count: 0, kind: "feed" }],
        unread_count: 0,
        collection_enabled: true,
      };
    }
    return { items: newsItems, next_offset: newsItems.length, has_more: false };
  },
  apiBlob,
  routeStillActive: (seq) => routeOk && seq === routeSeq,
  currentRouteSeq: () => routeSeq,
  setPageTitle() {},
  emptyState: (title) => \`<div class="empty-state">\${title}</div>\`,
  go() {},
  flash() {},
  escapeHtml: (value) => String(value ?? "").replace(/[&<>"']/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[ch])),
  trapFocus: () => () => {},
  fmtPublished: () => "10:04",
  externalLinkIcon: "",
  renderSidebar() {},
  renderBottomNav() {},
  updateNewsBadge() {},
  SEARCH_ICON: "",
  GEAR_ICON: "",
  NEWS_ICON: "",
  EYE_ICON: "",
  CHEVRON_DOWN_ICON: "",
  CHECK_ICON: "<span></span>",
  CHECK_CHECK_ICON: "",
});

function imageCount() {
  return imageCalls.length;
}

async function renderFresh() {
  state.newsItems = [];
  state.newsListKey = "reload-" + imageCount();
  state.newsOffset = 0;
  await view.renderFinancialNewsList();
  await drain();
}

const failures = [];
function check(name, ok, detail) {
  if (!ok) failures.push(name + ": " + detail);
}

await renderFresh();
await drain();
const first = imageCount();
check("first-round-270", first === 270, String(first));

const beforeSecond = imageCount();
await renderFresh();
await drain();
check("second-round-0", imageCount() - beforeSecond === 0, String(imageCount() - beforeSecond));

view.clearNewsReaderState();
routeSeq += 1;
const beforeAfterLeave = imageCount();
await renderFresh();
await drain();
check("after-leave-rerequests", imageCount() - beforeAfterLeave === 270, String(imageCount() - beforeAfterLeave));

imageMode = "timeout";
newsItems = articles(1, 1);
view.clearNewsReaderState();
routeSeq += 1;
const beforeTimeout = imageCount();
const renderPromise = renderFresh();
await drain();
await advance(8000);
await drain();
await advance(8000);
await drain();
await advance(8000);
await drain();
await renderPromise;
const timeoutCalls = imageCount() - beforeTimeout;
check("timeout-first-3", timeoutCalls === 3, String(timeoutCalls));
const beforeTimeoutSecond = imageCount();
await renderFresh();
await drain();
check("timeout-second-0", imageCount() - beforeTimeoutSecond === 0, String(imageCount() - beforeTimeoutSecond));
await advance(60_000);
const beforeTimeoutExpired = imageCount();
const again = renderFresh();
await drain();
await advance(8000);
await drain();
await advance(8000);
await drain();
await advance(8000);
await drain();
await again;
check("timeout-after-expiry-3", imageCount() - beforeTimeoutExpired === 3, String(imageCount() - beforeTimeoutExpired));

const result = { failures, imageCalls: imageCalls.length, now };
window.__done = result;
</script>
`;

function listen(port = 0) {
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, "http://127.0.0.1");
    const rel = decodeURIComponent(url.pathname);
    const file = path.normalize(path.join(staticDir, rel));
    if (!file.startsWith(staticDir)) {
      res.writeHead(403);
      res.end("no");
      return;
    }
    fs.readFile(file, (err, data) => {
      if (err) {
        res.writeHead(404);
        res.end("missing");
        return;
      }
      const ext = path.extname(file);
      const type = ext === ".js" ? "text/javascript" : ext === ".html" ? "text/html" : "application/octet-stream";
      res.writeHead(200, { "content-type": type, "cache-control": "no-store" });
      res.end(data);
    });
  });
  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => resolve(server));
  });
}

function freePort() {
  return new Promise((resolve) => {
    const probe = net.createServer();
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });
}

function get(url) {
  return new Promise((resolve, reject) => {
    http.get(url, (res) => {
      let body = "";
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => resolve({ status: res.statusCode, body }));
    }).on("error", reject);
  });
}

async function waitFor(fn, label, attempts = 50) {
  let last = "";
  for (let i = 0; i < attempts; i += 1) {
    try {
      return await fn();
    } catch (err) {
      last = err && err.message ? err.message : String(err);
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  throw new Error(label + ": " + last);
}

class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.next = 1;
    this.pending = new Map();
    ws.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id && this.pending.has(message.id)) {
        const { resolve, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (message.error) reject(new Error(JSON.stringify(message.error)));
        else resolve(message.result);
      }
    });
  }
  send(method, params = {}) {
    const id = this.next++;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
  }
}

async function main() {
  fs.writeFileSync(pagePath, page);
  const web = await listen();
  const webPort = web.address().port;
  const debugPort = await freePort();
  const profile = fs.mkdtempSync(path.join("/tmp", "thumb-chrome-"));
  const chrome = spawn("google-chrome", [
    "--headless=new",
    "--disable-gpu",
    "--no-sandbox",
    "--disable-dev-shm-usage",
    `--user-data-dir=${profile}`,
    `--remote-debugging-port=${debugPort}`,
    "about:blank",
  ], { stdio: "ignore" });
  try {
    const version = await waitFor(() => get(`http://127.0.0.1:${debugPort}/json/version`), "chrome");
    const browserUrl = JSON.parse(version.body).webSocketDebuggerUrl;
    const browser = new Cdp(new WebSocket(browserUrl));
    await new Promise((resolve, reject) => {
      browser.ws.addEventListener("open", resolve);
      browser.ws.addEventListener("error", reject);
    });
    const created = await browser.send("Target.createTarget", {
      url: `http://127.0.0.1:${webPort}/${pageName}`,
    });
    const list = await waitFor(async () => {
      const response = await get(`http://127.0.0.1:${debugPort}/json/list`);
      const targets = JSON.parse(response.body);
      const target = targets.find((item) => item.id === created.targetId);
      if (!target || !target.webSocketDebuggerUrl) throw new Error("target not ready");
      return target;
    }, "target");
    const pageCdp = new Cdp(new WebSocket(list.webSocketDebuggerUrl));
    await new Promise((resolve, reject) => {
      pageCdp.ws.addEventListener("open", resolve);
      pageCdp.ws.addEventListener("error", reject);
    });
    await pageCdp.send("Runtime.enable");
    await pageCdp.send("Page.enable");
    const done = await waitFor(async () => {
      const evaluated = await pageCdp.send("Runtime.evaluate", {
        expression: "window.__done ? JSON.stringify(window.__done) : ''",
        returnByValue: true,
      });
      if (evaluated.exceptionDetails) throw new Error(JSON.stringify(evaluated.exceptionDetails));
      const value = evaluated.result && evaluated.result.value;
      if (!value) throw new Error("pending");
      return JSON.parse(value);
    }, "test", 200);
    if (done.failures && done.failures.length) {
      console.error(JSON.stringify(done, null, 2));
      process.exitCode = 1;
    } else {
      console.log(JSON.stringify(done));
    }
    browser.ws.close();
    pageCdp.ws.close();
  } finally {
    chrome.kill("SIGKILL");
    web.close();
    fs.rmSync(pagePath, { force: true });
    fs.rmSync(profile, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(err);
  fs.rmSync(pagePath, { force: true });
  process.exit(1);
});
