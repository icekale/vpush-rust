import { escapeHtml } from "../../core/html.js";
import { REFRESH_ICON } from "../../core/icons.js";

export function createCiccView({ api, flash, fmtTs, currentRouteSeq, routeStillActive, renderLocalTab }) {
  let _ciccPollTimer = null;
  let _ciccRefreshTimer = null;
  let lifecycle = 0;
  let _ciccLastStatus = null;
  let _ciccDetailsOpen = false; // 轮询重绘时保留「研报采集」展开状态
  function startCiccPoll() {
    if (_ciccPollTimer) clearInterval(_ciccPollTimer);
    _ciccPollTimer = setInterval(() => {
      if (location.pathname !== "/admin/knowledge") { stopCiccPoll(); return; }
      loadCiccStatus(true);
    }, 15000);
  }

  function stopCiccPoll() {
    lifecycle += 1;
    if (_ciccPollTimer) { clearInterval(_ciccPollTimer); _ciccPollTimer = null; }
    if (_ciccRefreshTimer) { clearTimeout(_ciccRefreshTimer); _ciccRefreshTimer = null; }
  }

  function captureOwner() {
    const owner = lifecycle;
    const routeSeq = currentRouteSeq();
    return () => owner === lifecycle && routeStillActive(routeSeq);
  }

  function ciccRunningText(data) {
    if (!data.available) return "不可用（未挂载存储归档）";
    if (data.stale) return "状态过期（存储机未刷新）";
    const parts = [];
    if (data.running > 0) parts.push(`采集中（${data.running} 路）`);
    if (data.compress_running > 0) parts.push("压缩回刷中");
    if (!parts.length) parts.push("空闲");
    parts.push(`库存 ${data.files_total ?? "—"} 篇`);
    parts.push(`每日增量：${data.schedule_enabled ? "开" : "关"}（${(data.storage && data.storage.schedule && data.storage.schedule.time) || "03:00"}）`);
    if (data.last_incremental?.ts) {
      parts.push(`上次自动增量 ${fmtTs(data.last_incremental.ts)}（${data.last_incremental.note === "launched" ? "已启动" : data.last_incremental.note || ""}）`);
    }
    return parts.join(" · ");
  }

  const CICC_RESEARCH_SLUG = "cicc-research";
  // 与后端 app/cicc_collector.py 的 CICC_CATEGORIES 保持一致
  const CICC_CATEGORIES = ["宏观经济", "市场策略", "全球研究", "行业研究", "公司研究", "量化及ESG", "大宗商品", "外汇研究", "固定收益", "中金研究院", "其他"];

  function ciccPausedLine(cicc) {
    const p = cicc.paused;
    if (!p) return "";
    const why = p.reason === "quota" ? "本月研报配额已满，等月初重置"
      : p.reason === "auth" ? "登录态失效，请更新存储机 Cookie"
      : escapeHtml(p.detail || "未知原因");
    return `<p class="muted" style="color:var(--color-danger)" aria-live="polite">⏸ 采集已熔断：${why}（${fmtTs(p.ts)}）</p>`;
  }

  function ciccCategoriesHtml(cicc) {
    const active = Array.isArray(cicc.cicc_settings && cicc.cicc_settings.categories)
      ? cicc.cicc_settings.categories : [];
    const boxes = CICC_CATEGORIES.map((c) =>
      `<label style="margin-right:12px;white-space:nowrap"><input type="checkbox" class="cicc-cat" value="${escapeHtml(c)}" ${active.includes(c) ? "checked" : ""}> ${escapeHtml(c)}</label>`).join("");
    const activeKw = Array.isArray(cicc.cicc_settings && cicc.cicc_settings.keywords)
      ? cicc.cicc_settings.keywords : [];
    return `<details><summary class="cfg-group-title">品类定向与关键词白名单（全不勾选 = 采集全部）</summary>
      <p class="muted">当前：${active.length ? escapeHtml(active.join("、")) : "全部品类"}${activeKw.length ? ` · 关键词：${escapeHtml(activeKw.join("、"))}` : ""}</p>
      <div style="display:flex;flex-wrap:wrap;gap:4px 0;margin:6px 0;max-width:560px">${boxes}</div>
      <div style="margin:8px 0">标题关键词白名单（逗号分隔，命中任一即采集）：
        <input type="text" id="cicc-keywords" class="form-control" style="width:320px;display:inline-block;vertical-align:middle" value="${escapeHtml(activeKw.join(","))}" placeholder="如：宁德时代,半导体"></div>
      <button type="button" class="btn-ghost" onclick="saveCiccCategories()">保存品类与关键词</button></details>`;
  }

  async function saveCiccCategories() {
    const isActive = captureOwner();
    const cats = Array.from(document.querySelectorAll(".cicc-cat:checked")).map((el) => el.value);
    const keywords = (document.getElementById("cicc-keywords") || {}).value || "";
    try {
      const r = await api("/api/admin/ima-collector/cicc-categories", { method: "PUT", body: JSON.stringify({ categories: cats, keywords }) });
      if (!isActive()) return;
      flash(r.categories.length ? `品类定向已保存：${r.categories.join("、")}` : "已设为采集全部品类");
      loadCiccStatus();
    } catch (err) {
      if (!isActive()) return;
      flash(`保存失败：${err.message}`, "error");
    }
  }

  function ciccControlInnerHtml(cicc) {
    const logs = Object.entries(cicc.logs || {})
      .filter(([, line]) => line)
      .map(([name, line]) => `<p class="muted" style="margin:2px 0"><strong>${escapeHtml(name)}</strong>：${escapeHtml(line)}</p>`)
      .join("") || '<p class="muted">暂无日志</p>';
    const cmds = (cicc.commands || []).slice(-5).reverse()
      .map((c) => `<p class="muted" style="margin:2px 0">${fmtTs(c.ts)} · ${escapeHtml(c.mode || "?")} · ${c.ok ? "已执行" : `失败（${escapeHtml(c.error || "")}）`}</p>`)
      .join("") || '<p class="muted">暂无操作记录</p>';
    return `
      <p class="muted" aria-live="polite">${escapeHtml(ciccRunningText(cicc))} · 更新于 ${fmtTs(cicc.ts)}</p>
      <p class="muted">增量完成后，向已授权且开启「匹配研报库」的用户推送命中摘要。</p>
      ${ciccPausedLine(cicc)}
      <div class="toolbar" style="margin:12px 0">
        <button type="button" class="btn-normal" onclick="triggerCicc('incr')">增量采集（近3天）</button>
        <button type="button" class="btn-normal" onclick="triggerCicc('year')">今年回补</button>
        <button type="button" class="btn-ghost" onclick="triggerCicc('compress')">压缩回刷</button>
        <button type="button" class="btn-ghost" onclick="triggerCicc('all')">全量回补</button>
        <button type="button" class="btn-ghost danger" onclick="triggerCicc('stop')">停止采集</button>
      </div>
      <div class="toolbar" style="margin:0 0 12px">
        <input type="time" id="cicc-schedule-time" class="form-control" style="width:120px;display:inline-block;vertical-align:middle" value="${(_ciccLastStatus && _ciccLastStatus.storage && _ciccLastStatus.storage.schedule && _ciccLastStatus.storage.schedule.time) || "03:00"}">
        <button type="button" class="btn-ghost" onclick="saveCiccScheduleTime()">保存时间</button>
        <button type="button" class="btn-ghost" onclick="toggleCiccSchedule()">${cicc.schedule_enabled ? "关闭每日增量" : "开启每日增量"}（03:00）</button>
        <button type="button" class="btn-ghost" onclick="loadCiccStatus()">${REFRESH_ICON}<span>刷新</span></button>
      </div>
      ${ciccCategoriesHtml(cicc)}
      <details open><summary class="cfg-group-title">采集日志（最新一行）</summary>${logs}</details>
      <details><summary class="cfg-group-title">最近操作</summary>${cmds}</details>`;
  }

  async function loadCiccStatus(quiet = false) {
    const isActive = captureOwner();
    try {
      const data = await api("/api/admin/cicc/status");
      if (!isActive()) return;
      _ciccLastStatus = data;
      renderLocalTab();
    } catch (err) {
      if (!isActive()) return;
      if (!quiet) flash(err.message, "error");
      _ciccLastStatus = null;
      renderLocalTab();
    }
  }

  async function triggerCicc(mode) {
    const isActive = captureOwner();
    if (mode === "all" && !confirm("全量回补会拉取历史全部研报（数万篇，需数天），确定？")) return;
    if (mode === "stop" && !confirm("确定停止当前采集？已下载文件保留，下次触发自动续传。")) return;
    try {
      await api("/api/admin/cicc/trigger", { method: "POST", body: JSON.stringify({ mode }) });
      if (!isActive()) return;
      flash(mode === "stop" ? "已发送停止命令" : "已触发，稍候刷新看进度");
      if (_ciccRefreshTimer) clearTimeout(_ciccRefreshTimer);
      _ciccRefreshTimer = setTimeout(() => {
        _ciccRefreshTimer = null;
        if (isActive()) loadCiccStatus(true);
      }, 3000);
    } catch (err) {
      if (!isActive()) return;
      flash(err.message, "error");
    }
  }

  async function saveCiccScheduleTime() {
    const isActive = captureOwner();
    const value = (document.getElementById("cicc-schedule-time") || {}).value || "";
    if (!/^[0-9]{2}:[0-9]{2}$/.test(value)) { flash("时间格式应为 HH:mm", "error"); return; }
    const enabled = !!(_ciccLastStatus && _ciccLastStatus.schedule_enabled);
    try {
      const r = await api("/api/admin/cicc/schedule", {
        method: "PUT", body: JSON.stringify({ enabled, time: value }),
      });
      if (!isActive()) return;
      flash(`采集时间已设为每天 ${r.time}`);
      loadCiccStatus();
    } catch (err) {
      if (isActive()) flash(`保存失败：${err.message}`, "error");
    }
  }

  async function toggleCiccSchedule() {
    const isActive = captureOwner();
    try {
      const enabled = !(_ciccLastStatus && _ciccLastStatus.schedule_enabled);
      const time = (document.getElementById("cicc-schedule-time") || {}).value || "03:00";
      const r = await api("/api/admin/cicc/schedule", {
        method: "PUT", body: JSON.stringify({ enabled, time }),
      });
      if (!isActive()) return;
      flash(r.schedule_enabled ? `每日增量已开启（每天 ${r.time}）` : "每日增量已关闭");
      loadCiccStatus(true);
    } catch (err) {
      if (!isActive()) return;
      flash(err.message, "error");
    }
  }

  function renderLibraryControls(slug) {
    return slug === CICC_RESEARCH_SLUG && _ciccLastStatus
      ? `<details class="cicc-collect"${_ciccDetailsOpen ? " open" : ""}><summary class="cfg-group-title">研报采集</summary>${ciccControlInnerHtml(_ciccLastStatus)}</details>`
      : "";
  }

  function renderFallback(libs) {
    const hasCiccLib = libs.some((lib) => lib.slug === CICC_RESEARCH_SLUG);
    return !hasCiccLib && _ciccLastStatus
      ? `<div class="ima-source-block" style="margin:12px 0">
          <h3 class="ima-source-title">中金研报采集</h3>
          <p class="section-meta">尚未扫描到 cicc-research 库；采集可先行，文件落盘后扫描即入库。</p>
          ${ciccControlInnerHtml(_ciccLastStatus)}
        </div>`
      : "";
  }

  function rememberDetails(slot) {
    const prev = slot.querySelector("details.cicc-collect");
    if (prev) _ciccDetailsOpen = prev.open;
  }

  function reset() {
    stopCiccPoll();
    _ciccLastStatus = null;
    _ciccDetailsOpen = false;
  }

  return {
    loadCiccStatus, startCiccPoll, stopCiccPoll,
    saveCiccCategories, triggerCicc, saveCiccScheduleTime, toggleCiccSchedule,
    renderLibraryControls, renderFallback, rememberDetails, reset,
  };
}
