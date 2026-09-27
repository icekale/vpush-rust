export function createAdminUsersView(dependencies) {
  const {
    $,
    state,
    api,
    flash,
    escapeHtml,
    routeStillActive,
    currentAdminSeq,
    emptyState,
    SEARCH_ICON,
    fmtDbTime,
    trapFocus,
    renderSidebar,
    renderTopbar,
    USER_CHANNEL_KEYS,
    CHANNEL_LABELS,
    CHANNEL_ICONS,
    PLATFORM_LABELS,
    usernameRuleError,
    CHEVRON_RIGHT_ICON,
  } = dependencies;

  function userHasBoundChannel(u) {
    return !!(u.telegram_bound || u.feishu_bound || u.wecom_bound || u.bark_bound || u.webpush_bound);
  }

  function userChannelIconsHtml(u) {
    const bound = {
      telegram: !!u.telegram_bound,
      feishu: !!u.feishu_bound,
      wecom: !!u.wecom_bound,
      bark: !!u.bark_bound,
      webpush: !!u.webpush_bound,
    };
    const names = USER_CHANNEL_KEYS.filter((ch) => bound[ch]).map((ch) => CHANNEL_LABELS[ch]);
    const aria = names.length ? `已绑定 ${names.join("、")}` : "未绑定推送渠道";
    return `<span class="user-channels" title="${escapeHtml(aria)}" aria-label="${escapeHtml(aria)}">${
      USER_CHANNEL_KEYS.map((ch) =>
        `<span class="user-ch ${bound[ch] ? "on" : "off"}" data-channel="${ch}">${CHANNEL_ICONS[ch]}</span>`
      ).join("")
    }</span>`;
  }

  function adminUsersFiltered() {
    const q = (state.adminUsersQ || "").trim().toLowerCase();
    const filter = state.adminUsersFilter || "all";
    return (state.adminUsers || []).filter((u) => {
      if (filter === "admin" && !u.is_admin) return false;
      if (filter === "unbound" && userHasBoundChannel(u)) return false;
      if (filter === "push-off" && u.notify_enabled) return false;
      if (filter === "inactive" && !u.inactive) return false;
      if (!q) return true;
      return [u.username, u.origin_label, u.register_code, u.register_note].some(
        (s) => String(s || "").toLowerCase().includes(q)
      );
    });
  }

  function adminUserBoundLabels(u) {
    return USER_CHANNEL_KEYS.filter((ch) => u[`${ch}_bound`]).map((ch) => CHANNEL_LABELS[ch]);
  }

  function adminUserOriginHtml(u) {
    const label = escapeHtml(u.origin_label || "网页");
    const note = escapeHtml(u.register_note || u.register_code || "");
    if (u.origin === "invite" && note) {
      return `<div class="user-origin"><span>${label}</span><span class="muted">${note}</span></div>`;
    }
    return `<div class="user-origin"><span>${label}</span></div>`;
  }

  function adminUserLoginHtml(u) {
    const login = u.has_password
      ? `<span class="status-ok">可登录</span>`
      : `<span class="muted">无密码</span>`;
    const seen = u.last_login_at
      ? escapeHtml(fmtDbTime(u.last_login_at))
      : "从未登录";
    return `<div class="user-login">${login}<span class="muted">${seen}</span></div>`;
  }

  function adminDeleteImpact(users) {
    const list = (users || []).filter(Boolean);
    const subs = list.reduce((n, u) => n + (Number(u.subscription_count) || 0), 0);
    const bound = list.filter(userHasBoundChannel).length;
    const parts = [`订阅 ${subs} 个`];
    if (list.length === 1) {
      const names = adminUserBoundLabels(list[0]);
      if (names.length) parts.push(`已绑 ${names.join("、")}`);
    } else if (bound) {
      parts.push(`${bound} 人已绑渠道`);
    }
    return parts.join(" · ");
  }

  let _adminUsersSelected = new Set();
  let _inactivePolicyDraft = null;
  let _inactivePolicySaving = false;
  let _auPolicyOpen = false;
  let _inactivePreview = { marked_count: 0, purge_count: 0 };
  let _inactivePreviewTimer = 0;
  let _inactivePreviewSeq = 0;

  function inactivePolicySaved() {
    return state.inactivePolicy || { inactive_after_days: 90, inactive_purge_after_days: 30, customized: false };
  }

  function inactivePolicyDraft() {
    return _inactivePolicyDraft || inactivePolicySaved();
  }

  function inactivePolicyRuleLabel() {
    const saved = inactivePolicySaved();
    const n = Number(saved.inactive_after_days);
    const m = Number(saved.inactive_purge_after_days);
    return saved.customized ? `规则 ${n}+${m}` : `默认 ${n}+${m}`;
  }

  function inactivePolicyHint(n, m, preview) {
    n = Number(n);
    m = Number(m);
    preview = preview || _inactivePreview || {};
    const marked = Number(preview.marked_count);
    const doomed = Number(preview.purge_count);
    const blast = Number.isFinite(marked)
      ? (Number.isFinite(doomed) && doomed > 0 ? `现标 ${marked} 人，下次删 ${doomed} 人` : `现标 ${marked} 人`)
      : "";
    let core;
    if (!Number.isFinite(n) || n <= 0) core = "已关闭标记与删除";
    else if (!Number.isFinite(m) || m <= 0) core = "只标记，不自动删除";
    else if (!inactivePolicySaved().customized && n === 90 && m === 30) core = `未改过 · 默认 ${n}+${m}`;
    else core = `每天扫一次 · 满 ${n + m} 天删除`;
    return blast ? `${core} · ${blast}` : core;
  }

  function paintInactivePolicyHint() {
    const draft = inactivePolicyDraft();
    const text = inactivePolicyHint(draft.inactive_after_days, draft.inactive_purge_after_days);
    const hint = $("#au-inactive-hint");
    const summary = document.querySelector("details.au-policy > summary .muted");
    if (hint) hint.textContent = text;
    if (summary) summary.textContent = text;
  }

  function adminInactivePolicySyncSave(queuePreview) {
    const nEl = $("#au-inactive-n");
    const mEl = $("#au-inactive-m");
    const btn = $("#au-inactive-save");
    if (!nEl || !mEl) return;
    _inactivePolicyDraft = {
      inactive_after_days: nEl.value,
      inactive_purge_after_days: mEl.value,
    };
    paintInactivePolicyHint();
    const saved = inactivePolicySaved();
    const dirty =
      Number(nEl.value) !== Number(saved.inactive_after_days) ||
      Number(mEl.value) !== Number(saved.inactive_purge_after_days);
    if (btn) btn.disabled = !dirty || _inactivePolicySaving;
    if (queuePreview) adminInactivePolicyQueuePreview();
  }

  function adminInactivePolicyQueuePreview() {
    const seq = ++_inactivePreviewSeq;
    clearTimeout(_inactivePreviewTimer);
    _inactivePreviewTimer = setTimeout(() => adminRefreshInactivePreview(seq), 360);
  }

  async function adminRefreshInactivePreview(seq) {
    const draft = inactivePolicyDraft();
    const n = Number(draft.inactive_after_days);
    const m = Number(draft.inactive_purge_after_days);
    if (!Number.isInteger(n) || !Number.isInteger(m) || n < 0 || n > 3650 || m < 0 || m > 3650) return;
    try {
      const data = await api(
        `/api/admin/inactive-users-policy?inactive_after_days=${n}&inactive_purge_after_days=${m}`
      );
      if (seq && seq !== _inactivePreviewSeq) return;
      _inactivePreview = {
        marked_count: Number(data.marked_count) || 0,
        purge_count: Number(data.purge_count) || 0,
      };
      paintInactivePolicyHint();
    } catch {
      /* 输入过程中的预览失败不打断保存 */
    }
  }

  function adminInactivePolicyKeydown(event) {
    if (event.key !== "Enter") return;
    event.preventDefault();
    adminSaveInactivePolicy();
  }

  function adminUsersSyncBar() {
    const bar = $("#au-batch-bar");
    if (!bar) return;
    bar.style.display = _adminUsersSelected.size ? "flex" : "none";
    const strong = bar.querySelector("strong");
    if (strong) strong.textContent = `已选 ${_adminUsersSelected.size} 人`;
  }

  function adminUserToggleSelect(el) {
    const id = Number(el.dataset.id);
    if (el.checked) _adminUsersSelected.add(id);
    else _adminUsersSelected.delete(id);
    adminUsersSyncBar();
    const checkall = $("#au-checkall");
    const boxes = [...document.querySelectorAll(".au-check")];
    if (checkall) {
      checkall.checked = boxes.length > 0 && boxes.every((c) => c.checked);
      checkall.indeterminate = boxes.some((c) => c.checked) && !checkall.checked;
    }
  }

  function adminUserTogglePage(el) {
    document.querySelectorAll(".au-check").forEach((c) => {
      c.checked = el.checked;
      const id = Number(c.dataset.id);
      if (el.checked) _adminUsersSelected.add(id);
      else _adminUsersSelected.delete(id);
    });
    el.indeterminate = false;
    adminUsersSyncBar();
  }

  function adminUserClearSelect() {
    _adminUsersSelected.clear();
    document.querySelectorAll(".au-check").forEach((c) => { c.checked = false; });
    const checkall = $("#au-checkall");
    if (checkall) {
      checkall.checked = false;
      checkall.indeterminate = false;
    }
    adminUsersSyncBar();
  }

  async function adminUsersBatch(action) {
    const ids = [..._adminUsersSelected];
    if (!ids.length) return;
    let payloadIds = ids;
    if (action === "delete") {
      const picked = ids.map((id) => (state.adminUsers || []).find((u) => u.id === id)).filter(Boolean);
      const blocked = picked.filter((u) => u.is_admin || (state.user && u.id === state.user.id)).length;
      const doomed = picked.filter((u) => !u.is_admin && !(state.user && u.id === state.user.id));
      if (!doomed.length) {
        flash(blocked ? "选中的都是管理员，已跳过" : "没有可删除的用户", "error");
        return;
      }
      const extra = blocked ? `\n将跳过 ${blocked} 个管理员。` : "";
      if (!confirm(`确认删除选中的 ${doomed.length} 个用户？${extra}\n${adminDeleteImpact(doomed)}\n删除后不可恢复。`)) return;
      payloadIds = doomed.map((u) => u.id);
    }
    try {
      const data = await api("/api/admin/users/batch", {
        method: "POST",
        body: JSON.stringify({ ids: payloadIds, action }),
      });
      const n = data.count || 0;
      const skipped = data.skipped || 0;
      if (action === "delete") {
        flash(skipped ? `已删除 ${n} 人，跳过 ${skipped} 个管理员或无效项` : `已删除 ${n} 人`);
      } else if (action === "enable_notify") {
        flash(`已开启 ${n} 人推送`);
      } else if (action === "disable_notify") {
        flash(`已关闭 ${n} 人推送`);
      }
      _adminUsersSelected.clear();
      loadAdminUsers();
    } catch (err) {
      flash(err.message, "error");
    }
  }

  async function loadAdminUsers() {
    let users;
    let policy;
    let collector;
    let localLibs;
    try {
      [users, policy, collector, localLibs] = await Promise.all([
        api("/api/users"),
        api("/api/admin/inactive-users-policy"),
        api("/api/admin/ima-collector").catch(() => null),
        api("/api/admin/ima-local-libraries").catch(() => null),
      ]);
    } catch (err) {
      if (!routeStillActive(currentAdminSeq())) return;
      $("#admin-body").innerHTML = emptyState("加载失败: " + err.message);
      return;
    }
    state.adminUsers = users;
    const imaGroups = ((collector && collector.config && collector.config.groups) || [])
      .filter((group) => group && group.id && group.enabled !== false && !String(group.id).startsWith("feishu-"));
    const localGroups = (((localLibs && localLibs.libraries) || []) || [])
      .map((lib) => ({
        id: String(lib.group_id || ""),
        name: String(lib.name || lib.slug || lib.group_id || ""),
        enabled: Boolean(lib.enabled) && !lib.error,
        local: true,
      }))
      .filter((group) => group.id && group.name && !group.id.startsWith("feishu-"));
    state.imaKbGroups = imaGroups.concat(localGroups);
    if (policy) {
      state.inactivePolicy = policy;
      _inactivePreview = {
        marked_count: Number(policy.marked_count) || 0,
        purge_count: Number(policy.purge_count) || 0,
      };
    }
    const known = new Set(users.map((u) => u.id));
    for (const id of [..._adminUsersSelected]) {
      if (!known.has(id)) _adminUsersSelected.delete(id);
    }
    renderAdminUsers();
  }

  function adminUsersApplyFilter(filter) {
    const q = $("#au-q");
    if (q) state.adminUsersQ = q.value.trim();
    if (filter) state.adminUsersFilter = filter;
    renderAdminUsers();
  }

  async function adminSaveInactivePolicy() {
    const nEl = $("#au-inactive-n");
    const mEl = $("#au-inactive-m");
    const btn = $("#au-inactive-save");
    if (!nEl || !mEl || _inactivePolicySaving) return;
    const n = Number(nEl.value);
    const m = Number(mEl.value);
    if (!Number.isInteger(n) || !Number.isInteger(m) || n < 0 || n > 3650 || m < 0 || m > 3650) {
      flash("天数须在 0–3650", "error");
      return;
    }
    const saved = inactivePolicySaved();
    if (n === Number(saved.inactive_after_days) && m === Number(saved.inactive_purge_after_days)) return;
    _inactivePolicySaving = true;
    if (btn) btn.disabled = true;
    try {
      const seq = ++_inactivePreviewSeq;
      const preview = await api(
        `/api/admin/inactive-users-policy?inactive_after_days=${n}&inactive_purge_after_days=${m}`
      );
      if (seq !== _inactivePreviewSeq) return;
      _inactivePreview = {
        marked_count: Number(preview.marked_count) || 0,
        purge_count: Number(preview.purge_count) || 0,
      };
      paintInactivePolicyHint();
      if (
        _inactivePreview.purge_count > 0 &&
        !confirm(`下次扫描将删除 ${_inactivePreview.purge_count} 个未激活账号。确认按 ${n}+${m} 天保存？`)
      ) {
        return;
      }
      state.inactivePolicy = await api("/api/admin/inactive-users-policy", {
        method: "PUT",
        body: JSON.stringify({ inactive_after_days: n, inactive_purge_after_days: m }),
      });
      _inactivePolicyDraft = null;
      flash("已保存未激活规则");
      await loadAdminUsers();
    } catch (err) {
      flash(err.message, "error");
    } finally {
      _inactivePolicySaving = false;
      adminInactivePolicySyncSave();
    }
  }

  function renderAdminUsers() {
    if (!routeStillActive(currentAdminSeq())) return;
    const body = $("#admin-body");
    if (!body) return;
    const users = state.adminUsers || [];
    const filter = state.adminUsersFilter || "all";
    const filtered = adminUsersFiltered();
    const boundN = users.filter(userHasBoundChannel).length;
    const adminN = users.filter((u) => u.is_admin).length;
    const counts = {
      all: users.length,
      admin: adminN,
      unbound: users.filter((u) => !userHasBoundChannel(u)).length,
      "push-off": users.filter((u) => !u.notify_enabled).length,
      inactive: users.filter((u) => u.inactive).length,
    };
    const tab = (key, label) =>
      `<button class="settings-tab ${filter === key ? "active" : ""}" role="tab" aria-selected="${filter === key}" onclick="adminUsersApplyFilter('${key}')">${label} ${counts[key]}</button>`;
    const emptyMsg = users.length
      ? (filter === "inactive"
        ? "没有未激活账号。领码后从未登录、没绑渠道、没订阅的才会出现。"
        : "没有匹配的用户")
      : "还没有注册用户";
    const rows = filtered.map((u) => {
      const self = state.user && u.id === state.user.id;
      const pills = `${u.is_admin ? `<span class="user-pill">管理员</span>` : ""}${self ? `<span class="user-pill muted">本人</span>` : ""}${u.username_valid === false ? `<span class="user-pill warn">登录名不合规</span>` : ""}`;
      const push = u.inactive
        ? (u.days_until_purge == null
          ? `<span class="status-warn">未激活</span>`
          : `<span class="status-warn">未激活</span><span class="muted"> · ${Number(u.days_until_purge)} 天后删除</span>`)
        : u.notify_enabled
        ? `<span class="status-ok">开启</span>${u.dnd_enabled ? `<span class="muted"> · 免打扰</span>` : ""}`
        : `<span class="status-fail">关闭</span>`;
      return `<tr>
        <td><input type="checkbox" class="au-check" data-id="${u.id}" ${_adminUsersSelected.has(u.id) ? "checked" : ""} onchange="adminUserToggleSelect(this)" aria-label="选择用户"></td>
        <td>
          <div class="user-name">
            <strong>${escapeHtml(u.username)}</strong>
            ${pills}
          </div>
        </td>
        <td>${adminUserOriginHtml(u)}</td>
        <td>${adminUserLoginHtml(u)}</td>
        <td>${userChannelIconsHtml(u)}</td>
        <td>${Number(u.subscription_count) || 0}</td>
        <td>${push}</td>
        <td>
          <button class="btn-sm" onclick="adminOpenUser(${u.id})">管理</button>
          <button class="btn-sm" onclick="adminOpenUser(${u.id}, 'push')">测试推送</button>
        </td>
      </tr>`;
    }).join("");
    body.innerHTML = `
      <section class="section-panel">
        <header class="section-head au-head">
          <div>
            <h2 class="section-title">用户管理</h2>
            <p class="section-meta">${users.length} 人 · ${adminN} 管理员 · ${boundN} 已绑定渠道 · 未激活 ${counts.inactive} · ${escapeHtml(inactivePolicyRuleLabel())}</p>
          </div>
          <div class="search-bar au-search">
            ${SEARCH_ICON}
            <input id="au-q" type="search" placeholder="搜索用户名 / 来源 / 邀请码，回车" value="${escapeHtml(state.adminUsersQ || "")}" onkeydown="if(event.key==='Enter')adminUsersApplyFilter()">
          </div>
        </header>
        <div class="settings-tabs" role="tablist" aria-label="用户筛选">
          ${tab("all", "全部")}
          ${tab("admin", "管理员")}
          ${tab("unbound", "未绑定")}
          ${tab("push-off", "推送关闭")}
          ${tab("inactive", "未激活")}
        </div>
        <div class="toolbar admin-batch-bar" id="au-batch-bar" style="margin-top:10px;display:${_adminUsersSelected.size ? "flex" : "none"};align-items:center;gap:8px;flex-wrap:wrap">
          <strong>已选 ${_adminUsersSelected.size} 人</strong>
          <button type="button" class="btn-sm" onclick="adminUsersBatch('enable_notify')">开启推送</button>
          <button type="button" class="btn-sm" onclick="adminUsersBatch('disable_notify')">关闭推送</button>
          <button type="button" class="btn-sm danger" onclick="adminUsersBatch('delete')">删除</button>
          <button type="button" class="btn-sm" onclick="adminUserClearSelect()">取消选择</button>
        </div>
        <div class="table-wrap">
          <table>
            <thead><tr>
              <th scope="col" style="width:32px"><input type="checkbox" id="au-checkall" onchange="adminUserTogglePage(this)" aria-label="全选当前筛选"></th>
              <th scope="col">用户</th>
              <th scope="col">来源</th>
              <th scope="col">登录</th>
              <th scope="col">渠道</th>
              <th scope="col">订阅</th>
              <th scope="col">推送</th>
              <th scope="col">操作</th>
            </tr></thead>
            <tbody>${rows || `<tr><td colspan="8" class="muted">${emptyMsg}</td></tr>`}</tbody>
          </table>
        </div>
        <details class="au-policy" ${_auPolicyOpen ? "open" : ""}>
          <summary>${CHEVRON_RIGHT_ICON}<span>未激活清理规则</span><span class="muted">${escapeHtml(inactivePolicyHint(inactivePolicyDraft().inactive_after_days, inactivePolicyDraft().inactive_purge_after_days))}</span></summary>
          <p class="section-meta">领码或网页注册后从未登录，且没有渠道、订阅和推送记录。</p>
          <div class="rc-generate au-inactive-policy">
            <label class="rc-field rc-field-num">
              <span>列为未激活 <span class="cfg-unit">天</span></span>
              <input id="au-inactive-n" class="form-control" type="number" min="0" max="3650" inputmode="numeric" value="${escapeHtml(String(inactivePolicyDraft().inactive_after_days ?? 90))}" oninput="adminInactivePolicySyncSave(true)" onkeydown="adminInactivePolicyKeydown(event)" aria-describedby="au-inactive-hint">
            </label>
            <label class="rc-field rc-field-num">
              <span>之后删除 <span class="cfg-unit">天</span></span>
              <input id="au-inactive-m" class="form-control" type="number" min="0" max="3650" inputmode="numeric" value="${escapeHtml(String(inactivePolicyDraft().inactive_purge_after_days ?? 30))}" oninput="adminInactivePolicySyncSave(true)" onkeydown="adminInactivePolicyKeydown(event)" aria-describedby="au-inactive-hint">
            </label>
            <div class="rc-field-submit">
              <button type="button" class="btn-normal" id="au-inactive-save" onclick="adminSaveInactivePolicy()">保存</button>
            </div>
            <span class="muted rc-generate-hint" id="au-inactive-hint">${escapeHtml(inactivePolicyHint(inactivePolicyDraft().inactive_after_days, inactivePolicyDraft().inactive_purge_after_days))}</span>
          </div>
        </details>
      </section>`;
    const qEl = $("#au-q");
    if (qEl) qEl.value = state.adminUsersQ || "";
    const checkall = $("#au-checkall");
    const boxes = [...document.querySelectorAll(".au-check")];
    if (checkall) {
      checkall.checked = boxes.length > 0 && boxes.every((c) => _adminUsersSelected.has(Number(c.dataset.id)));
      checkall.indeterminate = boxes.some((c) => c.checked) && !checkall.checked;
    }
    const policy = body.querySelector("details.au-policy");
    if (policy) {
      policy.addEventListener("toggle", () => { _auPolicyOpen = policy.open; });
    }
    adminInactivePolicySyncSave();
  }

  function closeAdminModal() {
    document.querySelectorAll(".modal-mask").forEach((el) => el.remove());
  }

  function adminOpenUser(userId, focus) {
    const u = (state.adminUsers || []).find((row) => row.id === userId);
    if (!u) {
      flash("用户不存在或列表已过期", "error");
      return;
    }
    const self = state.user && u.id === state.user.id;
    const origin = escapeHtml(u.origin_label || "网页");
    const loginHint = u.has_password ? "可网页登录" : "无密码，不能网页登录";
    const lastSeen = u.last_login_at ? escapeHtml(fmtDbTime(u.last_login_at)) : "从未登录";
    const kbGroups = (state.imaKbGroups || []).filter((group) => !String(group.id || "").startsWith("feishu-"));
    const kbGranted = new Set(u.ima_kb_groups || []);
    const kbSubscribed = new Set(u.ima_kb_subscribed || []);
    const kbList = kbGroups.length
      ? `<div id="um-kb" class="um-kb-list">${kbGroups.map((group) => {
          const id = String(group.id || "");
          const name = group.name || id;
          const isSub = kbSubscribed.has(id);
          return `<label class="um-kb-item" data-kb-name="${escapeHtml(name)}"${isSub ? ` data-kb-subscribed="1"` : ""}>
            <input type="checkbox" data-kb-group="${escapeHtml(id)}"${kbGranted.has(id) ? " checked" : ""}>
            <span>${escapeHtml(name)}</span>
            ${group.local ? `<span class="muted">本地库${group.enabled === false ? " · 未启用" : ""}</span>` : ""}
            ${isSub ? `<span class="muted">已订阅</span>` : ""}
          </label>`;
        }).join("")}</div>
        <div class="toolbar">
          <button class="btn-sm" onclick="adminSaveUserKnowledge(${u.id})">保存</button>
        </div>`
      : `<p class="muted">还没有配置研报库。</p>`;
    closeAdminModal();
    const mask = document.createElement("div");
    mask.className = "modal-mask";
    mask.innerHTML = `
      <div class="modal-card user-modal" role="dialog" aria-modal="true" aria-labelledby="um-title">
        <h3 id="um-title">管理用户 · ${escapeHtml(u.username)}</h3>
        <p class="muted um-meta">${origin} · ${loginHint} · ${lastSeen}<br>ID ${u.id} · 订阅 ${Number(u.subscription_count) || 0} · 注册 ${escapeHtml(fmtDbTime(u.created_at))}</p>
        <section class="um-block">
          <h4>改名</h4>
          ${u.username_valid === false ? `<p class="muted">当前登录名不合规，保存前请改成 6-30 位中文、字母、数字、下划线或连字符，须以中文或字母开头。</p>` : ""}
          <label class="form-label">用户名
            <div class="row">
              <input id="um-name" class="form-control" maxlength="30" value="${escapeHtml(u.username)}" autocomplete="username">
              <button class="btn-sm" onclick="adminSaveUsername(${u.id})">保存</button>
            </div>
          </label>
        </section>
        <section class="um-block">
          <h4>密码</h4>
          ${u.has_password ? "" : `<p class="muted">这个账号没有密码，不能网页登录。设了密码后才能用账号登录。</p>`}
          <label class="form-label">${u.has_password ? "新密码" : "设置密码"}
            <div class="row">
              <input id="um-pass" class="form-control" type="password" minlength="10" placeholder="至少 10 位" autocomplete="new-password">
              <button class="btn-sm" onclick="adminSavePassword(${u.id})">${u.has_password ? "重置" : "设置"}</button>
            </div>
          </label>
        </section>
        ${self ? "" : `<section class="um-block">
          <h4>权限</h4>
          <div class="toolbar">
            <button class="btn-sm" onclick="adminToggleAdmin(${u.id}, ${!u.is_admin})">${u.is_admin ? "取消管理员" : "设为管理员"}</button>
          </div>
        </section>`}
        ${u.is_admin ? "" : `<section class="um-block">
          <h4>研报库</h4>
          <p class="muted">勾选后即可阅读；若对方开了「匹配研报库」，每日更新后会按他的关键词推一条摘要。取消勾选立即看不到，也不会再推该库。</p>
          ${kbList}
        </section>`}
        <section class="um-block">
          <h4>测试推送</h4>
          <label class="form-label">内容
            <textarea id="um-push-msg" class="form-control" rows="2">这是一条测试推送</textarea>
          </label>
          <div class="toolbar">
            <button class="btn-sm" id="um-push-send" onclick="adminSendTestPush(${u.id})">发送测试</button>
          </div>
          <p id="um-push-result" class="muted um-push-result" hidden></p>
        </section>
        ${self || u.is_admin ? "" : `<section class="um-block user-modal-danger">
          <h4>删除</h4>
          <p class="muted">${escapeHtml(adminDeleteImpact([u]))}。删除后不可恢复。</p>
          <button class="btn-sm danger" onclick="adminDeleteUser(${u.id})">删除用户</button>
        </section>`}
        <div class="toolbar um-close">
          <button class="btn-sm" onclick="closeAdminModal()">关闭</button>
        </div>
      </div>`;
    mask.addEventListener("click", (e) => {
      if (e.target === mask) mask.remove();
    });
    document.body.appendChild(mask);
    trapFocus(mask, () => mask.remove());
    const first = focus === "push" ? $("#um-push-msg") : $("#um-name");
    if (first) first.focus();
  }

  async function adminSaveUserKnowledge(userId) {
    const items = Array.from(document.querySelectorAll("#um-kb .um-kb-item"));
    const groupIds = [];
    const revoked = [];
    for (const item of items) {
      const input = item.querySelector("[data-kb-group]");
      const groupId = String(input?.dataset.kbGroup || "").trim();
      if (!groupId) continue;
      if (input.checked) groupIds.push(groupId);
      else if (item.dataset.kbSubscribed === "1") {
        revoked.push(String(item.dataset.kbName || groupId).trim());
      }
    }
    if (revoked.length && !confirm(`取消勾选后，对方会立刻看不到这些研报库：${revoked.join("、")}。确定保存？`)) return;
    try {
      await api(`/api/admin/users/${userId}/ima-kb`, {
        method: "PUT",
        body: JSON.stringify({ group_ids: groupIds }),
      });
      closeAdminModal();
      flash("已保存");
      loadAdminUsers();
    } catch (err) {
      flash(err.message, "error");
    }
  }

  async function adminSaveUsername(userId) {
    const input = $("#um-name");
    const trimmed = (input ? input.value : "").trim();
    const ruleErr = usernameRuleError(trimmed);
    if (ruleErr) {
      flash(ruleErr, "error");
      return;
    }
    try {
      await api(`/api/users/${userId}`, {
        method: "PUT",
        body: JSON.stringify({ username: trimmed }),
      });
      if (state.user && userId === state.user.id) {
        state.user.username = trimmed;
        renderSidebar(state.user);
        renderTopbar(state.user);
      }
      closeAdminModal();
      flash(`已重命名用户「${trimmed}」`);
      loadAdminUsers();
    } catch (err) {
      flash(err.message, "error");
    }
  }

  async function adminSavePassword(userId) {
    const input = $("#um-pass");
    const pw = input ? input.value : "";
    if (pw.length < 10) {
      flash("密码至少 10 位", "error");
      return;
    }
    try {
      await api(`/api/users/${userId}`, {
        method: "PUT",
        body: JSON.stringify({ password: pw }),
      });
      closeAdminModal();
      flash("密码已重置");
      loadAdminUsers();
    } catch (err) {
      flash(err.message, "error");
    }
  }

  async function adminSendTestPush(userId) {
    const btn = $("#um-push-send");
    const msgEl = $("#um-push-msg");
    const resultEl = $("#um-push-result");
    const msg = ((msgEl && msgEl.value) || "").trim() || "这是一条测试推送";
    if (btn) btn.disabled = true;
    try {
      const data = await api("/api/admin/test-push", {
        method: "POST",
        body: JSON.stringify({ user_id: userId, message: msg }),
      });
      const lines = (data.results || []).map((r) => {
        const label = CHANNEL_LABELS[r.channel] || r.channel;
        return r.ok ? `${label}：成功` : `${label}：失败：${r.error || ""}`;
      });
      if (resultEl) {
        resultEl.hidden = false;
        resultEl.textContent = lines.join("\n") || "没有返回渠道结果";
      }
      const failed = (data.results || []).some((r) => !r.ok);
      flash(failed ? "测试推送部分失败" : "测试推送已发送", failed ? "error" : "success");
    } catch (err) {
      flash(err.message, "error");
      if (resultEl) {
        resultEl.hidden = false;
        resultEl.textContent = err.message;
      }
    } finally {
      if (btn) btn.disabled = false;
    }
  }

  async function adminDeleteUser(userId) {
    const user = (state.adminUsers || []).find((u) => u.id === userId);
    if (!user) {
      flash("用户不存在或列表已过期", "error");
      return;
    }
    if (user.is_admin) {
      flash("不能删除管理员", "error");
      return;
    }
    if (!confirm(`确认删除用户「${user.username}」？\n${adminDeleteImpact([user])}\n删除后不可恢复。`)) return;
    try {
      await api(`/api/users/${userId}`, { method: "DELETE" });
      closeAdminModal();
      flash(`已删除用户「${user ? user.username : userId}」`);
      loadAdminUsers();
    } catch (err) {
      flash(err.message, "error");
    }
  }

  async function adminToggleAdmin(userId, makeAdmin) {
    const user = (state.adminUsers || []).find((u) => u.id === userId);
    const name = user ? user.username : String(userId);
    if (!confirm(makeAdmin ? `确认把「${name}」设为管理员？` : `确认取消「${name}」的管理员权限？`)) return;
    try {
      await api(`/api/users/${userId}`, {
        method: "PUT",
        body: JSON.stringify({ is_admin: makeAdmin }),
      });
      closeAdminModal();
      flash(makeAdmin ? `已将「${name}」设为管理员` : `已取消「${name}」的管理员权限`);
      loadAdminUsers();
    } catch (err) {
      flash(err.message, "error");
    }
  }


  async function loadAdminRequests() {
    let requests, all;
    try {
      [requests, all] = await Promise.all([
        api("/api/admin/kol-requests?status=pending"),
        api("/api/admin/kol-requests"),
      ]);
    } catch (err) {
      if (!routeStillActive(currentAdminSeq())) return;
      $("#admin-body").innerHTML = emptyState("加载失败: " + err.message);
      return;
    }
    const done = all.filter((r) => r.status !== "pending");
    state.pendingKolRequests = requests.length;
    const pendingRows = requests.length === 0
      ? `<tr><td colspan="8" class="muted">暂无待审批申请</td></tr>`
      : requests.map((r) => `
          <tr>
            <td>${r.id}</td><td>${PLATFORM_LABELS[r.platform] || r.platform}</td>
            <td>${escapeHtml(r.name || "（未填）")}</td><td>${escapeHtml(r.external_id)}</td>
            <td>${escapeHtml(r.category_name || "—")}</td>
            <td>${escapeHtml(r.requester || r.user_id)}</td><td>${escapeHtml(fmtDbTime(r.created_at))}</td>
            <td>
              <button class="btn-sm" onclick="adminApproveRequest(${r.id})">通过</button>
              <button class="btn-sm danger" onclick="adminRejectRequest(${r.id})">拒绝</button>
            </td>
          </tr>`).join("");
    const historyRows = done.length === 0
      ? `<tr><td colspan="8" class="muted">暂无处理记录</td></tr>`
      : done.map((r) => `
          <tr>
            <td>${r.id}</td><td>${PLATFORM_LABELS[r.platform] || r.platform}</td>
            <td>${escapeHtml(r.name || "（未填）")}</td><td>${escapeHtml(r.external_id)}</td>
            <td>${escapeHtml(r.category_name || "—")}</td>
            <td>${escapeHtml(r.requester || r.user_id)}</td>
            <td class="${r.status === "approved" ? "status-ok" : "status-fail"}">${r.status === "approved" ? "已通过" : "已拒绝"}</td>
            <td>${escapeHtml(fmtDbTime(r.handled_at))}</td>
          </tr>`).join("");
    if (!routeStillActive(currentAdminSeq())) return;
    $("#admin-body").innerHTML = `
      <section class="section-panel">
        <header class="section-head"><div><h2 class="section-title">添加审批</h2>
        <p class="section-meta">用户申请添加的大V，审批通过后进入订阅广场。</p></div></header>
        <div class="table-wrap">
          <table>
            <thead><tr><th scope="col">ID</th><th scope="col">平台</th><th scope="col">昵称</th><th scope="col">外部ID</th><th scope="col">分类</th><th scope="col">申请人</th><th scope="col">申请时间</th><th scope="col">操作</th></tr></thead>
            <tbody>${pendingRows}</tbody>
          </table>
        </div>
      </section>
      <section class="section-panel">
        <header class="section-head"><div><h2 class="section-title">处理记录</h2></div></header>
        <div class="table-wrap">
          <table>
            <thead><tr><th scope="col">ID</th><th scope="col">平台</th><th scope="col">昵称</th><th scope="col">外部ID</th><th scope="col">分类</th><th scope="col">申请人</th><th scope="col">状态</th><th scope="col">处理时间</th></tr></thead>
            <tbody>${historyRows}</tbody>
          </table>
        </div>
      </section>`;
  }

  async function adminApproveRequest(id) {
    try {
      await api(`/api/admin/kol-requests/${id}/approve`, { method: "POST" });
      flash("已通过申请，大V已进入订阅广场");
      loadAdminRequests();
    } catch (err) {
      alert("操作失败: " + err.message);
    }
  }

  async function adminRejectRequest(id) {
    if (!confirm("确认拒绝该申请？")) return;
    try {
      await api(`/api/admin/kol-requests/${id}/reject`, { method: "POST" });
      flash("已拒绝该申请");
      loadAdminRequests();
    } catch (err) {
      alert("操作失败: " + err.message);
    }
  }

  return {
    loadAdminUsers,
    loadAdminRequests,
    adminApproveRequest,
    adminRejectRequest,
    adminUsersApplyFilter,
    adminUsersBatch,
    adminUserToggleSelect,
    adminUserTogglePage,
    adminUserClearSelect,
    adminOpenUser,
    adminSaveUserKnowledge,
    adminSaveUsername,
    adminSavePassword,
    adminSendTestPush,
    adminDeleteUser,
    adminToggleAdmin,
    adminSaveInactivePolicy,
    adminInactivePolicySyncSave,
    adminInactivePolicyKeydown,
    closeAdminModal,
  };
}
