const MARKET_LABELS = { cn: "A股", hk: "港股", us: "美股" };
const MARKET_CURRENCIES = { CNY: "¥", HKD: "HK$", USD: "$" };
const STATUS_LABELS = { live: "交易中", closed: "休市", stale: "更新延迟", unavailable: "暂不可用" };
const DELIVERY_LABELS = {
  sent: "已发送", failed: "发送失败", suppressed: "已抑制", pending: "发送中",
};
const A500_DATA_URL = "https://icekale.github.io/a500/vpush_data.json";
const A500_CONFIG_URL = "https://icekale.github.io/a500/transition_config.json";
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
    stockYield: payload.stockYield,
    bondYield: payload.bondYield,
    dividendYield: payload.dividendYield,
    premium: isNumber(payload.premium) ? payload.premium : isNumber(payload.stockYield) && isNumber(payload.bondYield) ? payload.stockYield - payload.bondYield : null,
    spread: isNumber(payload.premium) ? payload.premium : isNumber(payload.stockYield) && isNumber(payload.bondYield) ? payload.stockYield - payload.bondYield : null,
    priceStale: payload.priceStale === true,
    priceFallback: payload.priceFallback === true,
    updatedLabel: updated && Number.isFinite(updated.getTime()) ? quoteTime(updated.toISOString()) : "更新时间未知",
    reason: available ? "" : age > MARKET_TEMPERATURE_MAX_AGE_MS || payload.delayed ? "数据延迟" : "暂不可用",
  };
}

function guideValue(value, suffix = "") {
  return isNumber(value) ? `${number(value)}${suffix}` : "--";
}

function renderTemperatureGuide(snapshot) {
  const s = snapshot || {};
  const pe = guideValue(s.pe);
  const stockYield = guideValue(isNumber(s.stockYield) ? s.stockYield : (isNumber(s.pe) && s.pe > 0 ? 100 / s.pe : null), "%");
  const bondYield = guideValue(s.bondYield, "%");
  const dividendYield = guideValue(s.dividendYield, "%");
  const spread = guideValue(s.spread, "%");
  return `<details class="watch-a500-guide">
    <summary>指标参考指南</summary>
    <div class="watch-a500-guide-body">
      <section><h4>温度</h4><div class="watch-a500-guide-table"><div><span>低于 30°C</span><b class="positive">可以买入</b></div><div><span>30–60°C</span><b>持有不动</b></div><div><span>60–80°C</span><b class="warning">关注，可减仓</b></div><div><span>高于 80°C</span><b class="negative">停止买入</b></div></div></section>
      <section><h4>回本年限（PE）</h4><div class="watch-a500-guide-table"><div><span>低于 13 年</span><b class="positive">可买入</b></div><div><span>13–15 年</span><b>持有不动</b></div><div><span>高于 15 年</span><b class="warning">谨慎</b></div><div><span>高于 17 年</span><b class="negative">避免买入</b></div></div><p>当前 <strong>${pe}</strong></p></section>
      <section><h4>盈利收益率（1 ÷ PE）</h4><p>假设 500 家公司利润不变且全部分红，每年能拿回投入的百分比，主要用来和国债利率对比。</p><p>当前 <strong>${stockYield}</strong></p></section>
      <section><h4>10 年国债利率</h4><p>中国政府债券的年利率，近似“无风险收益”基准。越低，股票的相对吸引力越高。</p><p>当前 <strong>${bondYield}</strong></p></section>
      <section><h4>股息率</h4><div class="watch-a500-guide-table"><div><span>高于 3%</span><b class="positive">分红不错，有安全垫</b></div><div><span>2–3%</span><b>中等，一般</b></div><div><span>低于 2%</span><b>偏低，分红吸引力不足</b></div></div><p>当前 <strong>${dividendYield}</strong></p></section>
      <section><h4>股票 vs 国债（ERP）</h4><div class="watch-a500-guide-table"><div><span>高于 3%</span><b class="positive">股票明显更划算</b></div><div><span>1.5–3%</span><b>还可以，中等</b></div><div><span>0–1.5%</span><b class="warning">吸引力一般</b></div><div><span>低于 0</span><b class="negative">买债券比股票好</b></div></div><p>当前 <strong>${spread}</strong></p></section>
      <section><h4>指标之间的关系</h4><ul><li><b>温度</b> = PE 分位 × 60% + 价格分位 × 40%</li><li><b>回本年限（PE）</b>就是估值本身</li><li><b>盈利收益率</b> = 1 ÷ PE，用来和国债利率对比</li><li><b>股债溢价</b> = 盈利收益率 − 国债利率</li><li><b>股息率</b> = 成分股分红 ÷ 指数价格</li></ul></section>
    </div>
  </details>`;
}

