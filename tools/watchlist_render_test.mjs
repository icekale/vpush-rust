import assert from "node:assert/strict";
import test from "node:test";
import { alertPayloadFromValues, createWatchlistView, normalizeA500Data, normalizeMarketSnapshot, parseRealtimeData, renderA500Panels, renderMarketTemperatureMarkup, renderWatchlistMarkup } from "../static-assets/views/watchlist.js";

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

test("parses the published a500 realtime payload", () => {
  const payload = parseRealtimeData('window.__RT = {"temperature":41,"fresh":true};');
  assert.equal(payload.temperature, 41);
  assert.equal(payload.fresh, true);
});
test("renders all A500 project sections from the owned aggregate", () => {
  const data = normalizeA500Data({
    schema_version: 1,
    generated_at: "2026-10-01T05:06:27Z",
    a500: { temperature: 41, price: 5370.3, pe: 16.31, pePercentile: 17.2, pricePercentile: 68.4, fresh: true, delayed: false, market: false, ts: "2026-10-01 12:04:49", temperature_history: [{ date: "2026-10-01", value: 41 }], pe_history: [{ date: "2026-10-01", value: 16.31 }] },
    dividend: { light_score: 74, light_label: "偏冷", pe: 8.24, dividend_yield: 4.39, spread: 1.2, etf_premium: -0.4, temp_history: [{ date: "2026-10-01", value: 52 }] },
    transition: { date: "2026-09-29", temperature: 61.6, band: "温和区", indicators: [{ name: "就业", score: 70 }] },
  }, new Date("2026-10-01T05:10:00Z"));
  assert.equal(data.a500.available, true);
  const html = renderA500Panels(data);
  assert.match(html, /市场温度/);
  assert.match(html, /红利低波/);
  assert.match(html, /宏观数据/);
  assert.match(html, /role="tabpanel"/);
  assert.match(html, /watch-panel-temperature/);
  assert.match(html, /就业/);
  assert.match(html, /74/);
  const unavailableTabs = renderA500Panels({ available: false, reason: "暂不可用" });
  assert.match(unavailableTabs, /市场温度/);
  assert.match(unavailableTabs, /红利低波/);
  assert.match(unavailableTabs, /宏观数据/);
});

test("research tabs render full trends, ETF state, and grouped macro indicators", () => {
  const data = normalizeA500Data({
    schema_version: 1, generated_at: "2026-10-01T05:06:27Z",
    a500: {
      temperature: 41, price: 5370.3, change: 0.19, pe: 16.31, pePercentile: 17.2,
      pricePercentile: 68.4, stockYield: 6.1, bondYield: 1.68, dcaPct: 75,
      dcaLabel: "正常定投", market: false, fresh: true, delayed: false, ts: "2026-10-01 12:04:49",
      temperature_history: [
        { date: "2026-09-25", temp: 30 }, { date: "2026-09-26", temp: 35 },
        { date: "2026-09-27", temp: 41 },
      ],
      pe_history: [{ date: "2026-09-25", pe: 15 }, { date: "2026-09-27", pe: 16.31 }],
    },
    dividend: {
      light_score: 74, light_label: "适合买入", etf_price: null, etf_update_time: null,
      dividend_yield: 4.39, pe: 8.24, temp_history: [
        { d: "2026-09-25", t: 70 }, { d: "2026-09-26", t: 72 }, { d: "2026-09-27", t: 74 },
      ],
      dividend_history: [{ d: "2026-09-25", v: 4.1 }, { d: "2026-09-27", v: 4.39 }],
    },
    transition: {
      temperature: 61.6, band: "温和区", lineScores: {
        line1: { name: "内需与物价", score: 65.5, weight: 30 },
        line2: { name: "就业与收入", score: 39.3, weight: 15 },
        line3: { name: "货币与信用", score: 39.8, weight: 25 },
        line4: { name: "转型与开放", score: 87.2, weight: 30 },
      },
      indicators: { cpi: { name: "CPI同比", line: 1, value: 1.2, score: 70, label: "温和", source: "<unsafe>", date: "2026-05-31", stale: true, stale_days: 121 } },
    },
  }, new Date("2026-10-01T05:10:00Z"));
  const config = { indicators: { cpi: { unit: "%", reference_range: "0.5%~3.0%" } } };
  const html = renderA500Panels(data, config);
  assert.match(html, /5,370\.30/);
  assert.match(html, /正常定投/);
  assert.match(html, /data-watch-trend="temperature"[\s\S]*?<polyline/);
  assert.match(html, /2026-09-25/);
  assert.match(html, /2026-09-27/);
  assert.match(html, /data-watch-trend="pe"/);
  assert.match(html, /data-watch-trend="dividend"/);
  assert.match(html, /data-watch-trend="dividend-temperature"/);
  assert.match(html, /563020/);
  assert.match(html, /暂无行情/);
  for (const name of ["内需与物价", "就业与收入", "货币与信用", "转型与开放"]) assert.match(html, new RegExp(name));
  assert.match(html, /1\.2%/);
  assert.match(html, /0\.5%~3\.0%/);
  assert.match(html, /数据较旧/);
  assert.match(html, /&lt;unsafe&gt;/);
  assert.doesNotMatch(html, /<unsafe>/);
  const noConfig = renderA500Panels(data);
  assert.match(noConfig, /CPI同比/);
  assert.match(noConfig, /1\.2/);
  assert.doesNotMatch(noConfig, /0\.5%~3\.0%/);
  data.a500.temperature_history = [];
  assert.match(renderMarketTemperatureMarkup(data.a500), /暂无历史数据/);
});

