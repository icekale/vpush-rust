const MARKET_LABELS = { cn: "A股", hk: "港股", us: "美股" };
const MARKET_CURRENCIES = { CNY: "¥", HKD: "HK$", USD: "$" };
const STATUS_LABELS = { live: "交易中", closed: "休市", stale: "更新延迟", unavailable: "暂不可用" };
const DELIVERY_LABELS = {
  sent: "已发送", failed: "发送失败", suppressed: "已抑制", pending: "发送中",
};
const A500_DATA_URL = "https://icekale.github.io/a500/vpush_data.json";
const A500_DATA_MAX_AGE_MS = 48 * 60 * 60 * 1000;
const MARKET_TEMPERATURE_MAX_AGE_MS = 24 * 60 * 60 * 1000;
const TEMPERATURE_LABELS = { cold: "偏冷", normal: "正常", hot: "偏热", overheated: "过热", unavailable: "暂不可用" };

const isNumber = value => typeof value === "number" && Number.isFinite(value);
const escapeFallback = value => String(value ?? "").replace(/[&<>\"']/g, char => ({
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
}[char]));

function decimalPlaces(value) {
  if (!isNumber(value) || value <= 0) return 2;
  const text = String(value).toLowerCase();
  if (text.includes("e-")) return Math.min(4, Number(text.split("e-")[1]));
  return Math.min(4, Math.max(0, (text.split(".")[1] || "").length));
}

function number(value, tickSize) {
  return isNumber(value) ? value.toLocaleString("en-US", { minimumFractionDigits: Math.max(2, decimalPlaces(tickSize)), maximumFractionDigits: 4 }) : "--";
}

function percent(value) {
  if (!isNumber(value)) return "--";
  return `${value > 0 ? "+" : ""}${value.toFixed(2)}%`;
}

function tone(value) {
  return isNumber(value) ? (value > 0 ? "positive" : value < 0 ? "negative" : "flat") : "flat";
}

function currencyLabel(currency) {
  return MARKET_CURRENCIES[currency] || currency || "--";
}

