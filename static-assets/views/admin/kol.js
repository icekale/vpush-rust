export function createAdminKolsView(dependencies) {
  const {
    $,
    state,
    api,
    flash,
    escapeHtml,
    routeStillActive,
    currentAdminSeq,
    emptyState,
    trapFocus,
    PLATFORM_LABELS,
    PLATFORM_TABS,
    platformTabHTML,
    routeQuery,
    CHEVRON_LEFT_ICON,
    CHEVRON_RIGHT_ICON,
  } = dependencies;

  let _adminKolsSeq = 0;
  const _adminKolsPageSize = 50;
  let _adminKolsSelected = new Set(); // 批量操作选中的大V id（跨页保留）

  async function loadAdminKols(opts) {
    opts = opts || {};
    const seq = ++_adminKolsSeq;
    let data, categories;
    try {
      const params = new URLSearchParams({
        limit: String(_adminKolsPageSize),
        offset: String((state.adminKolsPage || 0) * _adminKolsPageSize),
      });
      if (state.adminKolsPlatform) params.set("platform", state.adminKolsPlatform);
      if (state.adminKolsCategory) params.set("category_id", state.adminKolsCategory);
      if (state.adminKolsStatus !== "") params.set("status", state.adminKolsStatus);
      if (state.adminKolsQ) params.set("q", state.adminKolsQ);
      [data, categories] = await Promise.all([api(`/api/admin/kols?${params}`), api("/api/categories")]);
    } catch (err) {
      if (!routeStillActive(currentAdminSeq())) return;
      if (seq === _adminKolsSeq) $("#admin-body").innerHTML = emptyState("加载失败: " + err.message);
      return;
    }
    if (seq !== _adminKolsSeq) return; // 已切换筛选/翻页，丢弃过期响应
    const matchIds = new Set(data.ids || []);
    const focusIds = opts.focusIds || [];
    const visibleFocus = focusIds.filter((id) => matchIds.has(id));
    if (visibleFocus.length) {
      const idx = (data.ids || []).indexOf(visibleFocus[0]);
      const wantPage = Math.max(0, Math.floor(idx / _adminKolsPageSize));
      if (wantPage !== (state.adminKolsPage || 0)) {
        state.adminKolsPage = wantPage;
        return loadAdminKols({ focusIds: visibleFocus });
      }
    }
    const highlightIds = new Set(visibleFocus);
    const kols = data.items || [];
    state.adminKols = kols;
    state.adminKolsTotal = data.total || 0;
    for (const id of [..._adminKolsSelected]) {
      if (!matchIds.has(id)) _adminKolsSelected.delete(id);
    }
    const catOptions = categories.map((c) => `<option value="${c.id}">${escapeHtml(c.name)}</option>`).join("");
    const page = state.adminKolsPage || 0;
    const pages = Math.max(1, Math.ceil((data.total || 0) / _adminKolsPageSize));
    if (!routeStillActive(currentAdminSeq())) return;
    const selCount = _adminKolsSelected.size;
    const rows = kols.map((k) => {
      const tier = k.priority ? "优先" : k.secondary ? "次要" : "普通";
      const orig = k.platform === "weibo"
        ? (k.original_only ? '<span class="status-ok">是</span>' : "否")
        : "—";
      const tierBtns = k.priority
        ? `<button class="btn-sm" onclick="adminTogglePriority(${k.id}, false)">改普通</button>
                  <button class="btn-sm" onclick="adminToggleSecondary(${k.id}, true)">设次要</button>`
        : k.secondary
          ? `<button class="btn-sm" onclick="adminToggleSecondary(${k.id}, false)">改普通</button>
                  <button class="btn-sm" onclick="adminTogglePriority(${k.id}, true)">设优先</button>`
          : `<button class="btn-sm" onclick="adminTogglePriority(${k.id}, true)">设优先</button>
                  <button class="btn-sm" onclick="adminToggleSecondary(${k.id}, true)">设次要</button>`;
      return `
              <tr class="${highlightIds.has(k.id) ? "ak-row-flash" : ""}">
                <td class="ak-check"><input type="checkbox" class="kol-check" data-id="${k.id}" ${_adminKolsSelected.has(k.id) ? "checked" : ""} onchange="adminKolToggleSelect(this)" aria-label="选择 ${escapeHtml(k.name)}"></td>
                <td class="ak-hide-mobile" data-label="ID">${k.id}</td>
                <td data-label="平台">${PLATFORM_LABELS[k.platform] || k.platform}</td>
                <td data-label="昵称">${escapeHtml(k.name)}</td>
                <td data-label="分类">${escapeHtml(k.category_name || "")}</td>
                <td class="ak-hide-mobile" data-label="外部ID">${escapeHtml(k.external_id)}</td>
                <td data-label="档位">${tier}</td>
                <td class="ak-hide-mobile" data-label="原创">${orig}</td>
                <td data-label="可见性">${k.is_private ? '<span class="status-warn">私有</span>' : "公开"}</td>
                <td data-label="状态" class="${k.enabled ? "status-ok" : "status-fail"}">${k.enabled ? "启用" : "停用"}</td>
                <td class="ak-actions" data-label="操作">
                  ${tierBtns}
                  <button class="btn-sm" onclick="adminToggleKol(${k.id}, ${k.enabled ? 0 : 1})">${k.enabled ? "停用" : "启用"}</button>
                  <button class="btn-sm" onclick="adminEditKol(${k.id})">编辑</button>
                  <button class="btn-sm danger" onclick="adminDeleteKol(${k.id})">删除</button>
                </td>
              </tr>`;
    }).join("");
    $("#admin-body").innerHTML = `
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">添加大V</h2>
          <p class="section-meta">每行一个：昵称 + 主页链接（昵称可省略）。平台由链接自动识别。</p></div>
        </header>
        <textarea id="ad-batch-lines" class="form-control ak-add-lines" rows="6" placeholder="https://xueqiu.com/u/12345&#10;段永平 https://xueqiu.com/u/12345&#10;https://weibo.com/u/1642591402&#10;https://x.com/elonmusk&#10;https://xueqiu.com/P/ZH123456" aria-label="大V主页链接，每行一个" oninput="adminBatchLinesHint()"></textarea>
        <div class="toolbar ak-add-bar">
          <select id="ad-batch-category" class="form-control" aria-label="分类"><option value="">未分类</option>${catOptions}</select>
          <button class="btn-normal" id="ad-batch-btn" onclick="adminBatchAddKols()">添加</button>
          <div id="ad-batch-result" class="muted ak-add-result"></div>
        </div>
      </section>
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">大V列表</h2>
          <p class="section-meta" id="admin-kols-meta">共 ${state.adminKolsTotal} 个大V · 优先约 60 秒抓一次，次要走低频摘要</p></div>
          <div class="toolbar ak-filters">
            <input id="ak-q" class="form-control" style="width:200px" placeholder="昵称 / 外部ID" value="${escapeHtml(state.adminKolsQ || "")}" onkeydown="if(event.key==='Enter')adminKolsApplyFilter()">
            <select id="ak-category" class="form-control" style="width:auto" onchange="adminKolsApplyFilter()"><option value="">全部分类</option>${catOptions}</select>
            <select id="ak-status" class="form-control" style="width:auto" onchange="adminKolsApplyFilter()">
              <option value="">全部状态</option>
              <option value="1" ${state.adminKolsStatus === "1" ? "selected" : ""}>启用</option>
              <option value="0" ${state.adminKolsStatus === "0" ? "selected" : ""}>停用</option>
            </select>
            <button type="button" class="btn-ghost ak-search-btn" onclick="adminKolsApplyFilter()">搜索</button>
            <button type="button" class="btn-ghost ak-clear-btn" onclick="adminKolsClearFilter()">清除</button>
          </div>
          <div class="platform-tabs ak-platform-tabs" id="admin-kols-tabs"></div>
        </header>
        <div class="toolbar admin-batch-bar" id="ak-batch-bar" style="margin-top:10px;display:${selCount ? "flex" : "none"};align-items:center;gap:8px;flex-wrap:wrap">
          <strong>已选 ${selCount} 个</strong>
          <button class="btn-sm" onclick="adminKolBatch('enable')">批量启用</button>
          <button class="btn-sm" onclick="adminKolBatch('disable')">批量停用</button>
          <button class="btn-sm" onclick="adminKolBatch('priority', true)">批量设优先</button>
          <button class="btn-sm" onclick="adminKolBatch('secondary', true)">批量设次要</button>
          <button class="btn-sm" onclick="adminKolBatch('normal')">批量设普通</button>
          <select id="ak-batch-category" class="form-control" style="width:auto"><option value="">批量改分类…</option>${catOptions}<option value="0">（清除分类）</option></select>
          <button class="btn-sm" onclick="adminKolBatchCategory()">应用分类</button>
          <button class="btn-sm danger" onclick="adminKolBatch('delete')">批量删除</button>
          <button class="btn-sm" onclick="adminKolClearSelect()">取消选择</button>
        </div>
        <div class="table-wrap">
          <table class="ak-table">
            <thead><tr><th scope="col" style="width:32px"><input type="checkbox" id="ak-checkall" onchange="adminKolTogglePage(this)" aria-label="全选当前页"></th><th scope="col">ID</th><th scope="col">平台</th><th scope="col">昵称</th><th scope="col">分类</th><th scope="col">外部ID</th><th scope="col">档位</th><th scope="col">原创</th><th scope="col">可见性</th><th scope="col">状态</th><th scope="col">操作</th></tr></thead>
            <tbody>${rows || `<tr class="ak-empty"><td colspan="11" class="muted">${state.adminKolsQ || state.adminKolsCategory || state.adminKolsStatus !== "" || state.adminKolsPlatform ? "没有匹配的大V" : "还没有大V，先用上方表单添加"}</td></tr>`}</tbody>
          </table>
        </div>
        <div class="pager">
          <button class="btn-sm" ${page <= 0 ? "disabled" : ""} aria-label="上一页" title="上一页" onclick="adminKolsPage(${page - 1})">${CHEVRON_LEFT_ICON} 上一页</button>
          <span class="pager-count">第 ${page + 1}/${pages} 页 · 共 ${state.adminKolsTotal} 个</span>
          <button class="btn-sm" ${page + 1 >= pages ? "disabled" : ""} aria-label="下一页" title="下一页" onclick="adminKolsPage(${page + 1})">下一页 ${CHEVRON_RIGHT_ICON}</button>
        </div>
      </section>`;
    // 回填筛选控件当前值（页面重建后）
    const qEl = $("#ak-q"); if (qEl) qEl.value = state.adminKolsQ || "";
    const catEl = $("#ak-category"); if (catEl) catEl.value = state.adminKolsCategory || "";
    const statusEl = $("#ak-status"); if (statusEl) statusEl.value = state.adminKolsStatus ?? "";
    adminKolSyncCheckall(kols);
    $("#admin-kols-tabs").innerHTML = PLATFORM_TABS.map((p) => platformTabHTML(p, state.adminKolsPlatform, "admin")).join("");
    return { hiddenFocus: focusIds.length > 0 && visibleFocus.length === 0 };
  }

  function switchAdminKolsPlatform(platform) {
    const qEl = $("#ak-q");
    if (qEl) state.adminKolsQ = qEl.value.trim();
    state.adminKolsPlatform = platform;
    state.adminKolsPage = 0;
    loadAdminKols();
  }

  function adminKolsApplyFilter() {
    state.adminKolsQ = $("#ak-q").value.trim();
    state.adminKolsCategory = $("#ak-category").value;
    state.adminKolsStatus = $("#ak-status").value;
    state.adminKolsPage = 0;
    loadAdminKols();
  }

  function adminKolsClearFilter() {
    state.adminKolsQ = "";
    state.adminKolsCategory = "";
    state.adminKolsStatus = "";
    state.adminKolsPlatform = "";
    state.adminKolsPage = 0;
    loadAdminKols();
  }

  function adminKolSyncCheckall(kols) {
    const list = kols || state.adminKols || [];
    const checkall = $("#ak-checkall");
    if (!checkall) return;
    const pageSelected = list.filter((k) => _adminKolsSelected.has(k.id)).length;
    checkall.checked = !!list.length && pageSelected === list.length;
    checkall.indeterminate = pageSelected > 0 && pageSelected < list.length;
  }

  function adminKolsPage(page) {
    state.adminKolsPage = page;
    loadAdminKols();
  }

  function adminKolToggleSelect(el) {
    const id = Number(el.dataset.id);
    if (el.checked) _adminKolsSelected.add(id);
    else _adminKolsSelected.delete(id);
    const bar = $("#ak-batch-bar");
    if (bar) {
      bar.style.display = _adminKolsSelected.size ? "flex" : "none";
      const strong = bar.querySelector("strong");
      if (strong) strong.textContent = `已选 ${_adminKolsSelected.size} 个`;
    }
    adminKolSyncCheckall();
  }

  function adminKolTogglePage(el) {
    document.querySelectorAll(".kol-check").forEach((c) => {
      c.checked = el.checked;
      const id = Number(c.dataset.id);
      if (el.checked) _adminKolsSelected.add(id);
      else _adminKolsSelected.delete(id);
    });
    const bar = $("#ak-batch-bar");
    if (bar) {
      bar.style.display = _adminKolsSelected.size ? "flex" : "none";
      const strong = bar.querySelector("strong");
      if (strong) strong.textContent = `已选 ${_adminKolsSelected.size} 个`;
    }
    adminKolSyncCheckall();
  }

  function adminKolClearSelect() {
    _adminKolsSelected.clear();
    document.querySelectorAll(".kol-check").forEach((c) => { c.checked = false; });
    const bar = $("#ak-batch-bar");
    if (bar) bar.style.display = "none";
    const checkall = $("#ak-checkall");
    if (checkall) {
      checkall.checked = false;
      checkall.indeterminate = false;
    }
  }

  async function adminKolBatch(action, value) {
    const ids = [..._adminKolsSelected];
    if (!ids.length) return;
    if (action === "delete" && !confirm(`确认删除选中的 ${ids.length} 个大V？（将同时清理其订阅/帖子/推送记录）`)) return;
    const bar = $("#ak-batch-bar");
    const buttons = bar ? [...bar.querySelectorAll("button")] : [];
    buttons.forEach((b) => { b.disabled = true; });
    try {
      await api("/api/admin/kols/batch", {
        method: "POST",
        body: JSON.stringify({ ids, action, value: value ?? null }),
      });
      flash(action === "normal" ? `已将 ${ids.length} 个大V设为普通档` : `已对 ${ids.length} 个大V执行批量操作`);
      _adminKolsSelected.clear();
      loadAdminKols();
    } catch (err) {
      flash("批量操作失败: " + err.message, "error");
      buttons.forEach((b) => { b.disabled = false; });
    }
  }

  async function adminKolBatchCategory() {
    const value = $("#ak-batch-category").value;
    if (value === "") { flash("请选择要应用到的分类", "error"); return; }
    await adminKolBatch("category", value === "0" ? null : Number(value));
  }

  async function adminBatchAddKols() {
    const lines = $("#ad-batch-lines").value;
    if (!lines.trim()) {
      flash("请先填写要添加的大V主页链接", "error");
      return;
    }
    const category = $("#ad-batch-category").value;
    const btn = $("#ad-batch-btn");
    if (btn) btn.disabled = true;
    try {
      const data = await api("/api/kols/batch", {
        method: "POST",
        body: JSON.stringify({
          lines,
          category_id: category ? Number(category) : null,
        }),
      });
      const failLines = data.failed.map((f) => `${f.line} — ${f.error}`).join("\n");
      const view = await loadAdminKols({ focusIds: data.ids || [] });
      const resultEl = $("#ad-batch-result");
      if (resultEl) {
        resultEl.textContent = data.failed.length
          ? `成功 ${data.ok}/${data.total}，失败 ${data.failed.length} 条${failLines ? `\n${failLines}` : ""}`
          : `成功 ${data.ok}/${data.total}`;
        resultEl.style.color = data.failed.length ? "var(--color-danger)" : "var(--color-success)";
        resultEl.style.fontWeight = "600";
      }
      const hidden = view && view.hiddenFocus && data.ok;
      flash(data.failed.length
        ? `添加完成：成功 ${data.ok}/${data.total}，失败 ${data.failed.length} 条${hidden ? "，不在当前筛选里" : ""}`
        : hidden
          ? `添加成功：${data.ok} 个，不在当前筛选里`
          : `添加成功：${data.ok} 个`);
    } catch (err) {
      flash("添加失败: " + err.message, "error");
    } finally {
      if (btn) btn.disabled = false;
    }
  }

  function adminBatchLinesHint() {
    const lines = $("#ad-batch-lines")?.value || "";
    if (!/(?:xueqiu\.com\/P\/|ZH\d)/.test(lines)) return;
    const cat = $("#ad-batch-category");
    if (!cat || cat.value) return;
    for (const opt of cat.options) {
      if (opt.textContent.trim() === "实盘") { cat.value = opt.value; break; }
    }
  }

  async function adminToggleKol(id, enabled) {
    const kol = state.adminKols.find((k) => k.id === id);
    try {
      await api(`/api/kols/${id}`, { method: "PUT", body: JSON.stringify({ enabled: !!enabled }) });
      flash(`已${enabled ? "启用" : "停用"}「${kol ? kol.name : "该大V"}」`);
      loadAdminKols();
    } catch (err) {
      flash("操作失败: " + err.message, "error");
    }
  }

  async function adminTogglePriority(id, priority) {
    const kol = state.adminKols.find((k) => k.id === id);
    try {
      await api(`/api/kols/${id}`, { method: "PUT", body: JSON.stringify({ priority: !!priority }) });
      flash(`已${priority ? "设为优先" : "改为普通档"}「${kol ? kol.name : "该大V"}」`);
      loadAdminKols();
    } catch (err) {
      flash("操作失败: " + err.message, "error");
    }
  }

  async function adminToggleSecondary(id, secondary) {
    const kol = state.adminKols.find((k) => k.id === id);
    try {
      await api(`/api/kols/${id}`, { method: "PUT", body: JSON.stringify({ secondary: !!secondary }) });
      flash(`已${secondary ? "设为次要" : "改为普通档"}「${kol ? kol.name : "该大V"}」`);
      loadAdminKols();
    } catch (err) {
      flash("操作失败: " + err.message, "error");
    }
  }

  async function adminDeleteKol(id) {
    const kol = state.adminKols.find((k) => k.id === id);
    const subs = Number(kol && kol.subscriber_count) || 0;
    if (!confirm(`确认删除该大V${kol ? `「${kol.name}」` : ""}？将同时清理 ${subs} 个订阅及其帖子/推送记录。`)) return;
    try {
      await api(`/api/kols/${id}`, { method: "DELETE" });
      flash(`已删除「${kol ? kol.name : "该大V"}」`);
      loadAdminKols();
    } catch (err) {
      flash("删除失败: " + err.message, "error");
    }
  }

  function adminKolEditSnapshot() {
    return JSON.stringify({
      name: $("#ek-name").value.trim(),
      category: $("#ek-category").value,
      priv: $("#ek-private").checked,
      orig: $("#ek-original") ? $("#ek-original").checked : false,
      users: $("#ek-users").value.trim(),
    });
  }

  async function adminEditKol(id) {
    let kol, categories;
    try {
      [kol, categories] = await Promise.all([api(`/api/kols/${id}`), api("/api/categories")]);
    } catch (err) {
      flash("加载失败: " + err.message, "error");
      return;
    }
    const catOptions = categories.map((c) => `<option value="${c.id}" ${kol.category_id === c.id ? "selected" : ""}>${escapeHtml(c.name)}</option>`).join("");
    const mask = document.createElement("div");
    mask.className = "modal-mask";
    mask.innerHTML = `
      <div class="modal-card" role="dialog" aria-modal="true" aria-labelledby="ek-title">
        <h3 id="ek-title" style="margin-bottom:12px">编辑大V：${escapeHtml(kol.name)}</h3>
        <label class="form-label">昵称
          <input id="ek-name" class="form-control" value="${escapeHtml(kol.name)}">
        </label>
        <label class="form-label">分类
          <select id="ek-category" class="form-control"><option value="">未分类</option>${catOptions}</select>
        </label>
        <label class="form-label" title="订阅广场「推荐」位排序：越大越靠前，优先于订阅人数">
          推荐权重
          <input id="ek-weight" class="form-control" type="number" min="0" step="1" value="${Number(kol.recommend_weight) || 0}">
        </label>
        <label class="form-label" style="display:flex;align-items:center;gap:8px">
          <input id="ek-private" type="checkbox" ${kol.is_private ? "checked" : ""} onchange="document.getElementById('ek-users-wrap').hidden=!this.checked"> 私有大V（仅白名单用户可见/可订阅）
        </label>
        ${kol.platform === "weibo" ? `<label class="form-label" style="display:flex;align-items:center;gap:8px">
          <input id="ek-original" type="checkbox" ${kol.original_only ? "checked" : ""}> 只看原创（微博跳过转发，适合转发刷屏的大V）
        </label>` : ""}
        <label class="form-label" id="ek-users-wrap" ${kol.is_private ? "" : "hidden"}>白名单用户（逗号分隔用户名）
          <input id="ek-users" class="form-control" value="${escapeHtml((kol.visible_users || []).join(", "))}" placeholder="user1, user2">
        </label>
        <div class="toolbar" style="margin-top:16px">
          <button class="btn-normal" id="ek-save" onclick="saveKolEdit(${kol.id})">保存</button>
          <button type="button" class="btn-sm" data-close>取消</button>
        </div>
      </div>`;
    const initial = (() => {
      document.body.appendChild(mask);
      return adminKolEditSnapshot();
    })();
    const tryClose = () => {
      if (adminKolEditSnapshot() !== initial && !confirm("有未保存的修改，确定关闭？")) return;
      mask.remove();
    };
    mask.addEventListener("click", (e) => {
      if (e.target === mask) tryClose();
    });
    mask.querySelector("[data-close]").addEventListener("click", tryClose);
    trapFocus(mask, tryClose);
    mask.querySelector("input, select, textarea, button")?.focus();
  }

  async function saveKolEdit(id) {
    const mask = document.querySelector(".modal-mask");
    const name = $("#ek-name").value.trim();
    const isPrivate = $("#ek-private").checked;
    const visibleUsers = $("#ek-users").value.split(",").map((s) => s.trim()).filter(Boolean);
    if (isPrivate && !visibleUsers.length) {
      if (!confirm("白名单为空，该大V将对所有人隐藏。仍要保存？")) return;
    }
    const body = {
      name,
      category_id: $("#ek-category").value ? Number($("#ek-category").value) : null,
      is_private: isPrivate,
      visible_users: visibleUsers,
      recommend_weight: Math.max(Number($("#ek-weight")?.value) || 0, 0),
    };
    if ($("#ek-original")) body.original_only = $("#ek-original").checked;
    const btn = $("#ek-save");
    if (btn) btn.disabled = true;
    try {
      await api(`/api/kols/${id}`, {
        method: "PUT",
        body: JSON.stringify(body),
      });
      if (mask) mask.remove();
      flash(`已保存「${name}」`);
      loadAdminKols();
    } catch (err) {
      flash("保存失败: " + err.message, "error");
      if (btn) btn.disabled = false;
    }
  }

  async function loadAdminVocab() {
    // 深链：vtab=tags 进标签 Tab，其余值（含无参数）进分类 Tab；旧 /admin/vocab?tab=tags 由路由重定向带 vtab
    const params = routeQuery();
    const tab = params.get("vtab") === "tags" ? "tags" : "categories";
    if (!routeStillActive(currentAdminSeq())) return;
    $("#admin-body").innerHTML = `
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">标签分类</h2>
          <p class="section-meta">分类按大V分组（订阅广场/动态页/管理列表筛选）；标签按关键词规则给贴文内容自动打标。</p></div>
          <div class="settings-tabs" role="tablist" aria-label="标签分类">
            <button class="settings-tab ${tab === "categories" ? "active" : ""}" data-tab="categories" onclick="go('admin/content?tab=vocab')">分类</button>
            <button class="settings-tab ${tab === "tags" ? "active" : ""}" data-tab="tags" onclick="go('admin/content?tab=vocab&vtab=tags')">标签</button>
          </div>
        </header>
        <div id="vocab-tab-body" class="settings-tab-panel"></div>
      </section>`;
    await loadAdminVocabTab(tab);
  }

  async function loadAdminVocabTab(tab) {
    if (tab === "tags") return loadAdminTagsTab();
    return loadAdminCategoriesTab();
  }

  async function loadAdminCategoriesTab() {
    const categories = await api("/api/categories");
    if (!routeStillActive(currentAdminSeq())) return;
    $("#vocab-tab-body").innerHTML = `
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">添加分类</h2></div>
          <div class="toolbar" style="margin-top:12px">
            <input id="cat-name" class="form-control" style="margin:0;width:280px" placeholder="分类名，如：实盘、宏观、行业研究">
            <button class="btn-normal" onclick="adminAddCategory()">添加分类</button>
          </div>
        </header>
      </section>
      <section class="section-panel">
        <header class="section-head"><div><h2 class="section-title">分类列表</h2></div></header>
        <div class="table-wrap">
          <table>
            <thead><tr><th scope="col">ID</th><th scope="col">分类名</th><th scope="col">大V数</th><th scope="col">操作</th></tr></thead>
            <tbody>${categories.map((c) => `
              <tr>
                <td>${c.id}</td><td>${escapeHtml(c.name)}</td><td>${c.kol_count}</td>
                <td>
                  <button class="btn-sm" onclick="adminRenameCategory(${c.id})">重命名</button>
                  <button class="btn-sm danger" onclick="adminDeleteCategory(${c.id})">删除</button>
                </td>
              </tr>`).join("")}</tbody>
          </table>
        </div>
      </section>`;
  }

  async function adminAddCategory() {
    const name = $("#cat-name").value.trim();
    if (!name) {
      alert("请输入分类名");
      return;
    }
    try {
      await api("/api/categories", { method: "POST", body: JSON.stringify({ name }) });
      flash(`已添加分类「${name}」`);
      loadAdminVocabTab("categories");
    } catch (err) {
      alert("添加失败: " + err.message);
    }
  }

  async function adminRenameCategory(id) {
    const name = prompt("新的分类名：");
    if (name === null || !name.trim()) return;
    try {
      await api(`/api/categories/${id}`, { method: "PUT", body: JSON.stringify({ name: name.trim() }) });
      flash("已重命名分类");
      loadAdminVocabTab("categories");
    } catch (err) {
      alert("重命名失败: " + err.message);
    }
  }

  async function adminDeleteCategory(id) {
    if (!confirm("确认删除该分类？其下大V将变为未分类")) return;
    try {
      await api(`/api/categories/${id}`, { method: "DELETE" });
      flash("已删除分类");
      loadAdminVocabTab("categories");
    } catch (err) {
      alert("删除失败: " + err.message);
    }
  }

  async function loadAdminTagsTab() {
    const data = await api("/api/tags");
    const tags = Array.isArray(data?.tags) ? data.tags : [];
    const stockNames = Array.isArray(data?.stock_names) ? data.stock_names : [];
    const stockAliases = Array.isArray(data?.stock_aliases) ? data.stock_aliases : [];
    const excludedNames = Array.isArray(data?.excluded_stock_names) ? data.excluded_stock_names : [];
    const universe = data?.universe && typeof data.universe === "object" ? data.universe : {};
    const universeCount = Number(universe.count) || 0;
    const universeUpdated = universe.updated ? String(universe.updated) : "";
    const stats = data?.stats || { total: 0, processed: 0, tagged: 0, pending: 0 };
    if (!routeStillActive(currentAdminSeq())) return;
    // 词表编辑：每行一个标签，格式「标签名 | 关键词,关键词」；关键词为空则该标签不命中
    const vocabText = tags.map((r) => `${r.tag} | ${(r.keywords || []).join(", ")}`).join("\n");
    // 别名表编辑：每行「别名=正式名」
    const aliasText = stockAliases.map((a) => `${a.alias}=${a.stock}`).join("\n");
    $("#vocab-tab-body").innerHTML = `
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">贴文标签词表</h2>
          <p class="section-meta">新帖抓取入库时按关键词规则自动打标（零成本、不依赖 LLM）。每行一个标签：<b>标签名 | 关键词1,关键词2</b>，正文/标题命中任一关键词即打该标签，每条最多 3 个。</p></div>
        </header>
        <textarea id="tag-vocab-input" class="form-control" rows="10" style="margin-top:12px;font-family:monospace;line-height:1.6" placeholder="宏观 | 央行,降息,GDP&#10;大盘 | A股,沪指,指数">${escapeHtml(vocabText)}</textarea>
        <div class="toolbar" style="margin-top:12px">
          <button class="btn-normal" onclick="adminSaveTags()">保存词表</button>
        </div>
        <p class="section-meta" style="margin-top:8px">已处理 ${stats.processed} / ${stats.total} 条，其中有标签 ${stats.tagged} 条，待处理 ${stats.pending} 条</p>
      </section>
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">常用股票名</h2>
          <p class="section-meta">管理员可增删，每行一个。纯文字提及会打股票标签；$股票名(代码)$ 仍自动识别。删掉的名字每日维护不会加回，再写进列表并保存即可恢复。两字名只认这张表。</p></div>
        </header>
        <textarea id="stock-names-input" class="form-control" rows="8" style="margin-top:12px;font-family:monospace;line-height:1.6" placeholder="贵州茅台&#10;宁德时代">${escapeHtml(stockNames.join("\n"))}</textarea>
        <div class="toolbar" style="margin-top:12px">
          <button class="btn-normal" onclick="adminSaveStockNames()">保存股票名</button>
        </div>
        ${universeCount ? `<p class="section-meta" style="margin-top:8px">另有全市场 ${universeCount} 只 3 字及以上正式简称参与纯文字打标${universeUpdated ? `（${escapeHtml(universeUpdated)}）` : ""}，不占手改名单。</p>` : ""}
        ${excludedNames.length ? `<p class="section-meta" style="margin-top:8px">维护不加回：${excludedNames.map((n) => escapeHtml(n)).join("、")}</p>` : ""}
      </section>
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">黑话别名</h2>
          <p class="section-meta">常见黑话（宁王、药茅）启动时写入；雪球 $戏称(代码)$ 由系统 LLM 解析。正式名切半（宁德/英伟）不会入库。每行「别名=正式名」，正式名需在常用表或全市场名表中。</p></div>
        </header>
        <textarea id="stock-aliases-input" class="form-control" rows="5" style="margin-top:12px;font-family:monospace;line-height:1.6" placeholder="宁王=宁德时代">${escapeHtml(aliasText)}</textarea>
      </section>
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">标签维护</h2>
          <p class="section-meta">合并种子黑话、解析 $标记$ 新股、去掉指数/ETF 误入的股票名，并清理过期标签与碎片别名。每日自动一次，也可立即执行。标记解析走站点环境变量 LLM。</p></div>
        </header>
        <p class="section-meta" style="margin-top:8px" id="tag-maintain-meta">${escapeHtml(adminMaintainSummary(data))}</p>
        ${data.maintain && data.maintain.llm_ready ? "" : `<p class="section-meta">未检测到站点 LLM。请设环境变量 LLM_API_KEY。点运行仍会合并种子、清碎片和误标。</p>`}
        <div class="toolbar" style="margin-top:12px">
          <button class="btn-normal" onclick="adminMaintainTags('pending')">维护并回填待打标</button>
          <button class="btn-ghost" onclick="adminMaintainTags('none')">仅维护词表</button>
          <button class="btn-ghost" onclick="adminMaintainTags('all')">维护并重算全部</button>
          <span id="tag-maintain-result" class="muted"></span>
        </div>
      </section>
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">回填历史贴文</h2>
          <p class="section-meta">给未打标贴文按当前词表 + 股票名单补标签；「按当前规则重算全部」会覆盖全部历史贴文标签（危险操作，需确认）。</p></div>
        </header>
        <div class="toolbar" style="margin-top:12px">
          <button class="btn-normal" onclick="adminBackfillTags('pending')">处理待打标</button>
          <button class="btn-ghost" onclick="adminBackfillTags('all')">按当前规则重算全部</button>
          <span id="tag-backfill-result" class="muted"></span>
        </div>
      </section>
      <section class="section-panel">
        <header class="section-head"><div><h2 class="section-title">当前词表（${tags.length} 个）</h2></div></header>
        <div class="tag-vocab-preview">
          ${tags.length ? tags.map((r) => `<span class="cat cat-tag">${escapeHtml(r.tag)}</span>`).join("") : "（空）"}
        </div>
      </section>`;
  }

  async function adminSaveStockNames() {
    const stockNames = $("#stock-names-input").value.split(/\n/).map((s) => s.trim()).filter(Boolean);
    try {
      const data = await api("/api/tags", { method: "PUT", body: JSON.stringify({ stock_names: stockNames }) });
      const dropped = data.dropped_aliases || [];
      flash(dropped.length
        ? `已保存 ${data.stock_names.length} 只股票，去掉别名 ${dropped.map((a) => a.alias).join("、")}`
        : `已保存 ${data.stock_names.length} 只股票`);
      loadAdminVocabTab("tags");
    } catch (err) {
      alert("保存失败: " + err.message);
    }
  }

  async function adminSaveTags() {
    const raw = $("#tag-vocab-input").value;
    const tags = raw.split(/\n/).map((line) => line.trim()).filter(Boolean).map((line) => {
      // 每行「标签名 | 关键词,关键词」；无 | 时整行视为标签名（无关键词）
      const [tag, kw] = line.split("|").map((s) => s.trim());
      const keywords = kw ? kw.split(/[,，]/).map((k) => k.trim()).filter(Boolean) : [];
      return { tag, keywords };
    }).filter((r) => r.tag);
    const stockNames = $("#stock-names-input").value.split(/\n/).map((s) => s.trim()).filter(Boolean);
    const stockAliases = $("#stock-aliases-input").value.split(/\n/).map((line) => line.trim()).filter(Boolean).map((line) => {
      const [alias, stock] = line.split(/[=＝]/).map((s) => s.trim());
      return { alias, stock };
    }).filter((r) => r.alias && r.stock);
    try {
      const data = await api("/api/tags", { method: "PUT", body: JSON.stringify({ tags, stock_names: stockNames, stock_aliases: stockAliases }) });
      flash(`已保存词表（${data.tags.length} 个标签，${data.stock_names.length} 只股票，${data.stock_aliases.length} 个别名）`);
      loadAdminVocabTab("tags");
    } catch (err) {
      alert("保存失败: " + err.message);
    }
  }

  function adminMaintainSummary(data) {
    const last = data && data.maintain && data.maintain.last;
    if (!last || !last.at) return "尚未执行过";
    const parts = [`上次 ${last.at}`];
    if (last.llm_model) parts.push(String(last.llm_model));
    if (last.llm_used) parts.push("LLM 已返回");
    const aliases = last.added_aliases || [];
    const names = last.added_stock_names || [];
    const removed = last.removed_stock_names || [];
    const purged = last.purged_aliases || [];
    const seeded = last.seeded_aliases || [];
    if (aliases.length) parts.push(`新增别名 ${aliases.length}`);
    if (seeded.length) parts.push(`种子 ${seeded.length}`);
    if (purged.length) parts.push(`清除碎片 ${purged.length}`);
    if (names.length) parts.push(`新增股票 ${names.length}`);
    if (removed.length) parts.push(`移除非个股 ${removed.length}`);
    if (last.cleaned) parts.push(`清理 ${last.cleaned} 条`);
    if (last.backfill) parts.push(`回填 ${last.backfill.processed} 条`);
    if (last.error) parts.push("识别异常");
    return parts.join(" · ");
  }

  function formatMaintainResult(data) {
    const bits = [];
    const aliases = data.added_aliases || [];
    const names = data.added_stock_names || [];
    const removed = data.removed_stock_names || [];
    const purged = data.purged_aliases || [];
    if (aliases.length) {
      bits.push("新增别名 " + aliases.map((a) => `${a.alias}→${a.stock}`).join("、"));
    } else {
      bits.push("无新别名");
    }
    if (purged.length) bits.push("清除碎片 " + purged.map((a) => a.alias).join("、"));
    if (names.length) bits.push("新增股票 " + names.join("、"));
    if (removed.length) bits.push("移除 " + removed.join("、"));
    if (data.llm_used) bits.push("LLM 已返回");
    else if (data.error) bits.push("识别异常：" + data.error);
    if (data.cleaned) bits.push(`清理误标 ${data.cleaned} 条`);
    if (data.backfill) bits.push(`回填 ${data.backfill.processed} 条，其中 ${data.backfill.tagged} 条有标签`);
    if (data.error && data.llm_used) bits.push("识别异常：" + data.error);
    return bits.join("；");
  }

  async function adminMaintainTags(backfill = "pending") {
    if (backfill === "all" && !confirm("将覆盖全部历史贴文标签，确定继续？")) return;
    const buttons = document.querySelectorAll("[onclick^='adminMaintainTags']");
    buttons.forEach((button) => { button.disabled = true; });
    const result = $("#tag-maintain-result");
    if (result) result.textContent = backfill === "none" ? "维护中…" : "维护并回填中…";
    try {
      const data = await api("/api/tags/maintain", {
        method: "POST",
        body: JSON.stringify({ backfill }),
      });
      if (result) result.textContent = formatMaintainResult(data);
      flash(backfill === "none" ? "标签维护完成" : "标签维护并回填完成");
      loadAdminVocabTab("tags");
    } catch (err) {
      if (result) result.textContent = "";
      alert("维护失败: " + err.message);
    } finally {
      buttons.forEach((button) => { button.disabled = false; });
    }
  }

  async function adminBackfillTags(mode = "pending") {
    if (mode === "all" && !confirm("将覆盖全部历史贴文标签，确定继续？")) return;
    const buttons = document.querySelectorAll("[onclick^='adminBackfillTags']");
    buttons.forEach((button) => { button.disabled = true; });
    const result = $("#tag-backfill-result");
    if (result) result.textContent = mode === "all" ? "全量重算中…" : "处理中…";
    try {
      const data = await api("/api/tags/backfill", {
        method: "POST",
        body: JSON.stringify({ mode }),
      });
      if (result) result.textContent = `已处理 ${data.processed} 条，其中 ${data.tagged} 条有标签`;
      flash(mode === "all" ? "全量重算完成" : "待打标处理完成");
      loadAdminVocabTab("tags");
    } catch (err) {
      if (result) result.textContent = "";
      alert("处理失败: " + err.message);
    } finally {
      buttons.forEach((button) => { button.disabled = false; });
    }
  }



  return {
    loadAdminKols,
    loadAdminVocab,
    switchAdminKolsPlatform,
    adminKolsApplyFilter,
    adminKolsClearFilter,
    adminKolsPage,
    adminKolToggleSelect,
    adminKolTogglePage,
    adminKolClearSelect,
    adminKolBatch,
    adminKolBatchCategory,
    adminBatchAddKols,
    adminBatchLinesHint,
    adminToggleKol,
    adminTogglePriority,
    adminToggleSecondary,
    adminDeleteKol,
    adminEditKol,
    saveKolEdit,
    adminAddCategory,
    adminRenameCategory,
    adminDeleteCategory,
    adminSaveStockNames,
    adminSaveTags,
    adminMaintainTags,
    adminBackfillTags,
  };
}
