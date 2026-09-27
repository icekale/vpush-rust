export function createAdminCodesView(dependencies) {
  const {
    $,
    state,
    api,
    flash,
    escapeHtml,
    jsString,
    routeStillActive,
    SEARCH_ICON,
    fmtDbTime,
    parseDbUtcMs,
    currentAdminSeq,
  } = dependencies;

  const _codesUi = {
    note: "",
    count: 5,
    expires: 7,
    filter: "available",
    q: "",
    result: null,
  };
  function codeStatus(c) {
    if (c.used_by) return "used";
    if (c.revoked_at) return "revoked";
    const exp = parseDbUtcMs(c.expires_at);
    if (exp != null && exp <= Date.now()) return "expired";
    return "available";
  }

  function codeStatusLabel(status) {
    return { available: "可用", used: "已用", revoked: "已作废", expired: "已过期" }[status] || status;
  }

  function codeStatusClass(status) {
    return { available: "status-ok", used: "status-fail", revoked: "status-fail", expired: "status-warn" }[status] || "";
  }

  function formatInviteCopy(codeList, expiresDays, note) {
    const head = expiresDays
      ? `VPush 邀请码（一次性，${expiresDays}天内有效）`
      : "VPush 邀请码（一次性）";
    const lines = [head, ...codeList];
    if (note) lines.push(`备注：${note}`);
    return lines.join("\n");
  }

  function formatInviteCopyUntil(codeList, expiresAt, note) {
    const head = expiresAt
      ? `VPush 邀请码（一次性，有效期至 ${fmtDbTime(expiresAt)})`
      : "VPush 邀请码（一次性）";
    const lines = [head, ...codeList];
    if (note) lines.push(`备注：${note}`);
    return lines.join("\n");
  }

  function copyDataAttr(text) {
    return encodeURIComponent(String(text ?? "")).replace(/'/g, "%27");
  }

  function copyText(text, okMsg) {
    if (!text) return;
    if (navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(text).then(
        () => flash(okMsg || "已复制"),
        () => alert("请手动复制：\n" + text),
      );
    } else {
      alert("请手动复制：\n" + text);
    }
  }

  let _adminCodesSelected = new Set();

  function codeCanRevoke(c) {
    return c && !c.used_by && !c.revoked_at;
  }

  function codeCanPurge(c) {
    const st = codeStatus(c);
    return st === "used" || st === "revoked" || st === "expired";
  }

  function adminCodesSelectedRows() {
    const all = state.adminCodes || [];
    return all.filter((c) => _adminCodesSelected.has(c.code));
  }

  function adminCodesSyncBar() {
    const bar = $("#rc-batch-bar");
    if (!bar) return;
    const selected = adminCodesSelectedRows();
    bar.style.display = _adminCodesSelected.size ? "flex" : "none";
    const strong = bar.querySelector("strong");
    if (strong) {
      const visibleSelected = document.querySelectorAll(".rc-check:checked").length;
      strong.textContent = visibleSelected < _adminCodesSelected.size
        ? `已选 ${_adminCodesSelected.size} 个（当前显示 ${visibleSelected}）`
        : `已选 ${_adminCodesSelected.size} 个`;
    }
    const revokeBtn = $("#rc-batch-revoke");
    const purgeBtn = $("#rc-batch-purge");
    if (revokeBtn) revokeBtn.disabled = !selected.some(codeCanRevoke);
    if (purgeBtn) purgeBtn.disabled = !selected.some(codeCanPurge);
  }

  function adminCodesToggle(el) {
    const code = el.dataset.code;
    if (!code) return;
    if (el.checked) _adminCodesSelected.add(code);
    else _adminCodesSelected.delete(code);
    adminCodesSyncBar();
    adminCodesSyncBatchChecks();
    adminCodesSyncPageCheck();
  }

  function adminCodesTogglePage(el) {
    document.querySelectorAll(".rc-check").forEach((c) => {
      c.checked = el.checked;
      if (el.checked) _adminCodesSelected.add(c.dataset.code);
      else _adminCodesSelected.delete(c.dataset.code);
    });
    adminCodesSyncBatchChecks();
    adminCodesSyncPageCheck();
    adminCodesSyncBar();
  }

  function adminCodesSyncPageCheck() {
    const el = $("#rc-checkall");
    if (!el) return;
    const boxes = [...document.querySelectorAll(".rc-check")];
    el.checked = boxes.length > 0 && boxes.every((c) => c.checked);
    el.indeterminate = boxes.some((c) => c.checked) && !el.checked;
  }

  function adminCodesToggleBatch(el) {
    const batchId = el.dataset.batch;
    document.querySelectorAll(`.rc-check[data-batch="${batchId}"]`).forEach((c) => {
      c.checked = el.checked;
      if (el.checked) _adminCodesSelected.add(c.dataset.code);
      else _adminCodesSelected.delete(c.dataset.code);
    });
    el.indeterminate = false;
    adminCodesSyncBatchChecks();
    adminCodesSyncPageCheck();
    adminCodesSyncBar();
  }

  function adminCodesSyncBatchChecks() {
    document.querySelectorAll(".rc-batch-check").forEach((el) => {
      const boxes = [...document.querySelectorAll(`.rc-check[data-batch="${el.dataset.batch}"]`)];
      el.checked = boxes.length > 0 && boxes.every((c) => c.checked);
      el.indeterminate = boxes.some((c) => c.checked) && !el.checked;
    });
  }

  function adminCodesClearSelect() {
    _adminCodesSelected.clear();
    document.querySelectorAll(".rc-check").forEach((c) => { c.checked = false; });
    document.querySelectorAll(".rc-batch-check").forEach((c) => {
      c.checked = false;
      c.indeterminate = false;
    });
    adminCodesSyncPageCheck();
    adminCodesSyncBar();
  }

  function adminCodesCopySelected() {
    const codes = [..._adminCodesSelected];
    if (!codes.length) return;
    copyText(codes.join("\n"), `已复制 ${codes.length} 个邀请码`);
  }

  async function adminCodesBatch(action) {
    const selected = adminCodesSelectedRows();
    const codes = action === "revoke"
      ? selected.filter(codeCanRevoke).map((c) => c.code)
      : selected.filter(codeCanPurge).map((c) => c.code);
    if (!codes.length) return;
    const skipped = selected.length - codes.length;
    const skipTip = skipped ? `（另有 ${skipped} 个${action === "revoke" ? "不可作废" : "不可删除"}，已跳过）` : "";
    const ok = action === "revoke"
      ? confirm(`将作废选中的 ${codes.length} 个未使用邀请码${skipTip}，确认？`)
      : confirm(`将从列表删除选中的 ${codes.length} 个已用/已作废/已过期邀请码${skipTip}，不可恢复。确认？`);
    if (!ok) return;
    try {
      const data = await api("/api/admin/register-codes/batch", {
        method: "POST",
        body: JSON.stringify({ codes, action }),
      });
      const serverSkipped = data.skipped || 0;
      const msg = action === "revoke"
        ? (serverSkipped ? `已作废 ${data.count} 个，跳过 ${serverSkipped} 个` : `已作废 ${data.count} 个邀请码`)
        : (serverSkipped ? `已删除 ${data.count} 个，跳过 ${serverSkipped} 个` : `已删除 ${data.count} 个邀请码`);
      flash(msg);
      _adminCodesSelected.clear();
      loadAdminCodes();
    } catch (err) {
      flash(err.message, "error");
    }
  }

  function saveCodesForm() {
    const note = $("#rc-note");
    const count = $("#rc-count");
    const exp = $("#rc-expires");
    const q = $("#rc-q");
    if (note) _codesUi.note = note.value;
    if (count) _codesUi.count = Number(count.value) || 5;
    if (exp) _codesUi.expires = exp.value === "" ? null : Number(exp.value);
    if (q) _codesUi.q = q.value.trim();
  }

  function adminCodesPreset(note) {
    const el = $("#rc-note");
    if (el) el.value = note;
    _codesUi.note = note;
    adminCodesSyncPresets();
  }

  function adminCodesNoteInput() {
    const el = $("#rc-note");
    _codesUi.note = el ? el.value : "";
    adminCodesSyncPresets();
  }

  function adminCodesSyncPresets() {
    document.querySelectorAll(".rc-preset").forEach((b) => {
      b.classList.toggle("selected", b.dataset.note === _codesUi.note);
    });
  }

  async function loadAdminCodes(refetch = true) {
    if (refetch || !state.adminCodes) {
      state.adminCodes = await api("/api/admin/register-codes");
    }
    const known = new Set((state.adminCodes || []).map((c) => c.code));
    for (const code of [..._adminCodesSelected]) {
      if (!known.has(code)) _adminCodesSelected.delete(code);
    }
    if (!routeStillActive(currentAdminSeq())) return;
    const filter = _codesUi.filter;
    const expVal = _codesUi.expires == null ? "" : String(_codesUi.expires);
    const result = _codesUi.result;
    const allCodes = state.adminCodes || [];
    const tabCounts = { available: 0, used: 0, revoked: 0, expired: 0, all: allCodes.length };
    for (const c of allCodes) tabCounts[codeStatus(c)] += 1;
    const filterBtn = (key, label) =>
      `<button type="button" class="settings-tab ${filter === key ? "active" : ""}" role="tab" aria-selected="${filter === key}" data-filter="${key}" onclick="selectAdminCodeFilter('${key}')">${label} ${tabCounts[key]}</button>`;

    $("#admin-body").innerHTML = `
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">生成注册邀请码</h2>
          <p class="section-meta">一次性邀请码，按批生成；用过即废，可设有效期。</p></div>
        </header>
        <div class="rc-generate">
          <label class="rc-field rc-field-note">
            <span>备注</span>
            <input id="rc-note" class="form-control" maxlength="40" placeholder="给谁、什么场合" value="${escapeHtml(_codesUi.note)}" oninput="adminCodesNoteInput()">
          </label>
          <div class="rc-field">
            <span>常用</span>
            <div class="rc-presets" role="group" aria-label="常用备注">
              <button type="button" class="rc-preset${_codesUi.note === "内部" ? " selected" : ""}" data-note="内部" onclick="adminCodesPreset('内部')">内部</button>
              <button type="button" class="rc-preset${_codesUi.note === "朋友" ? " selected" : ""}" data-note="朋友" onclick="adminCodesPreset('朋友')">朋友</button>
            </div>
          </div>
          <label class="rc-field rc-field-count">
            <span>数量</span>
            <input id="rc-count" class="form-control" type="number" min="1" max="100" value="${escapeHtml(String(_codesUi.count))}">
          </label>
          <label class="rc-field rc-field-expires">
            <span>有效期</span>
            <select id="rc-expires" class="form-control">
              <option value="1" ${expVal === "1" ? "selected" : ""}>1天</option>
              <option value="7" ${expVal === "7" ? "selected" : ""}>7天</option>
              <option value="30" ${expVal === "30" ? "selected" : ""}>30天</option>
              <option value="" ${expVal === "" ? "selected" : ""}>永不过期</option>
            </select>
          </label>
          <div class="rc-field-submit">
            <button class="btn-normal" onclick="adminGenerateCodes()">生成</button>
          </div>
        </div>
        ${result ? renderCodesResult(result) : ""}
      </section>
      <section class="section-panel">
        <header class="section-head rc-list-head">
          <div>
            <h2 class="section-title">注册码列表</h2>
            <p class="section-meta">${tabCounts.all} 个 · ${tabCounts.available} 可用</p>
          </div>
          <div class="search-bar rc-search">
            ${SEARCH_ICON}
            <input id="rc-q" type="search" placeholder="搜索码或备注" value="${escapeHtml(_codesUi.q)}" oninput="searchAdminCodes(this.value)">
          </div>
        </header>
        <div class="settings-tabs rc-tabs" role="tablist" aria-label="注册码状态">
          ${filterBtn("available", "可用")}
          ${filterBtn("used", "已用")}
          ${filterBtn("revoked", "已作废")}
          ${filterBtn("expired", "已过期")}
          ${filterBtn("all", "全部")}
        </div>
        <div class="toolbar admin-batch-bar" id="rc-batch-bar" style="margin-top:10px;display:${_adminCodesSelected.size ? "flex" : "none"};align-items:center;gap:8px;flex-wrap:wrap">
          <strong>已选 ${_adminCodesSelected.size} 个</strong>
          <button type="button" class="btn-sm" onclick="adminCodesCopySelected()">复制</button>
          <button type="button" class="btn-sm" id="rc-batch-revoke" onclick="adminCodesBatch('revoke')">作废未用</button>
          <button type="button" class="btn-sm danger" id="rc-batch-purge" onclick="adminCodesBatch('delete')">清掉废码</button>
          <button type="button" class="btn-sm" onclick="adminCodesClearSelect()">取消选择</button>
        </div>
        <div class="rc-list-toolbar">
          <label class="rc-checkall">
            <input type="checkbox" id="rc-checkall" onchange="adminCodesTogglePage(this)" aria-label="全选当前筛选">
            <span>全选当前筛选</span>
          </label>
        </div>
        <div id="rc-list"></div>
      </section>`;
    renderCodesList();
    adminCodesSyncBar();
  }

  function renderCodesList() {
    const codes = state.adminCodes || [];
    const filter = _codesUi.filter;
    const q = (_codesUi.q || "").trim().toLowerCase();
    const filtered = codes.filter((c) => {
      if (filter !== "all" && codeStatus(c) !== filter) return false;
      if (!q) return true;
      return String(c.code).toLowerCase().includes(q) || String(c.note || "").toLowerCase().includes(q);
    });
    const groups = [];
    const byBatch = new Map();
    for (const c of codes) {
      const id = c.batch_id || c.code;
      if (!byBatch.has(id)) byBatch.set(id, []);
      byBatch.get(id).push(c);
    }
    const visibleIds = new Set(filtered.map((c) => c.batch_id || c.code));
    for (const [id, rows] of byBatch) {
      if (!visibleIds.has(id)) continue;
      groups.push({ id, rows, visible: rows.filter((c) => filtered.includes(c)) });
    }
    groups.sort((a, b) => String(b.rows[0].created_at).localeCompare(String(a.rows[0].created_at)));
    const el = $("#rc-list");
    if (el) el.innerHTML = renderCodeGroups(groups, filter);
    adminCodesSyncBatchChecks();
    adminCodesSyncPageCheck();
    adminCodesSyncBar();
  }

  function renderCodesResult(result) {
    const days = result.expires_in_days;
    const copy = formatInviteCopy(result.codes, days, result.note);
    return `<div class="rc-result">
      <div class="rc-result-head">
        <strong>已生成 ${result.codes.length} 个</strong>
        <div class="rc-result-actions">
          <button class="btn-sm" data-copy="${copyDataAttr(copy)}" onclick="copyText(decodeURIComponent(this.getAttribute('data-copy')), '已复制本批邀请码')">复制全部</button>
          <button class="btn-sm danger" onclick="adminRevokeBatch(${jsString(result.batch_id)}, true)">作废本批未用</button>
          <button class="btn-sm" onclick="clearAdminCodesResult()">关闭</button>
        </div>
      </div>
      <div class="rc-result-codes">${result.codes.map((code) =>
        `<div class="rc-result-row"><code>${escapeHtml(code)}</code><button class="btn-sm" data-code="${escapeHtml(code)}" onclick="copyText(this.dataset.code, '已复制')">复制</button></div>`
      ).join("")}</div>
    </div>`;
  }

  function renderCodeGroups(groups, filter) {
    if (groups.length === 0) {
      const empty =
        filter === "available"
          ? "没有可用注册码。在上方生成一批，复制后发给对方。"
          : filter === "used"
            ? "还没有人用过邀请码。"
            : "没有符合条件的注册码。";
      return `<p class="rc-empty muted">${empty}</p>`;
    }
    return groups.map((g) => {
      const all = g.rows;
      const notes = [...new Set(all.map((c) => c.note || ""))];
      const noteLabel = notes.length === 1 ? (notes[0] || "无备注") : "备注不一";
      const available = all.filter((c) => codeStatus(c) === "available");
      const usedN = all.filter((c) => codeStatus(c) === "used").length;
      const unusedOpen = all.filter((c) => !c.used_by && !c.revoked_at);
      const expLabel = all[0].expires_at ? `过期 ${escapeHtml(fmtDbTime(all[0].expires_at))}` : "永不过期";
      const creator = all[0].created_by_name ? ` · ${escapeHtml(all[0].created_by_name)}` : "";
      const copyCodes = available.map((c) => c.code);
      const copyNote = notes.length === 1 ? notes[0] : "";
      const copy = formatInviteCopyUntil(copyCodes, all[0].expires_at, copyNote);
      return `<div class="rc-batch">
        <div class="rc-batch-head">
          <div class="rc-batch-info">
            <div class="rc-batch-title">
              <input type="checkbox" class="rc-batch-check" data-batch="${escapeHtml(g.id)}" onchange="adminCodesToggleBatch(this)" aria-label="全选本批可见" title="全选本批当前可见的注册码">
              <strong>${escapeHtml(noteLabel)}</strong>
              <span class="rc-counts">${available.length} 可用 / ${usedN} 已用</span>
            </div>
            <p class="muted rc-batch-meta">${escapeHtml(fmtDbTime(all[0].created_at))} · ${expLabel}${creator}</p>
          </div>
          <div class="rc-batch-actions">
            <button class="btn-sm" ${copyCodes.length ? "" : "disabled"} data-copy="${copyDataAttr(copy)}" onclick="copyText(decodeURIComponent(this.getAttribute('data-copy')), '已复制未用码')">复制未用</button>
            <button class="btn-sm danger" ${unusedOpen.length ? "" : "disabled"} onclick="adminRevokeBatch(${jsString(g.id)})">作废未用</button>
          </div>
        </div>
        <div class="table-wrap">
          <table class="rc-table">
            <thead><tr><th scope="col">邀请码</th><th scope="col">备注</th><th scope="col">状态</th><th scope="col">使用者</th><th scope="col">时间</th><th scope="col">操作</th></tr></thead>
            <tbody>${g.visible.map((c) => renderCodeRow(c)).join("")}</tbody>
          </table>
        </div>
      </div>`;
    }).join("");
  }

  function renderCodeRow(c) {
    const st = codeStatus(c);
    const when = c.used_at ? fmtDbTime(c.used_at) : c.revoked_at ? fmtDbTime(c.revoked_at) : c.expires_at ? fmtDbTime(c.expires_at) : fmtDbTime(c.created_at);
    const canRevoke = st === "available" || st === "expired";
    const checked = _adminCodesSelected.has(c.code) ? "checked" : "";
    const note = String(c.note || "").trim();
    const user = String(c.used_by_name || "").trim();
    return `<tr>
      <td class="rc-code-cell"><span class="rc-code"><input type="checkbox" class="rc-check" data-code="${escapeHtml(c.code)}" data-batch="${escapeHtml(c.batch_id || c.code)}" ${checked} onchange="adminCodesToggle(this)" aria-label="选择邀请码"><code>${escapeHtml(c.code)}</code></span></td>
      <td class="rc-note-cell${note ? "" : " rc-empty-cell"}">${escapeHtml(note)}</td>
      <td class="rc-status-cell ${codeStatusClass(st)}">${codeStatusLabel(st)}</td>
      <td class="rc-user-cell${user ? "" : " rc-empty-cell"}">${escapeHtml(user)}</td>
      <td class="rc-time-cell">${escapeHtml(when)}</td>
      <td class="rc-actions"><button class="btn-sm" data-code="${escapeHtml(c.code)}" onclick="copyText(this.dataset.code, '已复制')">复制</button>${canRevoke ? `<button class="btn-sm danger" data-code="${escapeHtml(c.code)}" onclick="adminRevokeCode(this.dataset.code)">作废</button>` : ""}</td>
    </tr>`;
  }

  async function adminRevokeCode(code) {
    if (!confirm(`确认作废注册码 ${code}？作废后无法再使用。`)) return;
    try {
      await api(`/api/admin/register-codes/${encodeURIComponent(code)}/revoke`, { method: "POST" });
      flash(`已作废邀请码 ${code}`);
      loadAdminCodes();
    } catch (err) {
      alert("作废失败: " + err.message);
    }
  }

  async function adminRevokeBatch(batchId, fromResult) {
    if (!confirm("将作废本批所有未使用的邀请码，确认？")) return;
    try {
      await api(`/api/admin/register-code-batches/${encodeURIComponent(batchId)}/revoke-unused`, { method: "POST" });
      flash("已作废本批未用码");
      if (fromResult) _codesUi.result = null;
      loadAdminCodes();
    } catch (err) {
      alert("作废失败: " + err.message);
    }
  }

  async function adminGenerateCodes() {
    saveCodesForm();
    try {
      const expiresRaw = $("#rc-expires").value;
      const expires_in_days = expiresRaw === "" ? null : Number(expiresRaw);
      const data = await api("/api/admin/register-codes", {
        method: "POST",
        body: JSON.stringify({
          count: Number($("#rc-count").value) || 5,
          note: $("#rc-note").value.trim(),
          expires_in_days,
        }),
      });
      _codesUi.result = { ...data, expires_in_days };
      _codesUi.filter = "available";
      flash(`已生成 ${data.count} 个邀请码`);
      loadAdminCodes();
    } catch (err) {
      alert("生成失败: " + err.message);
    }
  }
  function selectAdminCodeFilter(key) {
    saveCodesForm();
    _codesUi.filter = key;
    return loadAdminCodes(false);
  }

  function searchAdminCodes(query) {
    _codesUi.q = query;
    renderCodesList();
  }

  function clearAdminCodesResult() {
    _codesUi.result = null;
    return loadAdminCodes();
  }

  return {
    loadAdminCodes,
    adminCodesBatch,
    adminCodesClearSelect,
    adminCodesCopySelected,
    adminCodesNoteInput,
    adminCodesPreset,
    adminCodesToggle,
    adminCodesToggleBatch,
    adminCodesTogglePage,
    adminGenerateCodes,
    adminRevokeBatch,
    adminRevokeCode,
    searchAdminCodes,
    selectAdminCodeFilter,
    clearAdminCodesResult,
    copyText,
  };
}