function quoteTime(value) {
  if (!value) return "暂无报价时间";
  const date = new Date(value);
  return Number.isFinite(date.getTime())
    ? new Intl.DateTimeFormat("zh-CN", { timeZone: "Asia/Shanghai", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(date) + "（北京时间）"
    : "报价时间未知";
}

function statusLabel(status) {
  return STATUS_LABELS[status] || "状态未知";
}

function deliveryLabel(status) {
  return DELIVERY_LABELS[status] || status || "暂无记录";
}

function parseRealtimeData(text) {
  const match = String(text || "").match(/window\.__RT\s*=\s*(\{.*?\})\s*;/s);
  if (!match) throw new Error("数据格式无效");
  return JSON.parse(match[1]);
}

function normalizeMarketSnapshot(payload, now = new Date()) {
  if (!payload || !isNumber(payload.temperature)) return { available: false, reason: "数据格式无效" };
  const updated = payload.ts ? new Date(payload.ts.replace(" ", "T") + "+08:00") : null;
  const age = updated && Number.isFinite(updated.getTime()) ? now.getTime() - updated.getTime() : Infinity;
  const valid = [payload.temperature, payload.price, payload.pePercentile, payload.pricePercentile].every(isNumber);
  const available = valid && payload.fresh === true && payload.delayed !== true && age >= 0 && age <= MARKET_TEMPERATURE_MAX_AGE_MS;
  const status = payload.temperature < 30 ? "cold" : payload.temperature < 60 ? "normal" : payload.temperature < 80 ? "hot" : "overheated";
  return {
    ...payload,
    available,
    age,
    temperature_status: status,
    temperatureLabel: TEMPERATURE_LABELS[status],
    market_status: payload.market ? "live" : "closed",
    pe: payload.pe,
    pe_percentile: payload.pePercentile,
    price_percentile: payload.pricePercentile,
    change: payload.change,
    updatedLabel: updated && Number.isFinite(updated.getTime()) ? quoteTime(updated.toISOString()) : "更新时间未知",
    reason: available ? "" : age > MARKET_TEMPERATURE_MAX_AGE_MS || payload.delayed ? "数据延迟" : "暂不可用",
  };
}

function renderMarketTemperatureMarkup(snapshot) {
  if (!snapshot?.available) return `<div class="watch-temperature-unavailable" data-watch-temperature-state><span class="watch-status watch-status-unavailable">${escapeFallback(snapshot?.reason || "暂不可用")}</span><p>${escapeFallback(snapshot?.reason === "数据延迟" ? "a500 数据超过可接受时效，暂不展示旧读数" : "a500 数据暂时不可用，暂不展示未经核验的读数")}</p></div>`;
  const marker = Math.max(0, Math.min(100, Number(snapshot.temperature)));
  return `<div class="watch-temperature-card" data-watch-temperature-state>
    <div class="watch-temperature-hero"><div><span class="watch-temperature-kicker">中国A500</span><strong class="watch-temperature-value">${snapshot.temperature}<small>°</small></strong><span class="watch-temperature-label">${escapeFallback(snapshot.temperatureLabel)}</span></div><div class="watch-temperature-meta"><span class="watch-status watch-status-${snapshot.market_status === "live" ? "live" : "stale"}">${snapshot.market_status === "live" ? "交易中" : "收盘"}</span><span>${escapeFallback(snapshot.updatedLabel)}</span></div></div>
    <div class="watch-temperature-scale" aria-label="市场温度 ${snapshot.temperature} 度"><span class="watch-temperature-marker" style="left:${marker}%"></span><span class="watch-temperature-scale-label">偏冷</span><span class="watch-temperature-scale-label">正常</span><span class="watch-temperature-scale-label">偏热</span></div>
    <div class="watch-temperature-metrics"><div><span>PE-TTM</span><strong>${number(snapshot.pe)}</strong></div><div><span>PE 分位</span><strong>${number(snapshot.pe_percentile)}%</strong></div><div><span>价格分位</span><strong>${number(snapshot.price_percentile)}%</strong></div><div><span>指数涨跌</span><strong class="${tone(snapshot.change)}">${percent(snapshot.change)}</strong></div><div><span>股债利差</span><strong>${number(snapshot.stockYield)}%</strong></div><div><span>定投比例</span><strong>${number(snapshot.dcaPct)}%</strong></div></div>
    <div class="watch-a500-columns"><div><h4>温度历史</h4>${historyMarkup(snapshot.temperature_history, "temp", "°")}</div><div><h4>PE 历史</h4>${historyMarkup(snapshot.pe_history, "pe", "")}</div></div>
    <div class="watch-temperature-source">数据源：a500 · ${escapeFallback(snapshot.dcaLabel || "定投策略数据") } · 更新于 ${escapeFallback(snapshot.updatedLabel)}</div>
  </div>`;
}
function normalizeA500Data(payload, now = new Date()) {
  if (!payload || payload.schema_version !== 1 || !payload.a500 || !payload.dividend || !payload.transition) return { available: false, reason: "数据格式无效" };
  const generated = payload.generated_at ? new Date(payload.generated_at) : null;
  const age = generated && Number.isFinite(generated.getTime()) ? now.getTime() - generated.getTime() : Infinity;
  const dataFresh = age >= 0 && age <= A500_DATA_MAX_AGE_MS;
  const a500 = normalizeMarketSnapshot(payload.a500, now);
  return {
    available: dataFresh,
    generated_at: payload.generated_at,
    age,
    a500: { ...a500, available: a500.available && dataFresh, reason: dataFresh ? a500.reason : "数据延迟" },
    dividend: payload.dividend || {},
    transition: payload.transition || {},
    reason: dataFresh ? a500.reason : "数据延迟",
  };
}

function historyMarkup(items, valueKey, suffix = "") {
  const rows = Array.isArray(items) ? items.slice(-8) : [];
  if (!rows.length) return `<p class="watch-a500-muted">暂无历史数据</p>`;
  const values = rows.map(row => Number(row[valueKey])).filter(Number.isFinite);
  const max = Math.max(...values, 1);
  return `<div class="watch-a500-history">${rows.map(row => {
    const value = Number(row[valueKey]);
    const width = Number.isFinite(value) ? Math.max(5, Math.min(100, value / max * 100)) : 5;
    return `<div class="watch-a500-history-row"><span>${escapeFallback(row.d || row.date || "--")}</span><i><b style="width:${width}%"></b></i><strong>${Number.isFinite(value) ? value.toFixed(2) : "--"}${suffix}</strong></div>`;
  }).join("")}</div>`;
}

function renderDividendPanel(data) {
  const d = data.dividend || {};
  const metrics = [["PE", d.pe, "", d.pe_date], ["PE 分位", d.pe_percentile, "%", "估值位置"], ["股息率", d.dividend_yield, "%", d.dividend_yield_date], ["股息率分位", d.dividend_yield_percentile, "%", "股息位置"], ["PB", d.pb, "", ""], ["ROE", d.roe, "%", ""], ["10Y 国债", d.bond_yield, "%", ""], ["股债利差", d.spread, "%", ""], ["ETF 价格", d.etf_price, "", d.etf_update_time], ["ETF 溢价", d.etf_premium, "%", ""], ["ETF 成交额", d.etf_volume, "", ""], ["ETF 规模", d.etf_size, "", ""]];
  return `<div class="watch-a500-panel-body"><div class="watch-a500-score"><div><span class="watch-a500-kicker">红利低波温度</span><strong>${escapeFallback(d.light_score ?? "--")}</strong><span>${escapeFallback(d.light_label || "暂无评级")}</span></div><span class="watch-status watch-status-live">${escapeFallback(d.update_time || "数据日期未知")}</span></div><div class="watch-a500-metrics">${metrics.map(([label, value, suffix, date]) => `<div><span>${label}</span><strong>${isNumber(value) ? number(value) + suffix : "--"}</strong><small>${escapeFallback(date || "")}</small></div>`).join("")}</div><div class="watch-a500-columns"><div><h4>红利率历史</h4>${historyMarkup(d.dividend_history, "v", "%")}</div><div><h4>红利温度历史</h4>${historyMarkup(d.temp_history, "t", "°")}</div></div><p class="watch-a500-source">数据源：a500 红利低波数据 · 权重：${escapeFallback(d.light_weights || "--")} · ${escapeFallback(d.trend_reason || "趋势状态未知")}</p></div>`;
}

function renderTransitionPanel(data) {
  const t = data.transition || {};
  const lines = Object.values(t.lineScores || {});
  const indicators = Array.isArray(t.indicators) ? t.indicators : Object.values(t.indicators || {});
  return `<div class="watch-a500-panel-body"><div class="watch-a500-score"><div><span class="watch-a500-kicker">经济健康温度</span><strong>${isNumber(t.temperature) ? t.temperature.toFixed(1) : "--"}</strong><span>${escapeFallback(t.band || "暂无区间")}</span></div><span class="watch-a500-emoji">${escapeFallback(t.emoji || "")}</span></div><p class="watch-a500-description">${escapeFallback(t.description || "暂无说明")}</p><div class="watch-a500-line-grid">${lines.map(line => `<div><span>${escapeFallback(line.name)}</span><strong>${isNumber(line.score) ? line.score.toFixed(1) : "--"}</strong><small>权重 ${isNumber(line.weight) ? line.weight.toFixed(1) : "--"}%</small></div>`).join("")}</div><div class="watch-a500-indicators"><h4>指标明细</h4>${indicators.map(item => `<div class="watch-a500-indicator"><div><strong>${escapeFallback(item.name || "未命名指标")}</strong><span>${escapeFallback(item.label || "")}</span></div><strong>${isNumber(item.score) ? item.score.toFixed(0) : "--"}</strong><small>${isNumber(item.value) ? item.value : "--"} · ${escapeFallback(item.date || "日期未知")} · ${escapeFallback(item.source || "来源未知")}${item.stale ? ` · 数据较旧${isNumber(item.stale_days) ? ` ${item.stale_days} 天` : ""}` : ""}</small></div>`).join("")}</div><p class="watch-a500-source">数据日期：${escapeFallback(t.date || "未知")} · 指标来源与更新时间随原项目数据同步。</p></div>`;
}

function a500UnavailableMarkup(reason = "暂不可用") {
  return `<div class="watch-a500-unavailable"><span class="watch-status watch-status-unavailable">${escapeFallback(reason)}</span><p>自有 A500 数据源暂时不可用，暂不展示旧读数。</p></div>`;
}

function renderA500Panels(data) {
  const available = Boolean(data?.available);
  const safe = data || {};
  const temperature = available ? renderMarketTemperatureMarkup(safe.a500) : a500UnavailableMarkup(safe.reason);
  const dividend = available ? renderDividendPanel(safe) : a500UnavailableMarkup(safe.reason);
  const macro = available ? renderTransitionPanel(safe) : a500UnavailableMarkup(safe.reason);
  return `<section class="section-panel watch-a500 watch-panel" id="watch-panel-temperature" role="tabpanel" aria-labelledby="watch-tab-temperature" data-watch-panel="temperature" hidden><div class="section-head"><h3 class="section-title">市场温度</h3><p class="section-meta">中国A500 · 不展示过期读数</p></div><div data-a500-temperature>${temperature}</div></section>
    <section class="section-panel watch-a500 watch-panel" id="watch-panel-dividend" role="tabpanel" aria-labelledby="watch-tab-dividend" data-watch-panel="dividend" hidden><div class="section-head"><h3 class="section-title">红利低波</h3><p class="section-meta">估值、股息与 ETF 数据</p></div><div data-a500-dividend>${dividend}</div></section>
    <section class="section-panel watch-a500 watch-panel" id="watch-panel-macro" role="tabpanel" aria-labelledby="watch-tab-macro" data-watch-panel="macro" hidden><div class="section-head"><h3 class="section-title">宏观数据</h3><p class="section-meta">经济健康 · 指标明细</p></div><div data-a500-macro>${macro}</div></section>`;
}
function targetValue(alert) {
  return alert?.target == null ? "" : String(alert.target);
}

export function alertPayloadFromValues(aboveTarget, aboveEnabled, belowTarget, belowEnabled) {
  const parseTarget = value => value === "" || value == null ? null : Number(value);
  return {
    above: { target: parseTarget(aboveTarget), enabled: Boolean(aboveEnabled) },
    below: { target: parseTarget(belowTarget), enabled: Boolean(belowEnabled) },
  };
}

function alertRuleMarkup(item, rule, label, escapeHtml) {
  const alert = item.alerts?.[rule] || {};
  const currency = currencyLabel(item.currency);
  const inputId = `watch-${item.market}-${item.symbol}-${rule}`.replace(/[^a-zA-Z0-9_-]/g, "-");
  const delivery = alert.last_triggered_at
    ? `${deliveryLabel(alert.delivery_status)} · ${quoteTime(alert.last_triggered_at)}`
    : deliveryLabel(alert.delivery_status);
  return `<div class="watch-rule" data-rule="${rule}">
    <div class="watch-rule-heading"><strong>${label}</strong><span class="watch-delivery">${escapeHtml(delivery)}</span></div>
    <div class="watch-rule-controls">
      <label class="watch-target-label" for="${inputId}">${escapeHtml(currency)}<span class="sr-only">目标价</span></label>
      <input id="${inputId}" class="form-control watch-target" type="number" min="0" step="any" inputmode="decimal" placeholder="目标价" value="${escapeHtml(targetValue(alert))}" data-target-input="${rule}">
      <label class="switch watch-enabled" for="${inputId}-enabled"><input id="${inputId}-enabled" type="checkbox" ${alert.enabled ? "checked" : ""} data-enabled-input="${rule}"><span class="track"></span><span>启用</span></label>
    </div>
  </div>`;
}

function itemMarkup(item, escapeHtml) {
  const status = item.status || "unavailable";
  const hasPrice = isNumber(item.price) && status !== "unavailable" && status !== "stale";
  const hasPercent = isNumber(item.percent) && status !== "unavailable" && status !== "stale";
  const quoteText = hasPrice
    ? `${currencyLabel(item.currency)} ${number(item.price, item.tick_size)}`
    : "--";
  const changeText = hasPercent ? percent(item.percent) : "--";
  const statusDetail = item.error && status === "unavailable" ? item.error : statusLabel(status);
  const key = `${item.market}:${item.symbol}`;
  return `<article class="watch-item" data-watch-key="${escapeHtml(key)}">
    <div class="watch-item-top">
      <div class="watch-identity"><strong>${escapeHtml(item.name || item.symbol)}</strong><span>${escapeHtml(item.symbol)} · ${escapeHtml(MARKET_LABELS[item.market] || item.market || "市场未知")}</span></div>
      <button type="button" class="watch-delete" data-delete-watch="${escapeHtml(key)}" aria-label="删除 ${escapeHtml(item.name || item.symbol)}">删除</button>
    </div>
    <div class="watch-quote" data-quote>
      <div class="watch-price"><span class="watch-price-value ${hasPrice ? "" : "is-unavailable"}">${escapeHtml(quoteText)}</span><span class="watch-quote-label">${escapeHtml(item.currency || "--")}</span></div>
      <div class="watch-change ${tone(item.percent)}"><span>${escapeHtml(changeText)}</span><span>涨跌幅</span></div>
      <div class="watch-state"><span class="watch-status watch-status-${escapeHtml(status)}">${escapeHtml(statusDetail)}</span><span>${escapeHtml(item.quoted_at ? `报价 · ${quoteTime(item.quoted_at)}` : "报价时间未知")}</span></div>
    </div>
    <form class="watch-rules" data-alert-form="${escapeHtml(key)}">
      ${alertRuleMarkup(item, "above", "上穿提醒", escapeHtml)}
      ${alertRuleMarkup(item, "below", "下穿提醒", escapeHtml)}
      <div class="watch-rule-actions"><span class="watch-rule-note">价格单位：${escapeHtml(item.currency || "未知货币")}；首次有效报价只建立基线，穿越目标价才通知，回到另一侧后可再次触发。</span><button type="submit" class="btn-ghost watch-save">保存提醒</button></div>
      <p class="watch-form-error" data-form-error role="alert"></p>
    </form>
  </article>`;
}

export { normalizeA500Data, normalizeMarketSnapshot, parseRealtimeData, renderA500Panels, renderMarketTemperatureMarkup };

export function renderWatchlistMarkup({ items = [], loading = false, error = "", a500Data = null, marketTemperature = null, escapeHtml: escape = escapeFallback } = {}) {
  const list = Array.isArray(items) ? items : [];
  const listHtml = loading
    ? `<div class="watch-state-panel" role="status"><strong>正在加载自选股</strong><span>从服务器读取最新缓存报价…</span></div>`
    : error
      ? `<div class="watch-state-panel watch-state-error" role="alert"><strong>自选股暂时无法加载</strong><span>${escape(error)}</span><button type="button" class="btn-ghost" data-retry-watch>重试</button></div>`
      : list.length
        ? `<div class="watch-list">${list.map(item => itemMarkup(item, escape)).join("")}</div>`
        : `<div class="watch-state-panel"><strong>还没有自选股</strong><span>用上方搜索添加 A股、港股或美股标的。</span></div>`;
  return `<div class="watchlist-page">
    <header class="watchlist-intro"><div><h2 class="section-title">自选股</h2><p class="section-meta">行情与市场研究</p></div></header>
    <div class="settings-tabs watch-workspace-tabs" role="tablist" aria-label="自选股板块">
      <button type="button" class="settings-tab" role="tab" id="watch-tab-temperature" aria-selected="false" aria-controls="watch-panel-temperature" tabindex="-1" data-watch-tab="temperature">市场温度</button>
      <button type="button" class="settings-tab" role="tab" id="watch-tab-dividend" aria-selected="false" aria-controls="watch-panel-dividend" tabindex="-1" data-watch-tab="dividend">红利低波</button>
      <button type="button" class="settings-tab" role="tab" id="watch-tab-macro" aria-selected="false" aria-controls="watch-panel-macro" tabindex="-1" data-watch-tab="macro">宏观数据</button>
      <button type="button" class="settings-tab active" role="tab" id="watch-tab-watchlist" aria-selected="true" aria-controls="watch-panel-watchlist" data-watch-tab="watchlist">自选股</button>
    </div>
    <div data-watch-a500-content>${renderA500Panels(a500Data || (marketTemperature ? { available: marketTemperature.available, a500: marketTemperature } : null))}</div>
    <section class="watch-panel" id="watch-panel-watchlist" role="tabpanel" aria-labelledby="watch-tab-watchlist" data-watch-panel="watchlist">
    <div class="watchlist-intro-actions"><span class="watch-fetch-time" data-watch-fetched>上次获取时间：--</span><a class="watch-settings-link" href="/settings" data-spa-link>推送设置</a></div>
    <section class="section-panel watch-search-panel" aria-labelledby="watch-search-title">
      <div class="section-head"><h3 class="section-title" id="watch-search-title">添加标的</h3><p class="section-meta">按市场搜索代码或名称，搜索结果来自可用数据源。</p></div>
      <form class="watch-search-form" data-watch-search-form><label class="sr-only" for="watch-market">市场</label><select id="watch-market" class="form-control" data-watch-market><option value="cn">A股</option><option value="hk">港股</option><option value="us">美股</option></select><label class="sr-only" for="watch-search">股票名称或代码</label><input id="watch-search" class="form-control" type="search" autocomplete="off" placeholder="股票名称或代码" data-watch-search><button type="submit" class="btn-normal">搜索</button></form>
      <p class="watch-search-note">支持按股票名称或代码搜索；搜索结果来自可用数据源。</p><div class="watch-search-results" data-watch-search-results aria-live="polite"></div>
    </section>
    <section class="watchlist-section" aria-labelledby="watch-items-title"><div class="watch-section-heading"><h3 class="section-title" id="watch-items-title">我的自选</h3><span class="watch-list-state" data-watch-list-state>${loading ? "加载中" : error ? "加载失败" : `${list.length} 个标的`}</span></div><div data-watch-list>${listHtml}</div></section>
    </section>
  </div>`;
}

export function createWatchlistView({ api, escapeHtml = escapeFallback, setPageTitle, go, flash, routeStillActive, currentRouteSeq, fetchSnapshot = globalThis.fetch } ) {
  const escape = escapeHtml;
  let active = false;
  let timer = null;
  let a500Timer = null;
  let host = null;
  let dirtyRules = new Set();
  let latestItems = [];
  let searchSeq = 0;
  let searchTimer = null;
  let requestSeq = 0;
  let actionSeq = 0;
  let lastFetchedAt = null;
  let temperatureRequest = 0;
  let removeListeners = () => {};

  const isCurrent = seq => active && routeStillActive(seq) && currentRouteSeq() === seq;
  const keyFor = item => `${item.market}:${item.symbol}`;

  function stop() {
    active = false;
    actionSeq += 1;
    requestSeq += 1;
    if (timer) clearInterval(timer);
    if (a500Timer) clearInterval(a500Timer);
    timer = null;
    a500Timer = null;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = null;
    searchSeq += 1;
    removeListeners();
    removeListeners = () => {};
    host = null;
    dirtyRules.clear();
  }

  function quoteMarkup(item) {
    const status = item.status || "unavailable";
    const hasPrice = isNumber(item.price) && status !== "unavailable" && status !== "stale";
    const hasPercent = isNumber(item.percent) && status !== "unavailable" && status !== "stale";
    const statusDetail = item.error && status === "unavailable" ? item.error : statusLabel(status);
    return `<div class="watch-price"><span class="watch-price-value ${hasPrice ? "" : "is-unavailable"}">${escape(hasPrice ? `${currencyLabel(item.currency)} ${number(item.price, item.tick_size)}` : "--")}</span><span class="watch-quote-label">${escape(item.currency || "--")}</span></div><div class="watch-change ${tone(item.percent)}"><span>${escape(hasPercent ? percent(item.percent) : "--")}</span><span>涨跌幅</span></div><div class="watch-state"><span class="watch-status watch-status-${escape(status)}">${escape(statusDetail)}</span><span>${escape(item.quoted_at ? `报价 · ${quoteTime(item.quoted_at)}` : "报价时间未知")}</span></div>`;
  }

  function actionIsCurrent(seq, action) {
    return isCurrent(seq) && action === actionSeq;
  }

  function setFetchedTime() {
    const node = host?.querySelector("[data-watch-fetched]");
    if (node) node.textContent = `上次获取时间：${lastFetchedAt ? quoteTime(lastFetchedAt) : "--"}`;
  }

  function updateQuotes(items, failed = false) {
    if (!host) return;
    const byKey = new Map(items.map(item => [keyFor(item), item]));
    host.querySelectorAll("[data-quote]").forEach(node => {
      const key = node.closest("[data-watch-key]")?.dataset.watchKey;
      const item = byKey.get(key) || (failed ? { market: key?.split(":")[0], symbol: key?.split(":").slice(1).join(":"), status: "unavailable", error: "自选列表更新失败" } : null);
      if (item) node.innerHTML = quoteMarkup(item);
    });
    const state = host.querySelector("[data-watch-list-state]");
    if (state) state.textContent = failed ? "更新失败" : `${items.length} 个标的`;
  }

  function preserveDirtyForms() {
    const preserved = new Map();
    host?.querySelectorAll("[data-alert-form]").forEach(form => {
      const key = form.dataset.alertForm;
      if (!dirtyRules.has(key)) return;
      preserved.set(key, {
        aboveTarget: form.querySelector('[data-target-input="above"]')?.value ?? "",
        aboveEnabled: form.querySelector('[data-enabled-input="above"]')?.checked === true,
        belowTarget: form.querySelector('[data-target-input="below"]')?.value ?? "",
        belowEnabled: form.querySelector('[data-enabled-input="below"]')?.checked === true,
        pending: form.querySelector(".watch-save")?.disabled === true,
      });
    });
    return preserved;
  }

  function restoreDirtyForms(preserved) {
    preserved.forEach((values, key) => {
      const form = host?.querySelector(`[data-alert-form="${CSS.escape(key)}"]`);
      if (!form) return;
      form.querySelector('[data-target-input="above"]').value = values.aboveTarget;
      form.querySelector('[data-enabled-input="above"]').checked = values.aboveEnabled;
      form.querySelector('[data-target-input="below"]').value = values.belowTarget;
      form.querySelector('[data-enabled-input="below"]').checked = values.belowEnabled;
      if (values.pending) form.querySelectorAll("input, button").forEach(node => { node.disabled = true; });
    });
  }

  function bindRuleDirty(form, on) {
    const key = form.dataset.alertForm;
    form.querySelectorAll("input").forEach(input => {
      const markDirty = () => dirtyRules.add(key);
      on(input, "input", markDirty);
      on(input, "change", markDirty);
    });
    on(form, "submit", event => saveRules(event, form));
  }

  async function saveRules(event, form) {
    event.preventDefault();
    const key = form.dataset.alertForm;
    const [market, ...symbolParts] = key.split(":");
    const symbol = symbolParts.join(":");
    const aboveTarget = form.querySelector('[data-target-input="above"]')?.value ?? "";
    const belowTarget = form.querySelector('[data-target-input="below"]')?.value ?? "";
    const payload = alertPayloadFromValues(aboveTarget, form.querySelector('[data-enabled-input="above"]')?.checked, belowTarget, form.querySelector('[data-enabled-input="below"]')?.checked);
    const seq = currentRouteSeq();
    const action = actionSeq;
    if (Object.values(payload).some(rule => (rule.enabled && rule.target === null) || (rule.target !== null && (!Number.isFinite(rule.target) || rule.target <= 0)))) {
      setFormError(form, "请输入大于零的目标价；启用提醒时必须填写目标价");
      return;
    }
    dirtyRules.add(key);
    const button = form.querySelector(".watch-save");
    setFormError(form, "");
    form.querySelectorAll("input, button").forEach(node => { node.disabled = true; });
    button?.setAttribute("aria-busy", "true");
    try {
      await api(`/api/me/watchlist/${encodeURIComponent(market)}/${encodeURIComponent(symbol)}/alerts`, { method: "PUT", body: JSON.stringify(payload) });
      if (!actionIsCurrent(seq, action)) return;
      dirtyRules.delete(key);
      await refresh(seq);
      if (actionIsCurrent(seq, action)) flash?.("提醒已保存", "success");
    } catch (error) {
      if (actionIsCurrent(seq, action)) setFormError(host?.querySelector(`[data-alert-form="${CSS.escape(key)}"]`) || form, error.message || "保存失败，请稍后重试");
    } finally {
      if (actionIsCurrent(seq, action)) {
        const currentForm = host?.querySelector(`[data-alert-form="${CSS.escape(key)}"]`) || form;
        currentForm.querySelectorAll("input, button").forEach(node => { node.disabled = false; });
        currentForm.querySelector(".watch-save")?.removeAttribute("aria-busy");
      }
    }
  }

  function setFormError(form, message) {
    const node = form.querySelector("[data-form-error]");
    if (node) node.textContent = message || "";
  }

  async function addItem(market, symbol, button) {
    const seq = currentRouteSeq();
    const action = actionSeq;
    button.disabled = true;
    try {
      await api("/api/me/watchlist", { method: "POST", body: JSON.stringify({ market, symbol }) });
      if (!actionIsCurrent(seq, action)) return;
      await refresh(seq, true);
      if (!actionIsCurrent(seq, action)) return;
      renderSearchResults([], "");
      flash?.("已加入自选股", "success");
    } catch (error) {
      if (!actionIsCurrent(seq, action)) return;
      const result = host?.querySelector("[data-watch-search-results]");
      if (result) result.innerHTML = `<p class="watch-search-error" role="alert">${escape(error.message || "添加失败，请稍后重试")}</p>`;
    } finally {
      button.disabled = false;
    }
  }

  async function deleteItem(key, button) {
    const [market, ...symbolParts] = key.split(":");
    const symbol = symbolParts.join(":");
    const seq = currentRouteSeq();
    const action = actionSeq;
    button.disabled = true;
    try {
      await api(`/api/me/watchlist/${encodeURIComponent(market)}/${encodeURIComponent(symbol)}`, { method: "DELETE" });
      if (!actionIsCurrent(seq, action)) return;
      dirtyRules.delete(key);
      await refresh(seq, true);
      if (actionIsCurrent(seq, action)) flash?.("已移除自选股", "success");
    } catch (error) {
      if (!actionIsCurrent(seq, action)) return;
      button.disabled = false;
      flash?.(error.message || "删除失败，请稍后重试", "error");
    }
  }

  function renderSearchResults(items, error) {
    const result = host?.querySelector("[data-watch-search-results]");
    if (!result) return;
    if (error) { result.innerHTML = `<p class="watch-search-error" role="alert">${escape(error)}</p>`; return; }
    if (!items.length) { result.textContent = ""; return; }
    result.innerHTML = `<div class="watch-search-result-list">${items.map(item => `<div class="watch-search-result"><div><strong>${escape(item.name || item.symbol)}</strong><span>${escape(item.symbol)} · ${escape(MARKET_LABELS[item.market] || item.market)} · ${escape(item.currency || "货币未知")}</span></div><button type="button" class="btn-ghost" data-add-watch="${escape(`${item.market}:${item.symbol}`)}">加入</button></div>`).join("")}</div>`;
    result.querySelectorAll("[data-add-watch]").forEach(button => button.addEventListener("click", () => {
      const [market, ...parts] = button.dataset.addWatch.split(":");
      addItem(market, parts.join(":"), button);
    }));
  }

  async function search() {
    const input = host?.querySelector("[data-watch-search]");
    const market = host?.querySelector("[data-watch-market]")?.value;
    const query = input?.value.trim() || "";
    const seq = ++searchSeq;
    if (!query) { renderSearchResults([], ""); return; }
    renderSearchResults([], "正在搜索…");
    try {
      const data = await api(`/api/market/symbol-search?market=${encodeURIComponent(market)}&q=${encodeURIComponent(query)}`);
      if (!active || seq !== searchSeq) return;
      renderSearchResults(Array.isArray(data.items) ? data.items : [], "");
      if (!data.items?.length) renderSearchResults([], "没有找到可添加的标的");
    } catch (error) {
      if (active && seq === searchSeq) renderSearchResults([], error.message || "搜索失败，请稍后重试");
    }
  }

  function bindEvents(seq) {
    const listeners = [];
    removeListeners();
    const on = (node, event, handler) => { node?.addEventListener(event, handler); if (node) listeners.push(() => node.removeEventListener(event, handler)); };
    const searchForm = host.querySelector("[data-watch-search-form]");
    on(searchForm, "submit", event => { event.preventDefault(); search(); });
    on(host.querySelector("[data-watch-search]"), "input", () => {
      searchSeq += 1;
      if (searchTimer) clearTimeout(searchTimer);
      searchTimer = setTimeout(search, 220);
    });
    on(host.querySelector("[data-watch-market]"), "change", () => { searchSeq += 1; search(); });
    host.querySelectorAll("[data-delete-watch]").forEach(button => on(button, "click", () => deleteItem(button.dataset.deleteWatch, button)));
    host.querySelectorAll("[data-alert-form]").forEach(form => bindRuleDirty(form, on));
    on(host.querySelector("[data-retry-watch]"), "click", () => refresh(seq, true));
    const tabs = [...host.querySelectorAll("[data-watch-tab]")];
    const selectTab = button => {
      tabs.forEach(tab => {
        const selected = tab === button;
        tab.classList.toggle("active", selected);
        tab.setAttribute("aria-selected", String(selected));
        tab.tabIndex = selected ? 0 : -1;
      });
      host.querySelectorAll("[data-watch-panel]").forEach(panel => { panel.hidden = panel.dataset.watchPanel !== button.dataset.watchTab; });
    };
    tabs.forEach((tab, index) => {
      on(tab, "click", () => selectTab(tab));
      on(tab, "keydown", event => {
        const next = event.key === "ArrowRight" ? index + 1 : event.key === "ArrowLeft" ? index - 1 : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : null;
        if (next === null) return;
        event.preventDefault();
        const target = tabs[(next + tabs.length) % tabs.length];
        selectTab(target);
        target.focus();
      });
    });
    removeListeners = () => { listeners.forEach(remove => remove()); };
  }

  function updateA500(data) {
    const content = host?.querySelector("[data-watch-a500-content]");
    if (!content) return;
    const available = Boolean(data?.available);
    const safe = data || {};
    const sections = {
      temperature: available ? renderMarketTemperatureMarkup(safe.a500) : a500UnavailableMarkup(safe.reason),
      dividend: available ? renderDividendPanel(safe) : a500UnavailableMarkup(safe.reason),
      macro: available ? renderTransitionPanel(safe) : a500UnavailableMarkup(safe.reason),
    };
    Object.entries(sections).forEach(([name, markup]) => {
      const node = content.querySelector(`[data-a500-${name}]`);
      if (node) node.innerHTML = markup;
    });
  }

  async function loadA500(seq) {
    const request = ++temperatureRequest;
    const content = host?.querySelector("[data-watch-a500-content]");
    if (!content || typeof fetchSnapshot !== "function") return;
    try {
      const response = await fetchSnapshot(`${A500_DATA_URL}?v=${Date.now()}`, { cache: "no-store" });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const data = normalizeA500Data(await response.json());
      if (isCurrent(seq) && request === temperatureRequest) updateA500(data);
    } catch {
      if (isCurrent(seq) && request === temperatureRequest) updateA500({ available: false, reason: "暂不可用" });
    }
  }

  async function refresh(seq, forceRender = false) {
    if (!isCurrent(seq)) return;
    const request = ++requestSeq;
    try {
      const data = await api("/api/me/watchlist");
      if (!isCurrent(seq) || request !== requestSeq) return;
      const items = Array.isArray(data.items) ? data.items : [];
      latestItems = items;
      lastFetchedAt = new Date().toISOString();
      setFetchedTime();
      if (forceRender || !dirtyRules.size || !host.querySelector("[data-watch-list]")) {
        const preserved = preserveDirtyForms();
        host.querySelector("[data-watch-list]").innerHTML = items.length ? `<div class="watch-list">${items.map(item => itemMarkup(item, escape)).join("")}</div>` : `<div class="watch-state-panel"><strong>还没有自选股</strong><span>用上方搜索添加 A股、港股或美股标的。</span></div>`;
        host.querySelector("[data-watch-list-state]").textContent = `${items.length} 个标的`;
        bindEvents(seq);
        restoreDirtyForms(preserved);
      } else {
        updateQuotes(items);
      }
    } catch (error) {
      if (!isCurrent(seq) || request !== requestSeq) return;
      latestItems = [];
      lastFetchedAt = new Date().toISOString();
      setFetchedTime();
      if (!dirtyRules.size || forceRender) {
        host.querySelector("[data-watch-list]").innerHTML = `<div class="watch-state-panel watch-state-error" role="alert"><strong>自选股暂时无法加载</strong><span>${escape(error.message || "服务器暂时不可用")}</span><button type="button" class="btn-ghost" data-retry-watch>重试</button></div>`;
        host.querySelector("[data-watch-list-state]").textContent = "加载失败";
        bindEvents(seq);
      } else updateQuotes([], true);
    }
  }

  async function renderWatchlist(seq) {
    stop();
    active = true;
    setPageTitle("行情");
    host = document.querySelector("#main");
    if (!host) return;
    host.innerHTML = renderWatchlistMarkup({ loading: true, escapeHtml });
    bindEvents(seq);
    loadA500(seq);
    a500Timer = setInterval(() => loadA500(seq), 300000);
    a500Timer.unref?.();
    await refresh(seq);
    if (!isCurrent(seq)) return;
    timer = setInterval(() => refresh(seq), 30000);
    timer.unref?.();
  }

  return { renderWatchlist, stopWatchlist: stop, getLatestItems: () => latestItems };
}
