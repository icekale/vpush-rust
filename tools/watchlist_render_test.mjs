import assert from "node:assert/strict";
import test from "node:test";
import { alertPayloadFromValues, createWatchlistView, renderWatchlistMarkup } from "../static-assets/views/watchlist.js";

test("watchlist renders loading, empty, unavailable, and zero states", () => {
  assert.match(renderWatchlistMarkup({ loading: true }), /正在加载自选股/);
  assert.match(renderWatchlistMarkup({ items: [] }), /还没有自选股/);
  const html = renderWatchlistMarkup({ items: [{
    market: "cn", symbol: "600519", name: "贵州茅台", currency: "CNY", tick_size: 0.01,
    price: 0, percent: 0, quoted_at: "2026-10-01T01:02:03Z", status: "live",
    alerts: { above: { target: 0, enabled: true, delivery_status: "pending" }, below: { target: null, enabled: false, delivery_status: "sent" } },
  }] });
  assert.match(html, /¥ 0\.00/);
  assert.match(html, /0\.00%/);
  assert.match(html, /step="any"/);
  const stale = renderWatchlistMarkup({ items: [{
    market: "us", symbol: "AAPL", name: "Apple", currency: "USD", price: 123.45, percent: 1.2,
    quoted_at: "2026-10-01T01:02:03Z", status: "stale", error: "delayed", alerts: {},
  }] });
  assert.match(stale, /watch-status-stale/);
  assert.match(stale, />--<\/span>/);
  const unavailable = renderWatchlistMarkup({ items: [], error: "<bad>" });
  assert.match(unavailable, /&lt;bad&gt;/);
});

test("mounted view renders escaped live data and ignores a departed route response", async () => {
  const originalDocument = globalThis.document;
  const nodes = new Map(["[data-watch-list]", "[data-watch-list-state]", "[data-watch-fetched]"].map(key => [key, { innerHTML: "", textContent: "" }]));
  const host = { innerHTML: "", querySelector: key => nodes.get(key) || null, querySelectorAll: () => [] };
  globalThis.document = { querySelector: selector => selector === "#main" ? host : null };
  let seq = 1;
  let finishRequest;
  let deferred = false;
  const view = createWatchlistView({
    api: () => deferred ? new Promise(resolve => { finishRequest = resolve; }) : Promise.resolve({ items: [{
      market: "hk", symbol: "00700", name: "腾讯<&控股", currency: "HKD", tick_size: 0.001,
      price: 0.249, percent: 0, status: "closed", alerts: {},
    }] }),
    setPageTitle() {}, currentRouteSeq: () => seq, routeStillActive: value => value === seq,
  });
  try {
    await view.renderWatchlist(1);
    const html = nodes.get("[data-watch-list]").innerHTML;
    assert.match(html, /腾讯&lt;&amp;控股/);
    assert.match(html, /HK\$ 0\.249/);
    assert.doesNotMatch(html, /%u[0-9A-F]{4}/i);
    deferred = true;
    const pending = view.renderWatchlist(1);
    view.stopWatchlist();
    seq = 2;
    const before = nodes.get("[data-watch-list]").innerHTML;
    finishRequest({ items: [{ market: "us", symbol: "SHOULD-NOT-RENDER", alerts: {} }] });
    await pending;
    assert.equal(nodes.get("[data-watch-list]").innerHTML, before);
  } finally {
    view.stopWatchlist();
    globalThis.document = originalDocument;
  }
});

test("alert payload preserves zero and clears blank targets", () => {
  assert.deepEqual(alertPayloadFromValues("0", true, "", false), {
    above: { target: 0, enabled: true },
    below: { target: null, enabled: false },
  });
});
