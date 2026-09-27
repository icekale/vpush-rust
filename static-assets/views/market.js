const GROUPS = {
  day: [["sh000001", "上证指数", "SH000001"], ["sz399001", "深证成指", "SZ399001"],
    ["sh000688", "科创50", "SH000688"], ["sz399006", "创业板", "SZ399006"],
    ["hkHSI", "恒生指数", "HSI"], ["hkHSTECH", "恒生科技", "HSTECH"]],
  night: [["us.INX", "标普 500 指数", "S&P 500"], ["us.IXIC", "纳斯达克指数", "NASDAQ"],
    ["us.NDX", "纳斯达克 100", "NDX"], ["us.DJI", "道琼斯指数", "DJIA"],
    ["usSOXX", "SOXX", "半导体 ETF"], ["usYINN", "YINN", "中国 3倍做多"]],
};
const number = value => value.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const signed = value => `${value > 0 ? "+" : ""}${number(value)}`;
const tone = value => value > 0 ? "positive" : value < 0 ? "negative" : "flat";
const timeFormat = new Intl.DateTimeFormat("zh-CN", {
  timeZone: "Asia/Shanghai", month: "2-digit", day: "2-digit",
  hour: "2-digit", minute: "2-digit", hourCycle: "h23",
});
const hourFormat = new Intl.DateTimeFormat("en-GB", { timeZone: "Asia/Shanghai", hour: "2-digit", hourCycle: "h23" });

function automaticGroup() {
  const hour = Number(hourFormat.format(new Date()));
  return hour >= 8 && hour < 20 ? "day" : "night";
}

