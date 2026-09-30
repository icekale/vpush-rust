const ETFS = [
  ["513100", "纳指ETF国泰"],
  ["513500", "标普500ETF博时"],
];

export function createEtfPremiumView({ api, escapeHtml }) {
  const host = document.querySelector("#tl-etf-premium");
  if (!host) return () => {};
  let active = true;
  let timer;
  const number = value => Number.isFinite(value) ? value.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 }) : "--";
  const time = value => value ? new Intl.DateTimeFormat("zh-CN", { timeZone: "Asia/Shanghai", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(new Date(value)) : "--";
  const render = data => {
    const items = data?.items || [];
    host.innerHTML = `<section class="market-etf" aria-labelledby="market-etf-title">
      <div class="market-heading"><h3 class="tl-rail-title" id="market-etf-title">ETF 溢价</h3><span class="market-status">${data ? "实时" : "加载中"}</span></div>
      ${ETFS.map(([symbol, fallback]) => { const item = items.find(row => row.symbol === symbol) || {}; return `<div class="market-etf-row"><div class="market-etf-name"><strong>${escapeHtml(item.name || fallback)}</strong><span>${symbol} · ${escapeHtml(item.status || "暂无数据")}</span></div><dl class="market-etf-values"><div><dt>现价</dt><dd>${number(item.market_price)}</dd></div><div><dt>IOPV</dt><dd>${number(item.reference_value)}</dd></div><div><dt>溢价率</dt><dd>${number(item.premium_rate)}%</dd></div></dl><div class="market-etf-times">报价时间 · ${time(item.market_at)} · IOPV时间 · ${time(item.reference_at)}</div></div>`; }).join("")}
      <p class="market-etf-empty">${data?.latest_alert ? `最近触发：${escapeHtml(data.latest_alert.name)} · ${number(data.latest_alert.triggered_pct)}%（阈值 ${number(data.latest_alert.threshold_pct)}%）` : "暂无触发记录"}</p>
    </section>`;
  };
  const refresh = async () => { try { const data = await api("/api/market/etf-premiums"); if (active) render(data); } catch { if (active) render(null); } };
  render(null); refresh(); timer = setInterval(refresh, 30000);
  return () => { active = false; clearInterval(timer); };
}