function metricInfo(title, body) {
  return `<details class="watch-a500-metric-info"><summary>${escapeFallback(title)}</summary><p>${escapeFallback(body)}</p></details>`;
}

function dividendGuide(d) {
  const spread = isNumber(d.spread) ? `${number(d.spread)}%` : "--";
  return `<details class="watch-a500-guide"><summary>💡 红利低波参考指南</summary><div class="watch-a500-guide-body">
    <section><h4>🚦 红绿灯理解</h4><p>🟢 <b>适合买入</b>：股息率高 + PE低 + 没溢价，三重信号共振。适合分批建仓。</p><p>🟡 <b>适合持有</b>：当前性价比中等。已持有的继续拿分红，等更好的买点再加仓。</p><p>🟡 <b>🔒 趋势保护</b>：PE从近期高点回撤超过10%时强制黄色，避免单边下跌中无脑接盘。</p><p>🔴 <b>观望/减仓</b>：股息率不够吸引人或PE太高，耐心等待。</p><p class="watch-a500-muted">公式：综合 = 股息率分位得分 × 60% + PE分位反向得分 × 30% + 折溢价得分 × 10%（连续打分，无断点跳跃）。</p></section>
    <section><h4>📊 各因子含义</h4><div class="watch-a500-guide-table"><div><span>股息率分位</span><b>历史位置，高=性价比高</b></div><div><span>PE分位</span><b>历史位置，低=便宜，反向打分</b></div><div><span>折溢价</span><b>ETF价格 vs 净值，溢价高不划算</b></div></div></section>
    <section><h4>💰 股息率 vs 国债利差</h4><p>红利低波的安全垫来自股息率 − 10年国债利率。当前利差约 <b>${spread}</b>，利差越高，票息优势越突出。</p></section>
    <section><h4>⏰ 什么时候卖？</h4><p>这是配置型品种，正常长期持有吃分红；股息率跌破3.5%且 PE 分位超过80%同时出现时，再考虑减仓。</p></section>
    <section><h4>🔗 相关指标</h4><p><b>PE</b> = 指数价格 ÷ 成分股总利润；<b>PB</b> = 指数价格 ÷ 成分股净资产；<b>ROE</b> = 净利润 ÷ 净资产；ETF规模越大通常流动性越好。</p></section>
  </div></details>`;
}

function dividendFactor(label, value, tag, toneName = "") {
  const width = isNumber(value) ? Math.max(0, Math.min(100, Math.abs(value))) : 0;
  return `<div class="watch-dividend-factor"><span>${escapeFallback(label)}</span><strong class="${toneName}">${escapeFallback(isNumber(value) ? `${value}${label === "折溢价" ? "%" : "%"}` : "--")}</strong><b class="${toneName}">${escapeFallback(tag)}</b><i><em class="${toneName}" style="width:${width}%"></em></i></div>`;
}