test("watchlist has four peer tabs, defaulting to the watchlist and keeping other panels hidden", () => {
  const html = renderWatchlistMarkup({ loading: true });
  assert.deepEqual([...html.matchAll(/data-watch-tab="([^"]+)"/g)].map(match => match[1]), ["temperature", "dividend", "macro", "watchlist"]);
  assert.equal((html.match(/role="tabpanel"/g) || []).length, 4);
  assert.match(html, /data-watch-panel="temperature" hidden/);
  assert.match(html, /data-watch-panel="dividend" hidden/);
  assert.match(html, /data-watch-panel="macro" hidden/);
  assert.match(html, /aria-selected="true"[^>]*data-watch-tab="watchlist"/);
  assert.match(html, /data-watch-panel="watchlist"[^>]*>[\s\S]*data-watch-search-form[\s\S]*data-watch-list/);
  assert.doesNotMatch(html, /data-a500-tab=/);
});

test("market snapshot renders a fresh A500 temperature card and rejects stale data", () => {
  const snapshot = normalizeMarketSnapshot({
    temperature: 41, price: 5370.3, pe: 16.31, pePercentile: 17.2, pricePercentile: 68.4,
    market: false, fresh: true, delayed: false, change: 0.19, dcaLabel: "正常定投", ts: "2026-10-01 12:04:49",
  }, new Date("2026-10-01T04:05:00Z"));
  assert.equal(snapshot.available, true);
  const html = renderMarketTemperatureMarkup(snapshot);
  assert.match(html, /41/);
  assert.match(html, /中国A500/);
  assert.match(html, /watch-temperature-primary[\s\S]*watch-temperature-quote/);
  assert.match(html, /正常定投/);
  assert.equal((html.match(/>收盘</g) || []).length, 1);
  assert.equal(html.split(snapshot.updatedLabel).length - 1, 1);
  assert.doesNotMatch(html, /更新于/);

  const stale = normalizeMarketSnapshot({ temperature: 41, price: 5370.3, pe: 16.31, pePercentile: 17.2, pricePercentile: 68.4, market: false, fresh: true, delayed: false, change: 0.19, ts: "2026-09-30 12:04:49" }, new Date("2026-10-01T04:05:00Z"));
  assert.equal(stale.available, false);
  assert.match(renderMarketTemperatureMarkup(stale), /数据延迟/);
});


test("slow or failed macro config does not hide the main A500 data", async () => {
  const originalDocument = globalThis.document;
  const nodes = new Map(["[data-watch-a500-content]", "[data-a500-temperature]", "[data-a500-dividend]", "[data-a500-macro]", "[data-watch-list]", "[data-watch-list-state]", "[data-watch-fetched]"].map(key => [key, { innerHTML: "", textContent: "" }]));
  const content = nodes.get("[data-watch-a500-content]");
  content.querySelector = key => nodes.get(key);
  const host = { innerHTML: "", querySelector: key => nodes.get(key), querySelectorAll: () => [] };
  globalThis.document = { querySelector: selector => selector === "#main" ? host : null };
  let rejectConfig;
  const payload = {
    schema_version: 1, generated_at: new Date().toISOString(),
    a500: { temperature: 41, price: 5370.3, pe: 16.31, pePercentile: 17, pricePercentile: 60, fresh: true, delayed: false, ts: new Date().toISOString() },
    dividend: { light_score: 74 }, transition: { temperature: 61.6, indicators: { cpi: { name: "CPI", line: 1, value: 1.2 } }, lineScores: { line1: { name: "内需与物价" } } },
  };
  const view = createWatchlistView({
    api: async () => ({ items: [] }), setPageTitle() {}, currentRouteSeq: () => 1, routeStillActive: () => true,
    fetchSnapshot: url => url.includes("transition_config") ? new Promise((_, reject) => { rejectConfig = reject; }) : Promise.resolve({ ok: true, json: async () => payload }),
  });
  try {
    await view.renderWatchlist(1);
    await new Promise(resolve => setTimeout(resolve, 0));
    assert.match(nodes.get("[data-a500-dividend]").innerHTML, /74/);
    rejectConfig(new Error("offline"));
    await new Promise(resolve => setTimeout(resolve, 0));
    assert.match(nodes.get("[data-a500-macro]").innerHTML, /CPI/);
  } finally {
    view.stopWatchlist();
    globalThis.document = originalDocument;
  }
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
    fetchSnapshot: () => Promise.reject(new Error("test")),
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