export function createMarketView({ api, escapeHtml }) {
  let stop = () => {};
  let selection = "auto";

  function sparkline(item) {
    const series = item.intraday;
    if (!series?.points?.length || !Number.isFinite(item.previous_close)) return '<span class="market-spark-empty" aria-label="暂无当日分时">--</span>';
    const values = series.points.map(point => point.price);
    const range = Math.max(...values.map(value => Math.abs(value - item.previous_close)));
    const y = value => range === 0 ? 18 : 18 - (value - item.previous_close) / range * 14;
    const x = point => 2 + point.minute / series.duration * 60;
    const points = series.points.map(point => `${x(point).toFixed(1)},${y(point.price).toFixed(1)}`).join(" ");
    const last = series.points.at(-1);
    const label = `${item.name}，${series.date} 日内分时，${series.points[0].time}–${last.time}（交易所当地时间），昨收 ${number(item.previous_close)}，当日 ${signed(item.percent)}%${item.intraday_stale ? "，分时更新延迟" : ""}`;
    return `<svg class="market-spark ${tone(item.percent)}" viewBox="0 0 64 36" role="img" aria-label="${escapeHtml(label)}">
      <title>${escapeHtml(label)}</title>
      <line class="market-spark-baseline" x1="2" x2="62" y1="18" y2="18" />
      <polyline points="${points}" />
      ${series.points.length === 1 ? `<circle cx="${x(last)}" cy="${y(last.price)}" r="1.5" fill="currentColor" />` : ""}
    </svg>`;
  }

  function startMarketQuotes() {
    stopMarketQuotes();
    const host = document.querySelector("#tl-market");
    if (!host) return;
    let active = true;
    let group = selection === "auto" ? automaticGroup() : selection;
    const snapshots = {};
    const pending = new Set();
    const failures = new Set();

    function render() {
      const focus = host.contains(document.activeElement) && document.activeElement.matches(":focus-visible")
        ? document.activeElement.dataset.marketFocus : null;
      const snapshot = snapshots[group];
      const failed = failures.has(group);
      const available = (snapshot?.items || []).filter(item => Number.isFinite(item.price));
      const stale = failed || snapshot?.stale;
      const states = available.map(item => item.status);
      const status = !available.length ? (failed || snapshot ? "暂不可用" : "加载中")
        : stale ? "更新失败" : states.includes("delayed") ? "部分延迟"
        : states.includes("trading") ? "交易中" : states.includes("break") ? "午间休市"
        : states.includes("holiday") ? "今日休市" : "休市";
      const rows = GROUPS[group].map(([symbol, name, code]) => {
        const item = snapshot?.items?.find(item => item.symbol === symbol) || { symbol, name };
        const quoted = Number.isFinite(item.price);
        const quoteTitle = quoted ? `${timeFormat.format(new Date(item.quoted_at))}（北京时间）${item.stale || failed ? " · 更新失败" : ""}` : "暂无报价";
        return `<tr><th scope="row"><span class="market-name">${escapeHtml(name)}</span>
          <span class="market-symbol">${escapeHtml(code)}</span></th>
          <td class="market-chart">${sparkline(item)}</td>
          <td class="market-values" title="${escapeHtml(quoteTitle)}"><span class="market-price">${quoted ? number(item.price) : "--"}</span>
          <span class="market-change ${quoted ? tone(item.percent) : "flat"}" title="${quoted ? `涨跌额 ${signed(item.change)}` : "暂无报价"}">${quoted ? `${signed(item.percent)}%` : "--"}</span></td></tr>`;
      }).join("");
      const dates = available.map(item => Date.parse(item.quoted_at)).filter(Number.isFinite);
      const oldest = dates.length ? new Date(Math.min(...dates)) : null;
      const tradingDates = [...new Set(available.map(item => item.intraday?.date).filter(Boolean))].sort();
      const dateLabel = tradingDates.length ? tradingDates[0].slice(5).replace("-", "/")
        + (tradingDates.length > 1 ? `–${tradingDates.at(-1).slice(5).replace("-", "/")}` : "") : "";
      const chartDelayed = failed || available.some(item => item.intraday_stale);
      const marketClosed = states.length > 0 && states.every(state => state === "closed" || state === "holiday");
      const footerLabel = marketClosed ? "最近交易日" : chartDelayed ? "分时延迟" : "日内分时";
      host.innerHTML = `<div class="market-heading">
          <h3 class="tl-rail-title" id="market-title">市场概览</h3>
          <span class="market-status" role="status">${status}</span>
        </div>
        <div class="market-toolbar">
          <div class="market-tabs" role="group" aria-label="市场分组">
            ${[["day", "A股 / 港股"], ["night", "美股"]].map(([key, label]) => `<button type="button" data-market-group="${key}" data-market-focus="${key}" aria-pressed="${group === key}">${label}</button>`).join("")}
          </div>
          <label class="market-auto" title="北京时间 08:00–20:00 显示 A股 / 港股，其余时段显示美股"><input type="checkbox" data-market-focus="auto" ${selection === "auto" ? "checked" : ""}>自动</label>
        </div>
        <table class="market-table" aria-labelledby="market-title">
          <thead class="sr-only"><tr><th scope="col">标的</th><th scope="col">日内分时走势</th><th scope="col">点位与涨跌幅</th></tr></thead>
          <tbody>${rows}</tbody>
        </table>
        <div class="market-footer"><span title="分时日期（交易所当地日期）${chartDelayed ? ' · 部分分时更新延迟' : ''}">${footerLabel}${dateLabel ? ` · ${dateLabel}` : ""}</span>
          ${stale ? `<button type="button" class="market-retry" data-market-focus="retry" ${pending.has(group) ? "disabled" : ""}>重试</button>` : ""}
          <span title="报价时间（北京时间）">${oldest ? `<time datetime="${oldest.toISOString()}">${escapeHtml(timeFormat.format(oldest))}</time>` : "--"}</span>
        </div>`;
      host.querySelectorAll("[data-market-group]").forEach(button => button.addEventListener("click", () => {
        selection = button.dataset.marketGroup;
        group = selection;
        render();
        refresh();
      }));
      host.querySelector(".market-auto input").addEventListener("change", event => {
        selection = event.target.checked ? "auto" : group;
        refresh();
        render();
      });
      host.querySelector(".market-retry")?.addEventListener("click", refresh);
      if (focus) host.querySelector(`[data-market-focus="${focus}"]`)?.focus({ preventScroll: true });
    }

    async function refresh() {
      if (!active || document.visibilityState === "hidden") return;
      const next = selection === "auto" ? automaticGroup() : selection;
      if (next !== group) { group = next; render(); }
      if (pending.has(group)) return;
      const requested = group;
      pending.add(requested);
      const retry = host.querySelector(".market-retry");
      if (retry) retry.disabled = true;
      try {
        const data = await api(`/api/market/indices?group=${requested}`);
        if (!active || !host.isConnected) return;
        snapshots[requested] = data;
        failures.delete(requested);
      } catch {
        failures.add(requested);
      } finally {
        pending.delete(requested);
        if (active && host.isConnected && group === requested) render();
      }
    }

    render();
    refresh();
    const timer = setInterval(refresh, 30000);
    document.addEventListener("visibilitychange", refresh);
    stop = () => {
      active = false;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", refresh);
    };
  }

  function stopMarketQuotes() { stop(); }
  return { startMarketQuotes, stopMarketQuotes };
}