function dividendMetric(label, value, desc, info) {
  return `<div class="watch-dividend-metric"><div><span>${escapeFallback(label)}</span><strong>${escapeFallback(value)}</strong></div><span class="watch-dividend-metric-desc">${escapeFallback(desc)}</span>${metricInfo("i", info)}</div>`;
}
function renderMarketTemperatureMarkup(snapshot) {
  if (!snapshot?.available) return `<div class="watch-temperature-unavailable" data-watch-temperature-state><span class="watch-status watch-status-unavailable">${escapeFallback(snapshot?.reason || "暂不可用")}</span><p>${escapeFallback(snapshot?.reason === "数据延迟" ? "a500 数据超过可接受时效，暂不展示旧读数" : "a500 数据暂时不可用，暂不展示未经核验的读数")}</p></div>`;
  const marker = Math.max(0, Math.min(100, Number(snapshot.temperature)));
  return `<div class="watch-temperature-card" data-watch-temperature-state>
    <div class="watch-temperature-hero"><div class="watch-temperature-primary"><div class="watch-temperature-score"><strong class="watch-temperature-value">${snapshot.temperature}<small>°</small></strong><span class="watch-temperature-label">${escapeFallback(snapshot.temperatureLabel)}</span></div><span class="watch-temperature-advice">${escapeFallback(snapshot.dcaLabel || "定投策略数据")}</span></div><div class="watch-temperature-quote"><span class="watch-temperature-kicker">中国A500 · 沪深全指</span><div class="watch-temperature-index-row"><strong class="watch-temperature-index">${number(snapshot.price)}</strong><span class="watch-temperature-change ${tone(snapshot.change)}">${percent(snapshot.change)}</span></div><div class="watch-temperature-meta"><span class="watch-status watch-status-${snapshot.market_status === "live" ? "live" : "stale"}">${snapshot.market_status === "live" ? "交易中" : "收盘"}</span><span>${escapeFallback(snapshot.updatedLabel)}</span></div></div></div>
    <div class="watch-temperature-scale" aria-label="市场温度 ${snapshot.temperature} 度"><span class="watch-temperature-marker" style="left:${marker}%"></span><span class="watch-temperature-scale-label">低估</span><span class="watch-temperature-scale-label">合理</span><span class="watch-temperature-scale-label">偏高</span><span class="watch-temperature-scale-label">过热</span></div>
    <div class="watch-temperature-legend"><span><i class="positive"></i>0–30°C · 适合买入</span><span><i></i>30–60°C · 继续定投</span><span><i class="warning"></i>60–80°C · 暂停买入</span><span><i class="negative"></i>80–100°C · 考虑卖出</span></div>
    <div class="watch-temperature-metrics">
      <div><span>回本年限（PE）</span><strong>${number(snapshot.pe)} 年</strong>${metricInfo("i", "PE 是指数价格 ÷ 成分股利润，表示假设利润不变时的回本年限。低于13年偏便宜，高于17年偏贵。")}</div>
      <div><span>盈利收益率</span><strong>${number(snapshot.stockYield)}%</strong>${metricInfo("i", "盈利收益率约等于 1 ÷ PE，用来和10年国债利率比较。")}</div>
      <div><span>10年国债</span><strong>${number(snapshot.bondYield)}%</strong>${metricInfo("i", "中国政府债券的年利率，近似无风险收益基准。")}</div>
      <div><span>股息率</span><strong>${number(snapshot.dividendYield)}%</strong>${metricInfo("i", "成分股分红 ÷ 指数价格，反映现金分红的安全垫。")}</div>
      <div><span>股债利差</span><strong>${number(snapshot.spread)}%</strong></div>
      <div><span>定投比例</span><strong>${number(snapshot.dcaPct)}%</strong></div>
    </div>
    <div class="watch-a500-columns"><div>${trendBlock(snapshot.temperature_history, "temp", "temperature", "温度走势（0–100）", "°")}</div><div>${trendBlock(snapshot.pe_history, "pe", "pe", "PE-TTM 走势")}</div></div>
    ${renderTemperatureGuide(snapshot)}
    <div class="watch-temperature-source">数据源：a500 · ${escapeFallback(snapshot.dcaLabel || "定投策略数据")} · ${snapshot.priceFallback ? "点位使用最近收盘" : snapshot.priceStale ? "点位略有延迟" : snapshot.market_status === "live" ? "盘中实时" : "盘后数据"} · PE截至 ${escapeFallback(snapshot.pe_last_calibrated || "估值日期未知")} · 仅供参考</div>
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

function trendMarkup(items, valueKey, dataName, label, suffix = "") {
  const rows = (Array.isArray(items) ? items : []).map(row => ({
    date: row?.d || row?.date || "--", value: Number(row?.[valueKey]),
  })).filter(row => Number.isFinite(row.value));
  if (!rows.length) return `<p class="watch-a500-muted">暂无历史数据</p>`;
  const values = rows.map(row => row.value);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const spread = max - min || 1;
  const points = rows.map((row, index) => `${(index / Math.max(1, rows.length - 1) * 100).toFixed(2)},${(94 - (row.value - min) / spread * 82).toFixed(2)}`).join(" ");
  const first = rows[0];
  const last = rows.at(-1);
  return `<div class="watch-a500-trend" data-watch-trend="${escapeFallback(dataName)}">
    <svg viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label="${escapeFallback(label)}，${escapeFallback(first.date)} 至 ${escapeFallback(last.date)}，${number(last.value)}${escapeFallback(suffix)}">
      <path class="watch-a500-trend-grid" d="M0 12H100M0 53H100M0 94H100"></path><polyline points="${points}"></polyline>
    </svg><div class="watch-a500-trend-meta"><span>${escapeFallback(first.date)} · ${number(first.value)}${escapeFallback(suffix)}</span><span>${escapeFallback(last.date)} · ${number(last.value)}${escapeFallback(suffix)}</span></div>
  </div>`;
}

function trendBlock(items, valueKey, dataName, title, suffix = "") {
  return `<div class="watch-a500-trend-block"><h4>${escapeFallback(title)}</h4>${trendMarkup(items, valueKey, dataName, title, suffix)}</div>`;
}

function renderDividendPanel(data) {
  const d = data.dividend || {};
  const light = d.trend_blocked ? "yellow" : d.light || "yellow";
  const lightIcon = light === "green" ? "🟢" : light === "red" ? "🔴" : "🟡";
  const lightLabel = `${d.trend_blocked ? "🔒 " : ""}${d.light_label || "暂无评级"}`;
  const etfPrice = isNumber(d.etf_price) ? number(d.etf_price) : "--";
  const premium = isNumber(d.etf_premium) ? `${d.etf_premium >= 0 ? "+" : ""}${number(d.etf_premium)}%` : "--";
  const volume = isNumber(d.etf_volume) ? d.etf_volume >= 1e8 ? `${number(d.etf_volume / 1e8)}亿` : `${number(d.etf_volume / 1e4)}万` : "--";
  const size = isNumber(d.etf_size) ? `${number(d.etf_size)}亿` : "--";
  const dp = isNumber(d.dividend_yield_percentile) ? d.dividend_yield_percentile : null;
  const pp = isNumber(d.pe_percentile) ? d.pe_percentile : null;
  const pm = isNumber(d.etf_premium) ? d.etf_premium : null;
  const dpTone = dp >= 80 ? "positive" : dp >= 50 ? "warning" : "negative";
  const ppTone = pp < 30 ? "positive" : pp < 60 ? "warning" : "negative";
  const pmTone = pm <= 0.3 ? "positive" : pm <= 0.8 ? "warning" : "negative";
  const pe = isNumber(d.pe) ? number(d.pe) : "--";
  const pb = isNumber(d.pb) ? number(d.pb) : "--";
  const roe = isNumber(d.roe) ? `${number(d.roe)}%` : "--";
  const peDesc = pp == null ? "--" : `${pp}% ${pp >= 80 ? "偏高" : pp >= 50 ? "中等" : "偏低"}`;
  const pbDesc = !isNumber(d.pb) ? "--" : d.pb < 0.7 ? "偏低" : d.pb < 1.2 ? "正常" : "偏高";
  const roeDesc = !isNumber(d.roe) ? "--" : d.roe >= 15 ? "优秀" : d.roe >= 8 ? "中等" : "偏低";
  const sizeDesc = !isNumber(d.etf_size) ? "--" : d.etf_size > 50 ? "充裕" : d.etf_size > 10 ? "正常" : "偏小";
  const spreadStatus = d.spread >= 3 ? "高位✅" : d.spread >= 2 ? "中等" : d.spread == null ? "--" : "偏低⚠️";
  return `<div class="watch-a500-panel-body watch-dividend-panel">
    <div class="watch-a500-decision"><div class="watch-dividend-light watch-dividend-light-${light}"><span>${lightIcon}</span><strong>${escapeFallback(lightLabel)}</strong><small>综合 ${isNumber(d.light_score) ? Math.round(d.light_score) : "--"} 分</small></div><div class="watch-a500-etf"><span>563020 易方达红利低波</span><strong>${etfPrice}</strong><small>${escapeFallback(d.etf_update_time || d.update_time || "数据日期未知")} · ${d.etf_price == null ? "暂无实时行情" : "ETF 行情"}</small></div></div>
    ${d.trend_blocked ? `<p class="watch-dividend-trend-warning">🔒 ${escapeFallback(d.trend_reason || "趋势保护已触发")}</p>` : ""}
    <div class="watch-dividend-etf-meta"><span>折溢率 <b>${premium}</b></span><span>成交量 <b>${volume}</b></span><span>规模 <b>${size}</b></span><span>股息率 <b>${isNumber(d.dividend_yield) ? `${number(d.dividend_yield)}%` : "--"}</b></span><span>股息率分位 <b>${isNumber(dp) ? `${number(dp)}%` : "--"}</b></span><span>股息率 vs 国债 <b>${isNumber(d.spread) ? `${number(d.spread)}%` : "--"} · ${spreadStatus}</b></span></div>
    <div class="watch-dividend-factors">${dividendFactor("股息率分位", dp, dp >= 80 ? "高" : dp >= 50 ? "中" : "低", dpTone)}${dividendFactor("PE分位", pp, pp < 30 ? "低" : pp < 60 ? "中" : "偏高", ppTone)}${dividendFactor("折溢价", pm, pm == null ? "--" : pm <= 0 ? "折价" : pm <= 0.3 ? "正常" : "偏高", pmTone)}</div>
    <div class="watch-dividend-metrics">${dividendMetric("市盈率", pe, peDesc, "PE 是指数价格除以成分股总利润，红利低波通常在6–10倍。")}${dividendMetric("市净率", pb, pbDesc, "PB 是指数价格除以成分股净资产，反映资产估值。")}${dividendMetric("ROE", roe, roeDesc, "ROE 是净利润除以净资产，衡量成分股盈利能力。")}${dividendMetric("ETF规模", size, sizeDesc, "规模越大通常流动性越好，日成交低于5000万时注意滑点。")}</div>
    <div class="watch-a500-columns"><div>${trendBlock(d.dividend_history, "v", "dividend", "股息率走势", "%")}</div><div>${trendBlock(d.temp_history, "t", "dividend-temperature", "温度走势（基于PE分位）", "°")}</div></div>
    ${dividendGuide(d)}<p class="watch-a500-source">数据源：a500 红利低波数据 · 权重：${escapeFallback(d.light_weights || "6:3:1")} · ${escapeFallback(d.trend_reason || "趋势状态未知")} · 股息率实时，PE/估值可能滞后 · 仅供参考</p>
  </div>`;
}

const TRANSITION_INFO = {
  cpi: ["CPI同比（居民消费价格）", "今年这个月买东西比去年同月贵了多少。", "国家统计局在全国各地采价，对比今年和去年同期的价格变化。", "0.5-3%为温和通胀。太低说明没人消费，太高说明钱不值钱。", "CPI温和正增长说明内需正常。"],
  ppi: ["PPI同比（工业生产者出厂价格）", "工厂把货卖给下游时的价格，比去年同月贵/便宜了多少。", "国家统计局收集全国工业企业的出厂价。", "-1%到3%。低于-2%说明工业需求不足。", "PPI回升说明工业需求在修复。"],
  pmi: ["制造业PMI", "每个月对全国采购经理做问卷，问\"你这个月生意比上个月好还是差\"，汇总成一个指数。", "扩散指数。50是荣枯线——>50说明大部分人觉得生意在变好。", ">52说明景气向好，48-50偏弱。", "PMI在扩张区间意味着制造业整体稳定。"],
  electricity: ["全社会用电量同比", "今年这个月全国用了多少电，比去年同月多了还是少了。", "电表读数没法作假，是衡量真实经济活力最硬的指标。", "2-8%。", "只要电表在转，经济就在运转。"],
  retail_sales: ["社会消费品零售总额同比", "今年这个月全国卖了多少东西，比去年同月多还是少。", "", "3-10%为正常增长。负增长说明消费收缩。", "社零是内需最直接的指标。"],
  unemployment: ["城镇调查失业率", "全国城镇劳动力中，正在找工作但没找到的人占多少比例。", "", "4.5-5.5%为正常。", "失业率是民生指标的核心。"],
  disposable_income: ["居民可支配收入累计同比", "居民拿到手可以花的钱，比去年多了还是少了。", "", "4-7%为正常增长。", "收入增速跟不上GDP，内需就很难起来。"],
  m2: ["M2同比", "全社会一共有多少钱。", "", "8-10%为适度。", "适度宽松的货币环境是企业正常经营的前提。"],
  rmb_loan: ["人民币贷款余额同比", "银行借出去的所有人民币贷款余额，比去年同月多了多少。", "", "8-14%为正常。", "贷款增速是\"宽信用\"的直接度量。"],
  bond_yield: ["10年国债收益率", "你买10年期国债，国家每年给你多少利息。", "", "1.8-3%。过低说明避险情绪高。", "低利率降低企业融资成本，但过低也反映经济信心不足。"],
  lpr: ["1年期LPR", "银行给最优质客户贷款的基准利率。", "", "3.0-4.5%。利率越低说明货币政策越宽松。", "LPR持续下调说明央行在主动宽松。"],
  industrial_output: ["规模以上工业增加值同比", "全国规模以上工业企业这个月生产了多少东西，比去年同月多了还是少了。", "", "5-8%为正常。", "工业增加值是实体经济供给端的核心指标。"],
  export: ["出口同比增速", "今年这个月卖到国外的商品总额，比去年同月多了还是少了。", "", "3-10%为正常。", "出口稳定说明制造业有竞争力。"],
  currency_index: ["人民币CFETS汇率指数", "人民币对一篮子货币的整体强弱。", "", "95-102为稳定。", "汇率稳定有利于进出口贸易。"],
  ai_market_share: ["中国AI模型海外调用份额", "全球开发者在OpenRouter平台上调用AI模型时，中国模型占了多少比例。", "", "", "这是新经济竞争力的直接体现。"],
  new_energy_penetration: ["新能源汽车渗透率", "全国新卖出的汽车中，新能源车占了多少比例。", "", "30-50%。越高说明新能源替代越快。", "这是中国新质生产力最具标志性的指标。"],
};

function transitionIndicatorInfo(key) {
  const info = TRANSITION_INFO[key];
  if (!info) return "";
  return `<details class="watch-transition-info"><summary>指标解读</summary><div><h5>${escapeFallback(info[0])}</h5>${["这是什么", key === "electricity" ? "为什么可靠" : "怎么算的", "正常范围", "反映的经济状况"].map((label, index) => info[index + 1] ? `<p><b>${label}：</b>${escapeFallback(info[index + 1])}</p>` : "").join("")}</div></details>`;
}

function transitionJudgment(key, score) {
  const judgments = {
    line1: ["需求不振", "内需偏弱", "内需正常"], line2: ["就业承压", "就业正常", "就业良好"],
    line3: ["信用偏紧", "货币适度", "信用扩张"], line4: ["动力不足", "转型推进", "转型加速"],
  };
  return judgments[key]?.[score < 40 ? 0 : score < 70 ? 1 : 2] || "";
}

function transitionTakeaway(lineScore, indicators) {
  if (lineScore >= 70) return "整体向好";
  const scored = indicators.filter(([, item]) => isNumber(item.score));
  const best = scored.reduce((a, b) => !a || b[1].score > a[1].score ? b : a, null)?.[1];
  const worst = scored.reduce((a, b) => !a || b[1].score < a[1].score ? b : a, null)?.[1];
  return [worst?.score < 40 ? `${worst.name}偏弱` : "", best?.score >= 70 ? `${best.name}有支撑` : ""].filter(Boolean).join("，") || "表现中等";
}

function renderTransitionGuide(transition, config) {
  const t = transition || {};
  const lines = config?.lines || {};
  const lineScores = t.lineScores || {};
  const indicators = Object.entries(t.indicators || {});
  const lineRows = ["line1", "line2", "line3", "line4"].map(key => {
    const line = lines[key] || {};
    const score = lineScores[key] || {};
    const weight = isNumber(score.weight) ? score.weight : isNumber(line.weight) ? line.weight * 100 : null;
    return `<div><span>${escapeFallback(line.emoji || "")} ${escapeFallback(score.name || line.name || key)}</span><b>${isNumber(weight) ? `${weight.toFixed(0)}%` : "--"}</b></div>`;
  }).join("");
  const indicatorRows = indicators.map(([key, item]) => `<div class="watch-transition-guide-indicator"><span><strong>${escapeFallback(item.name || key)}</strong> <small>${item.auto ? "✅自动" : "⚠️手动"}</small></span><span>当前 ${isNumber(item.value) ? item.value : "--"} → ${isNumber(item.score) ? Math.round(item.score) : "--"}分（${escapeFallback(item.label || "--")}）· 权重${isNumber(item.weight) ? Math.round(item.weight * 100) : "--"}%</span><small>${escapeFallback(item.source || "来源未知")} · 截至 ${escapeFallback(item.date || "未知")}${item.stale ? " · ⚠️过期" : ""}</small></div>`).join("");
  return `<details class="watch-a500-guide watch-transition-guide">
    <summary>经济健康指南 · 权重与解读</summary>
    <div class="watch-a500-guide-body">
      <section><h4>🌡️ 经济健康度怎么算？</h4><p>综合温度 = 各指标得分 × 权重的加权总和。四条主线：</p><div class="watch-a500-guide-table">${lineRows}</div><p>每个指标按当前值 vs 阈值映射到 0–100 分。</p></section>
      <section><h4>📊 温度区间解读</h4><div class="watch-a500-guide-table"><div><span>❄️ &lt; 40</span><b>低温区 · 整体偏冷</b></div><div><span>🌤️ 40–69</span><b>温和区 · 正常运转</b></div><div><span>🔥 ≥ 70</span><b>升温区 · 整体向好</b></div></div></section>
      <section class="watch-transition-guide-indicators"><h4>📈 各指标详情</h4>${indicatorRows || '<p>暂无指标数据</p>'}</section>
    </div>
  </details>`;
}

function renderTransitionPanel(data) {
  const t = data.transition || {};
  const config = data.transitionConfig || {};
  const configIndicators = config.indicators || {};
  const entries = Array.isArray(t.indicators) ? t.indicators.map((item, index) => [item.key || item.id || String(index), item]) : Object.entries(t.indicators || {});
  const lines = Object.entries(t.lineScores || {});
  if (!lines.length && entries.length) lines.push(["line1", { name: "指标明细" }]);
  const grouped = lines.map(([key, line]) => {
    const lineNumber = Number(String(key).replace("line", ""));
    return { key, line, indicators: entries.filter(([, item]) => Number(item.line) === lineNumber || (lineNumber === 1 && item.line == null)) };
  });
  const judgments = { line1: "内需", line2: "就业", line3: "信用", line4: "转型" };
  const overview = grouped.map(({ key, line, indicators }) => {
    const score = line.score;
    const best = indicators.filter(([, item]) => isNumber(item.score)).reduce((a, b) => !a || b[1].score > a[1].score ? b : a, null)?.[1];
    const worst = indicators.filter(([, item]) => isNumber(item.score)).reduce((a, b) => !a || b[1].score < a[1].score ? b : a, null)?.[1];
    const weight = isNumber(line.weight) ? `${Math.round(line.weight)}%` : "--";
    const short = name => String(name || "").replace("同比", "").replace("累计", "").replace("规模以上", "");
    return `<div class="watch-transition-line"><div><strong>${escapeFallback(line.name || judgments[key] || "未命名主线")}</strong><small>${weight}</small></div><b>${isNumber(score) ? Math.round(score) : "--"}</b><span>${escapeFallback(isNumber(score) ? transitionJudgment(key, score) : "暂无评级")}</span><div class="watch-transition-bar"><i style="width:${isNumber(score) ? Math.max(0, Math.min(100, score)) : 0}%"></i></div><small>${best && worst && best !== worst ? `▲ ${escapeFallback(short(best.name))} ${Math.round(best.score)} · ▼ ${escapeFallback(short(worst.name))} ${Math.round(worst.score)} · ` : ""}${escapeFallback(isNumber(score) ? transitionTakeaway(score, indicators) : "暂无说明")}</small></div>`;
  }).join("");
  const stale = entries.filter(([, item]) => item.stale).map(([, item]) => item.name);
  const manual = entries.filter(([, item]) => !item.auto).map(([, item]) => item.name);
  const warnings = [stale.length ? `数据过期(${stale.length}): ${stale.join("、")}` : "", manual.length ? `手动录入: ${manual.join("、")}` : ""].filter(Boolean);
  const sections = grouped.map(({ key, line, indicators }) => {
    const metaLine = config.lines?.[key] || {};
    const weight = isNumber(line.weight) ? line.weight.toFixed(1) : isNumber(metaLine.weight) ? (metaLine.weight * 100).toFixed(1) : "--";
    const items = indicators.map(([indicatorKey, item]) => {
      const meta = configIndicators[indicatorKey] || {};
      const unit = meta.unit || "";
      const range = meta.reference_range || "";
      return `<div class="watch-a500-indicator${item.stale ? " is-stale" : ""}"><div><strong>${escapeFallback(item.name || meta.name || "未命名指标")}</strong><span>${item.stale ? "⚠️过期" : item.auto ? "✅自动" : "⚠️手动"}</span></div><strong>${isNumber(item.value) ? item.value.toLocaleString("en-US", { maximumFractionDigits: 4 }) : "--"}${escapeFallback(unit)}</strong><small>${escapeFallback(item.label || "")} · ${isNumber(item.score) ? Math.round(item.score) : "--"}分 · ${range ? `参考 ${escapeFallback(range)} · ` : ""}${escapeFallback(item.date || "日期未知")} · ${escapeFallback(item.source || meta.data_source || "来源未知")}${item.stale ? ` · 数据较旧${isNumber(item.stale_days) ? ` ${item.stale_days} 天` : ""}` : ""}</small>${transitionIndicatorInfo(indicatorKey)}</div>`;
    }).join("");
    return `<section class="watch-a500-line-section"><div class="watch-a500-line-heading"><div><strong>${escapeFallback(metaLine.emoji || "")} ${escapeFallback(line.name || metaLine.name || "未命名主线")}</strong><span>权重 ${weight}% · ${escapeFallback(metaLine.description || "")}</span></div><b>${isNumber(line.score) ? line.score.toFixed(1) : "--"}</b></div><div class="watch-a500-indicators">${items || `<p class="watch-a500-muted">暂无该主线指标</p>`}</div></section>`;
  }).join("");
  return `<div class="watch-a500-panel-body"><div class="watch-a500-score"><div><span class="watch-a500-kicker">经济健康温度</span><strong>${isNumber(t.temperature) ? t.temperature.toFixed(1) : "--"}</strong><span>${escapeFallback(t.band || "暂无区间")}</span></div><span class="watch-a500-emoji">${escapeFallback(t.emoji || "")}</span></div><p class="watch-a500-description">${escapeFallback(t.description || "暂无说明")}</p><div class="watch-transition-overview">${overview}</div><div class="watch-a500-line-sections">${sections}</div>${warnings.length ? `<p class="watch-transition-warning">${warnings.map(escapeFallback).join("<br>")}</p>` : ""}${renderTransitionGuide(t, config)}<p class="watch-a500-source">数据日期：${escapeFallback(t.date || "未知")} · ✅ 自动 · ⚠️ 手动 · ⚠️过期 · 月频更新 · 仅供参考</p></div>`;
}

function a500UnavailableMarkup(reason = "暂不可用") {
  return `<div class="watch-a500-unavailable"><span class="watch-status watch-status-unavailable">${escapeFallback(reason)}</span><p>自有 A500 数据源暂时不可用，暂不展示旧读数。</p></div>`;
}

function renderA500Panels(data, transitionConfig = data?.transitionConfig || null) {
  const available = Boolean(data?.available);
  const safe = { ...(data || {}), transitionConfig: transitionConfig || data?.transitionConfig || {} };
  const temperature = available ? renderMarketTemperatureMarkup(safe.a500) : a500UnavailableMarkup(safe.reason);
  const dividend = available ? renderDividendPanel(safe) : a500UnavailableMarkup(safe.reason);
  const macro = available ? renderTransitionPanel(safe) : a500UnavailableMarkup(safe.reason);
  return `<section class="section-panel watch-a500 watch-panel" id="watch-panel-temperature" role="tabpanel" aria-labelledby="watch-tab-temperature" data-watch-panel="temperature" hidden><div class="section-head"><h3 class="section-title">市场温度</h3><p class="section-meta">中国A500 · 指数与估值趋势</p></div><div data-a500-temperature>${temperature}</div></section>
    <section class="section-panel watch-a500 watch-panel" id="watch-panel-dividend" role="tabpanel" aria-labelledby="watch-tab-dividend" data-watch-panel="dividend" hidden><div class="section-head"><h3 class="section-title">红利低波</h3><p class="section-meta">563020 · 股息、估值与趋势</p></div><div data-a500-dividend>${dividend}</div></section>
    <section class="section-panel watch-a500 watch-panel" id="watch-panel-macro" role="tabpanel" aria-labelledby="watch-tab-macro" data-watch-panel="macro" hidden><div class="section-head"><h3 class="section-title">宏观数据</h3><p class="section-meta">经济健康 · 四条主线</p></div><div data-a500-macro>${macro}</div></section>`;
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
      <div class="watch-search-results" data-watch-search-results aria-live="polite"></div>
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
      if (!data.available) return;
      const configResponse = await Promise.resolve(fetchSnapshot(`${A500_CONFIG_URL}?v=${Date.now()}`, { cache: "no-store" })).catch(() => null);
      if (configResponse?.ok) {
        const config = await configResponse.json().catch(() => null);
        if (config) {
          data.transitionConfig = config;
          if (isCurrent(seq) && request === temperatureRequest) updateA500(data);
        }
      }
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
