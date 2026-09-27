export function createAdminInfraView(dependencies) {
  const {
    $,
    state,
    api,
    flash,
    escapeHtml,
    routeStillActive,
    currentRouteSeq,
    currentAdminSeq,
    sessionOwnerStillActive,
    imaMountState,
    fmtTs,
    imaStoragePanelHtml,
    logout,
    PLATFORM_LABELS,
  } = dependencies;

  function renderArmStorage(data) {
    const box = document.getElementById("ima-storage-health");
    const status = document.getElementById("ima-storage-status");
    if (!box) return;
    const pull = data.pull || {};
    let pullText = "未配置 /pull";
    if (pull.configured) {
      pullText = pull.ok ? "ARM /pull 正常" : `ARM /pull 不可达（${pull.status || "失败"}）`;
      if (pull.circuit_open) pullText += " · 熔断打开，短时间内不再打 ARM";
    }
    if (status) status.textContent = pullText;
    const when = data.last_finished_at ? fmtTs(data.last_finished_at) : "无";
    const next = data.next_run_at ? fmtTs(data.next_run_at) : "未排程";
    const err = data.last_error ? `<p class="muted">上次错误：${escapeHtml(data.last_error)}</p>` : "";
    const rows = (data.libraries || []).map((row) => {
      const done = row.finished_at ? fmtTs(row.finished_at) : "—";
      const note = row.error ? escapeHtml(row.error) : "—";
      return `<tr><td>${escapeHtml(row.name || row.id || "")}</td><td>${row.downloaded || 0}</td><td>${row.failed || 0}</td><td>${escapeHtml(done)}</td><td>${note}</td></tr>`;
    }).join("");
    const table = rows
      ? `<table id="ima-arm-groups"><thead><tr><th>知识库</th><th>下载</th><th>失败</th><th>结束</th><th>说明</th></tr></thead><tbody>${rows}</tbody></table>`
      : `<p class="muted">还没有分库记录。</p>`;
    box.innerHTML = `<p class="section-meta">最近采集 ${escapeHtml(when)} · 成功 ${data.groups || 0} 个库 · 下载 ${data.downloaded || 0} · 失败 ${data.failed || 0}</p>${err}${table}<p class="section-meta">下一轮 ${escapeHtml(next)}（每天 01:00 / 09:00 / 17:00）</p>`;
  }

  async function loadStorageHealth() {
    const box = document.getElementById("ima-storage-health");
    const status = document.getElementById("ima-storage-status");
    if (!box) return;
    try {
      renderArmStorage(await api("/api/admin/ima-arm"));
    } catch (err) {
      if (status) status.textContent = "ARM 状态加载失败";
      box.innerHTML = `<p class="muted">${escapeHtml(err.message)}</p>`;
    }
  }

  async function refreshImaStorage() {
    const btn = $("#ima-storage-refresh");
    if (btn?.disabled) return;
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    if (btn) btn.disabled = true;
    try {
      const data = await api("/api/admin/ima-arm");
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      renderArmStorage(data);
      flash("ARM 状态已刷新");
    } catch (err) {
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash(err.message, "error");
    } finally {
      if (btn && document.body.contains(btn)) btn.disabled = false;
    }
  }

  function proxyStatusLabel(status) {
    return { unknown: "未测", ok: "可用", dead: "失效" }[status] || "未知";
  }

  function proxyStatusClass(status) {
    return { ok: "status-ok", dead: "status-fail" }[status] || "";
  }

  function proxyOptionLabel(row) {
    const auth = row.username ? `${escapeHtml(row.username)}@` : "";
    return `#${row.id} ${row.protocol} ${auth}${escapeHtml(row.host)}:${row.port}`;
  }

  function proxyBusy(btn, on) {
    if (!btn) return false;
    if (on && btn.disabled) return true;
    btn.disabled = on;
    return false;
  }

  async function loadProxyAdmin() {
    const box = $("#st-proxies");
    if (!box) return;
    const drafts = {};
    document.querySelectorAll("textarea[id^='pp-import-']").forEach((el) => {
      if (el.value) drafts[el.id] = el.value;
    });
    try {
      const [routes, pools, proxies] = await Promise.all([
        api("/api/admin/proxy-routes"),
        api("/api/admin/proxy-pools"),
        api("/api/admin/proxies"),
      ]);
      box.innerHTML = renderProxyAdmin(routes, pools.items || [], proxies.items || []);
      Object.entries(drafts).forEach(([id, text]) => {
        const el = document.getElementById(id);
        if (el) el.value = text;
      });
      ["xueqiu", "combination", "weibo", "twitter"].forEach((p) => {
        const r = routes[p] || {};
        if (r.pool_id && $(`#pr-${p}-pool`)) $(`#pr-${p}-pool`).value = String(r.pool_id);
        if (r.proxy_id && $(`#pr-${p}-proxy`)) $(`#pr-${p}-proxy`).value = String(r.proxy_id);
        syncProxyRouteInputs(p);
      });
    } catch (err) {
      box.innerHTML = `<p class="muted">${escapeHtml(err.message || "加载代理失败")}</p>
        <div class="toolbar"><button type="button" class="btn-ghost" onclick="loadProxyAdmin()">重试</button></div>`;
    }
  }

  function renderProxyAdmin(routes, pools, proxies) {
    const platforms = ["xueqiu", "combination", "weibo", "twitter"];
    const poolOpts = pools.length
      ? pools.map((p) => `<option value="${p.id}">${escapeHtml(p.name)}（${p.proxy_count}）</option>`).join("")
      : `<option value="">先创建代理池</option>`;
    const proxyOpts = proxies.length
      ? proxies.map((p) => `<option value="${p.id}">${proxyOptionLabel(p)}</option>`).join("")
      : `<option value="">先导入或提取代理</option>`;
    const routeRows = platforms.map((p) => {
      const r = routes[p] || { mode: "direct" };
      const label = PLATFORM_LABELS[p];
      return `<div class="proxy-route">
        <label class="cfg-field">
          <span>${label}</span>
          <select id="pr-${p}-mode" class="form-control" onchange="syncProxyRouteInputs('${p}')">
            <option value="direct"${r.mode === "direct" ? " selected" : ""}>直连</option>
            <option value="pool"${r.mode === "pool" ? " selected" : ""}>指定池</option>
            <option value="proxy"${r.mode === "proxy" ? " selected" : ""}>指定代理</option>
          </select>
        </label>
        <label class="cfg-field" id="pr-${p}-pool-wrap"${r.mode === "pool" ? "" : " hidden"}>
          <span>代理池</span>
          <select id="pr-${p}-pool" class="form-control" aria-label="${label} 代理池">${poolOpts}</select>
        </label>
        <label class="cfg-field" id="pr-${p}-proxy-wrap"${r.mode === "proxy" ? "" : " hidden"}>
          <span>指定代理</span>
          <select id="pr-${p}-proxy" class="form-control" aria-label="${label} 指定代理">${proxyOpts}</select>
        </label>
      </div>`;
    }).join("");
    const poolCards = pools.map((p) => {
      const rows = proxies.filter((x) => x.pool_id === p.id);
      const lines = rows.map((x) => {
        const statusClass = proxyStatusClass(x.status);
        return `<tr>
        <td class="ak-hide-mobile" data-label="协议">${escapeHtml(x.protocol)}</td>
        <td data-label="地址">${escapeHtml(x.host)}:${x.port}</td>
        <td class="ak-hide-mobile" data-label="账号">${escapeHtml(x.username || "—")}</td>
        <td data-label="状态"${statusClass ? ` class="${statusClass}"` : ""}>${escapeHtml(proxyStatusLabel(x.status))}</td>
        <td class="ak-hide-mobile" data-label="来源">${x.source === "extract" ? "提取" : "手动"}</td>
        <td data-label="过期">${x.expires_at ? fmtTs(x.expires_at) : "—"}</td>
        <td class="ak-actions" data-label="操作">
          <button type="button" class="btn-sm" data-proxy-test="${x.id}" onclick="testProxyNode(${x.id})">测试</button>
          <button type="button" class="btn-sm danger" onclick="deleteProxyNode(${x.id})">删除</button>
        </td>
      </tr>`;
      }).join("");
      const extract = p.kind === "extract"
        ? `<p class="section-meta proxy-extract-url">提取 ${escapeHtml(p.extract_url || "未填")}${p.last_error ? ` · 上次错误 ${escapeHtml(p.last_error)}` : ""}</p>
           <div class="toolbar"><button type="button" class="btn-ghost" data-proxy-extract="${p.id}" onclick="extractProxyPool(${p.id})">立即提取</button></div>`
        : "";
      return `<section class="section-panel">
        <header class="section-head rc-list-head"><div>
          <h2 class="section-title">${escapeHtml(p.name)} <span class="hint">${p.kind === "extract" ? "提取池" : "静态池"} · ${escapeHtml(p.protocol)}</span></h2>
          ${extract}
        </div>
        <button type="button" class="btn-ghost danger" onclick="deleteProxyPool(${p.id})">删除池</button></header>
        <label class="form-label" for="pp-import-${p.id}"><span>导入节点</span>
          <textarea id="pp-import-${p.id}" class="form-control cookie-paste proxy-import" rows="3" placeholder="host:port 或 socks5://user:pass@host:port，一行一条"></textarea>
        </label>
        <div class="toolbar">
          <button type="button" class="btn-normal" data-proxy-import="${p.id}" onclick="importProxyPool(${p.id})">导入</button>
        </div>
        <div class="table-wrap proxy-nodes-wrap">
          <table class="ak-table proxy-nodes">
            <thead><tr><th>协议</th><th>地址</th><th>账号</th><th>状态</th><th>来源</th><th>过期</th><th>操作</th></tr></thead>
            <tbody>${lines || `<tr class="ak-empty"><td colspan="7" class="muted">还没有节点，先导入或提取。</td></tr>`}</tbody>
          </table>
        </div>
      </section>`;
    }).join("");
    return `
      <section class="section-panel">
        <header class="section-head"><div>
          <h2 class="section-title">抓取出口</h2>
          <p class="section-meta">按平台选择直连、指定池或指定代理。组合与雪球常同出口，但不强制绑定。池空时本轮抓取失败，不会偷偷直连。</p>
        </div></header>
        <div class="cfg-fields">${routeRows}</div>
        <div class="cfg-save-row"><button type="button" class="btn-normal" id="pr-save" onclick="saveProxyRoutes()">保存出口</button></div>
      </section>
      <section class="section-panel">
        <header class="section-head"><div>
          <h2 class="section-title">新建代理池</h2>
          <p class="section-meta">静态池粘贴导入；提取池填商家提取 URL（一行一个 IP），按过期秒数刷新。</p>
        </div></header>
        <div class="cfg-fields">
          <label class="cfg-field"><span>名称</span><input id="pp-name" class="form-control" maxlength="40" placeholder="海外S5"></label>
          <label class="cfg-field"><span>类型</span>
            <select id="pp-kind" class="form-control" onchange="syncProxyPoolForm()">
              <option value="static">静态</option>
              <option value="extract">提取 URL</option>
            </select>
          </label>
          <label class="cfg-field"><span>协议</span>
            <select id="pp-protocol" class="form-control">
              <option value="http">HTTP</option>
              <option value="socks5">SOCKS5</option>
            </select>
          </label>
          <label class="cfg-field" id="pp-extract-wrap" hidden><span>提取 URL</span><input id="pp-extract-url" class="form-control" placeholder="https://api.example.com/get?key="></label>
          <label class="cfg-field" id="pp-expire-wrap" hidden><span>过期<span class="cfg-unit">秒</span></span><input id="pp-expire" type="number" class="form-control" min="0" value="300"></label>
          <label class="cfg-field" id="pp-refresh-wrap" hidden><span>刷新<span class="cfg-unit">秒</span></span><input id="pp-refresh" type="number" class="form-control" min="0" value="180"></label>
        </div>
        <div class="cfg-save-row"><button type="button" class="btn-normal" id="pp-create" onclick="createProxyPool()">创建</button></div>
      </section>
      ${poolCards || `<p class="muted">还没有代理池。先创建一个，再导入或提取。</p>`}`;
  }

  function syncProxyRouteInputs(platform) {
    const mode = $(`#pr-${platform}-mode`)?.value;
    const poolWrap = $(`#pr-${platform}-pool-wrap`);
    const proxyWrap = $(`#pr-${platform}-proxy-wrap`);
    if (poolWrap) poolWrap.hidden = mode !== "pool";
    if (proxyWrap) proxyWrap.hidden = mode !== "proxy";
  }

  function syncProxyPoolForm() {
    const extract = $("#pp-kind")?.value === "extract";
    ["pp-extract-wrap", "pp-expire-wrap", "pp-refresh-wrap"].forEach((id) => {
      const el = document.getElementById(id);
      if (el) el.hidden = !extract;
    });
  }

  async function saveProxyRoutes() {
    const body = {};
    for (const p of ["xueqiu", "combination", "weibo", "twitter"]) {
      const mode = $(`#pr-${p}-mode`).value;
      body[p] = { mode };
      if (mode === "pool") {
        const poolId = $(`#pr-${p}-pool`).value;
        if (!poolId) {
          flash("请先创建代理池", "error");
          return;
        }
        body[p].pool_id = Number(poolId);
      }
      if (mode === "proxy") {
        const proxyId = $(`#pr-${p}-proxy`).value;
        if (!proxyId) {
          flash("请先导入或提取代理", "error");
          return;
        }
        body[p].proxy_id = Number(proxyId);
      }
    }
    const btn = $("#pr-save");
    if (proxyBusy(btn, true)) return;
    try {
      await api("/api/admin/proxy-routes", { method: "PUT", body: JSON.stringify(body) });
      flash("抓取出口已保存");
      loadProxyAdmin();
    } catch (err) {
      flash(err.message || "保存失败", "error");
    } finally {
      if (btn && document.body.contains(btn)) btn.disabled = false;
    }
  }

  async function createProxyPool() {
    const name = $("#pp-name").value.trim();
    if (!name) {
      flash("请填写代理池名称", "error");
      return;
    }
    const kind = $("#pp-kind").value;
    if (kind === "extract" && !$("#pp-extract-url").value.trim()) {
      flash("提取池需要填写提取 URL", "error");
      return;
    }
    const btn = $("#pp-create");
    if (proxyBusy(btn, true)) return;
    try {
      await api("/api/admin/proxy-pools", {
        method: "POST",
        body: JSON.stringify({
          name,
          kind,
          protocol: $("#pp-protocol").value,
          extract_url: $("#pp-extract-url").value,
          expire_seconds: Number($("#pp-expire").value || 0),
          refresh_interval_seconds: Number($("#pp-refresh").value || 0),
        }),
      });
      flash("代理池已创建");
      loadProxyAdmin();
    } catch (err) {
      flash(err.message || "创建失败", "error");
    } finally {
      if (btn && document.body.contains(btn)) btn.disabled = false;
    }
  }

  async function importProxyPool(poolId) {
    const text = $(`#pp-import-${poolId}`)?.value || "";
    if (!text.trim()) {
      flash("请先粘贴要导入的代理", "error");
      return;
    }
    const btn = document.querySelector(`[data-proxy-import="${poolId}"]`);
    if (proxyBusy(btn, true)) return;
    try {
      const result = await api(`/api/admin/proxy-pools/${poolId}/import`, {
        method: "POST",
        body: JSON.stringify({ text }),
      });
      const ta = $(`#pp-import-${poolId}`);
      if (ta) ta.value = "";
      flash(`导入 ${result.imported} 条`);
      loadProxyAdmin();
    } catch (err) {
      flash(err.message || "导入失败", "error");
    } finally {
      if (btn && document.body.contains(btn)) btn.disabled = false;
    }
  }

  async function extractProxyPool(poolId) {
    const btn = document.querySelector(`[data-proxy-extract="${poolId}"]`);
    if (proxyBusy(btn, true)) return;
    try {
      const result = await api(`/api/admin/proxy-pools/${poolId}/extract`, { method: "POST" });
      flash(`提取 ${result.imported} 条`);
      loadProxyAdmin();
    } catch (err) {
      flash(err.message || "提取失败", "error");
    } finally {
      if (btn && document.body.contains(btn)) btn.disabled = false;
    }
  }

  async function deleteProxyPool(poolId) {
    if (!confirm("删除这个代理池及其节点？")) return;
    try {
      await api(`/api/admin/proxy-pools/${poolId}`, { method: "DELETE" });
      flash("已删除");
      loadProxyAdmin();
    } catch (err) {
      flash(err.message || "删除失败", "error");
    }
  }

  async function deleteProxyNode(proxyId) {
    if (!confirm("删除后需要重新导入。确定删除这个节点？")) return;
    try {
      await api(`/api/admin/proxies/${proxyId}`, { method: "DELETE" });
      flash("已删除");
      loadProxyAdmin();
    } catch (err) {
      flash(err.message || "删除失败", "error");
    }
  }

  async function testProxyNode(proxyId) {
    const btn = document.querySelector(`[data-proxy-test="${proxyId}"]`);
    if (proxyBusy(btn, true)) return;
    try {
      const result = await api(`/api/admin/proxies/${proxyId}/test`, { method: "POST" });
      flash(result.ok ? "测试成功" : (result.error || `测试失败 ${result.status_code || ""}`), result.ok ? "success" : "error");
      await loadProxyAdmin();
    } catch (err) {
      flash(err.message || "测试失败", "error");
      if (btn && document.body.contains(btn)) btn.disabled = false;
    }
  }
  function backupStatusHtml(s) {
    const parts = [];
    if (s.last_ok_at) {
      parts.push(`上次<span class="status-ok">成功</span> ${escapeHtml(s.last_ok_at)}`);
    }
    if (s.last_error) {
      parts.push(`上次<span class="status-fail">失败</span> ${escapeHtml(s.last_error)}`);
    }
    if (s.last_remote_name) parts.push(`远端 ${escapeHtml(s.last_remote_name)}`);
    if (s.next_run_at) parts.push(`下次 ${escapeHtml(s.next_run_at)}`);
    if (!parts.length) {
      return `<p class="section-meta backup-status" id="backup-status">尚未执行过定时备份</p>`;
    }
    return `<p class="section-meta backup-status" id="backup-status">${parts.join(" · ")}</p>`;
  }

  function backupWebDAVBody() {
    const body = {
      url: $("#bk-url").value.trim(),
      username: $("#bk-user").value.trim(),
      path: $("#bk-path").value.trim() || "/vpush-backups",
      hour: Number($("#bk-hour").value),
      keep: Number($("#bk-keep").value),
    };
    const password = $("#bk-pass").value;
    if (password) body.password = password;
    return body;
  }

  async function loadAdminBackup() {
    const s = await api("/api/admin/backup");
    if (!routeStillActive(currentAdminSeq())) return;
    $("#admin-body").innerHTML = `
      <section class="section-panel backup-page">
        <header class="section-head">
          <div>
            <h2 class="section-title">本机备份</h2>
            <p class="section-meta">下载当前数据库，不经过 WebDAV。</p>
          </div>
        </header>
        <div class="toolbar backup-actions">
          <button class="btn-ghost" onclick="backupDownload()">下载当前数据库</button>
        </div>
      </section>
      <section class="section-panel backup-page">
        <header class="section-head">
          <div>
            <h2 class="section-title">WebDAV 定时</h2>
            <p class="section-meta">填好后由调度每天自动上传；密码只写不回显。</p>
          </div>
        </header>
        <label class="form-label">地址
          <input id="bk-url" class="form-control" type="url" autocomplete="off" placeholder="https://example.com/webdav" value="${escapeHtml(s.url || "")}">
        </label>
        <label class="form-label">用户名
          <input id="bk-user" class="form-control" autocomplete="off" value="${escapeHtml(s.username || "")}">
        </label>
        <label class="form-label">密码
          <input id="bk-pass" class="form-control" type="password" autocomplete="new-password" placeholder="${s.password_set ? "已设置" : "WebDAV 密码"}">
        </label>
        <div class="backup-grid">
          <label class="form-label backup-path">远端目录
            <input id="bk-path" class="form-control" autocomplete="off" placeholder="/vpush-backups" value="${escapeHtml(s.path || "/vpush-backups")}">
          </label>
          <label class="form-label backup-num">每天几点
            <input id="bk-hour" class="form-control" type="number" min="0" max="23" value="${s.hour ?? 3}">
          </label>
          <label class="form-label backup-num">保留份数
            <input id="bk-keep" class="form-control" type="number" min="1" max="90" value="${s.keep ?? 14}">
          </label>
        </div>
        ${backupStatusHtml(s)}
        <div class="cfg-save-row backup-actions">
          <button class="btn-normal" onclick="saveBackupWebDAV()">保存</button>
          <button class="btn-ghost" onclick="testBackupWebDAV()">测试连接</button>
        </div>
      </section>
      <section class="section-panel backup-page">
        <header class="section-head">
          <div>
            <h2 class="section-title">恢复</h2>
            <p class="section-meta">会覆盖当前账号、订阅和帖子。恢复失败时现库不变。</p>
          </div>
        </header>
        <div class="backup-stack">
          <div class="toolbar backup-actions">
            <button id="bk-restore-webdav" class="btn-ghost danger" onclick="backupRestoreWebDAV()">从 WebDAV 恢复最新一份</button>
          </div>
          <label class="form-label">本地 .db 文件
            <input id="bk-file" class="backup-file-input" type="file" accept=".db">
          </label>
          <div class="toolbar backup-actions">
            <button id="bk-restore-upload" class="btn-ghost danger" onclick="backupRestoreUpload()">用本地备份恢复</button>
          </div>
        </div>
      </section>`;
  }

  async function saveBackupWebDAV() {
    try {
      await api("/api/admin/backup/webdav", {
        method: "PUT",
        body: JSON.stringify(backupWebDAVBody()),
      });
      flash("WebDAV 配置已保存");
      $("#bk-pass").value = "";
      await loadAdminBackup();
    } catch (err) {
      flash(err.message, "error");
    }
  }

  async function testBackupWebDAV() {
    try {
      await api("/api/admin/backup/webdav/test", {
        method: "POST",
        body: JSON.stringify(backupWebDAVBody()),
      });
      flash("WebDAV 连接正常");
    } catch (err) {
      flash(err.message, "error");
    }
  }

  async function backupDownload() {
    const resp = await fetch("/api/admin/backup/download", {
      headers: { Authorization: `Bearer ${state.token}` },
    });
    if (resp.status === 401) {
      logout();
      return;
    }
    if (!resp.ok) {
      const data = await resp.json().catch(() => ({}));
      flash(typeof data.detail === "string" ? data.detail : "下载失败", "error");
      return;
    }
    const blob = await resp.blob();
    const match = /filename="?([^";]+)"?/.exec(resp.headers.get("content-disposition") || "");
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = match ? match[1] : "dav-backup.db";
    a.click();
    URL.revokeObjectURL(a.href);
  }

  async function backupRestoreWebDAV() {
    if (!confirm("确认用备份覆盖当前数据库？当前账号、订阅和帖子都会被替换。")) return;
    const btn = $("#bk-restore-webdav");
    if (btn) btn.disabled = true;
    try {
      await api("/api/admin/backup/restore/webdav", { method: "POST" });
      flash("已从 WebDAV 恢复");
      await loadAdminBackup();
    } catch (err) {
      flash(err.message, "error");
    } finally {
      if (btn) btn.disabled = false;
    }
  }

  async function backupRestoreUpload() {
    if (!confirm("确认用备份覆盖当前数据库？当前账号、订阅和帖子都会被替换。")) return;
    const input = $("#bk-file");
    if (!input?.files?.[0]) {
      flash("请选择 .db 备份文件", "error");
      return;
    }
    const btn = $("#bk-restore-upload");
    if (btn) btn.disabled = true;
    try {
      const fd = new FormData();
      fd.append("file", input.files[0]);
      const resp = await fetch("/api/admin/backup/restore/upload", {
        method: "POST",
        headers: { Authorization: `Bearer ${state.token}` },
        body: fd,
      });
      if (resp.status === 401) {
        logout();
        return;
      }
      const data = await resp.json().catch(() => ({}));
      if (!resp.ok) {
        flash(typeof data.detail === "string" ? data.detail : "恢复失败", "error");
        return;
      }
      flash("已从本地备份恢复");
      await loadAdminBackup();
    } catch (err) {
      flash(err.message, "error");
    } finally {
      if (btn) btn.disabled = false;
    }
  }

  return {
    loadStorageHealth,
    refreshImaStorage,
    loadProxyAdmin,
    syncProxyRouteInputs,
    syncProxyPoolForm,
    saveProxyRoutes,
    createProxyPool,
    importProxyPool,
    extractProxyPool,
    deleteProxyPool,
    deleteProxyNode,
    testProxyNode,
    loadAdminBackup,
    saveBackupWebDAV,
    testBackupWebDAV,
    backupDownload,
    backupRestoreWebDAV,
    backupRestoreUpload,
  };
}
