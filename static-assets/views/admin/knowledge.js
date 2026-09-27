export function createAdminKnowledgeView(dependencies) {
  const {
    $,
    state,
    api,
    flash,
    escapeHtml,
    emptyState,
    routeStillActive,
    currentRouteSeq,
    currentAdminSeq,
    routeQuery,
    REFRESH_ICON,
    CHEVRON_RIGHT_ICON,
    setPageTitle,
    imaMountState,
    imaCollectorPureCache,
    imaStoragePanelHtml,
    fmtCacheBytes,
    imaCollectorHasUnsaved,
    imaCollectorFormSnapshot,
    rememberImaCollectorDraft,
    initImaMountState,
    renderImaMountGroups,
    renderImaGroupAcl,
    imaGroupIntervalSeconds,
    imaMountGroup,
    restoreImaCollectorOwnerToken,
    clearImaCollectorDraft,
    startImaProgressPoll,
    stopImaProgressPoll,
    applyImaCollectorProgress,
    imaCollectorFormRevision,
    imaCollectorStatusText,
    imaGroupDiscoveryStatusText,
    imaCollectorProgressHtml,
    cookieUpdatedLabel,
    stopStatsTimer,
    renderStatsData,
    startDashboardLiveTimer,
    loadLocalLibraries,
    loadCiccStatus,
    startCiccPoll,
    stopCiccPoll,
    loadStorageHealth,
    loadFeishuDocumentSources,
    nextStatsLoadSeq,
    currentStatsLoadSeq,
    getStatsSnapshot,
    setStatsSnapshot,
  } = dependencies;

  const KS_TAB_KEY = "ks-tab";

  // ponytail: tablist 方向键导航（roving tabindex），左右/上下/Home/End
  function onKnowledgeTabsKey(event) {
    const current = event.target?.closest?.(".ks-tab");
    if (!current) return;
    const tabs = [...document.querySelectorAll(".ks-tab")];
    const i = tabs.indexOf(current);
    if (i < 0) return;
    let next = -1;
    if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (i + 1) % tabs.length;
    else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (i - 1 + tabs.length) % tabs.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = tabs.length - 1;
    else return;
    event.preventDefault();
    tabs[next].focus();
    switchKnowledgeSettingsTab(tabs[next].dataset.tab);
  }

  // ponytail: 刷新/关闭前只拦采集草稿（其他均为即时生效）；页签切换由 switchKnowledgeSettingsTab 守卫
  window.addEventListener("beforeunload", (event) => {
    if (!imaCollectorHasUnsaved || !imaCollectorHasUnsaved()) return;
    event.preventDefault();
  });

  function switchKnowledgeSettingsTab(tab) {
    if (tab === "cicc") tab = "local"; // 旧页签记忆迁移：中金已并入本地库
    const allowed = ["collect", "zsxq", "storage", "local", "feishu"];
    const next = allowed.includes(tab) ? tab : "collect";
    // ponytail: 页签级守卫只拦采集未保存（ACL/cookie/开关均为即时生效，无需拦）
    const current = document.querySelector(".ks-tab.is-on")?.dataset.tab || "collect";
    if (next !== current && current === "collect" && imaCollectorHasUnsaved()
      && !confirm("采集配置有未保存的修改，切换页签将保留草稿。继续吗？")) return;
    if (next === "local") {
      loadLocalLibraries();
      loadCiccStatus();
      startCiccPoll();
    } else {
      stopCiccPoll();
    }
    if (next === "storage") loadStorageHealth();
    if (next === "feishu") loadFeishuDocumentSources();
    try { sessionStorage.setItem(KS_TAB_KEY, next); } catch { /* ignore */ }
    // ponytail: roving tabindex + aria-selected，方向键按 tablist 惯例切换
    document.querySelectorAll(".ks-tab").forEach((btn) => {
      const on = btn.dataset.tab === next;
      btn.classList.toggle("is-on", on);
      btn.setAttribute("aria-selected", String(on));
      btn.tabIndex = on ? 0 : -1;
    });
    document.querySelectorAll(".ks-panel").forEach((panel) => {
      panel.classList.toggle("is-on", panel.dataset.panel === next);
    });
  }

  async function loadAdminKnowledge(seq = currentAdminSeq(), authoritativeImaStatus = null) {
    if (!routeStillActive(seq)) return false;
    const generation = imaMountState.generation;
    const pendingOwner = imaMountState.saveOwner;
    if (pendingOwner && !pendingOwner.putCompleted && $("#ima-mount-layout")) {
      pendingOwner.liveSnapshot = imaCollectorFormSnapshot();
      rememberImaCollectorDraft(pendingOwner.liveSnapshot);
    }
    const statsLoadSeq = nextStatsLoadSeq();
    let s;
    let statsLoadError = null;
    try {
      const stats = await api("/api/stats");
      if (!routeStillActive(seq) || statsLoadSeq !== currentStatsLoadSeq()
        || generation !== imaMountState.generation) return false;
      s = authoritativeImaStatus ? { ...stats, ima_collector: authoritativeImaStatus } : stats;
      setStatsSnapshot(s);
    } catch (err) {
      if (!routeStillActive(seq) || statsLoadSeq !== currentStatsLoadSeq()
        || generation !== imaMountState.generation) return false;
      const message = `加载失败: ${err.message || "请求失败"}`;
      const fallbackStats = getStatsSnapshot();
      if (fallbackStats && authoritativeImaStatus) {
        s = { ...fallbackStats, ima_collector: authoritativeImaStatus };
        statsLoadError = message;
      } else {
        const retry = `<div><button type="button" class="btn-normal" onclick="loadAdminKnowledge(${seq})">重试</button></div>`;
        const body = $("#admin-body");
        if (body) body.innerHTML = emptyState(message, retry);
        return false;
      }
    }
    if (!routeStillActive(seq) || statsLoadSeq !== currentStatsLoadSeq()
      || generation !== imaMountState.generation) return false;
    stopStatsTimer();
    const owner = imaMountState.saveOwner;
    const ownerIsCurrent = owner && owner === pendingOwner;
    const ownerLiveSnapshot = ownerIsCurrent ? owner.liveSnapshot : null;
    const ownerHasNewerEdits = !!ownerLiveSnapshot
      && imaCollectorFormRevision(ownerLiveSnapshot) !== owner.formRevision;
    const ownerSnapshot = ownerIsCurrent
      ? (owner.putCompleted
        ? (ownerHasNewerEdits ? ownerLiveSnapshot : null)
        : (ownerLiveSnapshot || owner.snapshot))
      : null;
    const pendingCollectorDraft = imaMountState.collectorDraft;
    const confirmedCollectorDraft = pendingCollectorDraft
      && imaMountState.collectorDraftRevision === imaMountState.collectorConfirmedRevision
      && imaMountState.collectorRevision === imaMountState.collectorConfirmedLiveRevision
      && imaMountState.revision === imaMountState.collectorConfirmedMountRevision;
    const preserveMountDraft = imaMountState.dirty
      && !confirmedCollectorDraft
      && !(ownerIsCurrent && owner.putCompleted && !ownerHasNewerEdits);
    const mountRevisionChangedDuringSave = ownerIsCurrent
      && imaMountState.revision !== owner.mountRevision;
    const preserveMountDraftForReload = preserveMountDraft || mountRevisionChangedDuringSave;
    const imaCollector = s.ima_collector || {};
    const pure = imaCollector.config || {};
    const collectorDraft = confirmedCollectorDraft ? null
      : (ownerSnapshot || (ownerIsCurrent && owner.putCompleted ? null : pendingCollectorDraft));
    const collector = collectorDraft || pure;
    const collectorGroups = collectorDraft?.groups || pure.groups || [];
    Object.assign(imaCollectorPureCache, {
      uid: collector.uid || "",
      interval_seconds: Number(collector.interval_seconds || 3600),
      knowledge_base_id: collector.knowledge_base_id || "",
      root_folder_id: collector.root_folder_id || "",
    });
    const zq = s.zsxq_cookie || {};
    const zc = s.zsxq_cache || { files: 0, bytes: 0 };
    const zcSize = fmtCacheBytes(zc.bytes);
    setPageTitle("研报设置");
    $("#admin-body").innerHTML = `
      <div id="stats-poll-error"></div>
      <div class="knowledge-settings">
        <div class="ks-tabs" role="tablist" aria-label="研报设置页签" onkeydown="onKnowledgeTabsKey(event)">
          <button type="button" role="tab" class="ks-tab is-on" data-tab="collect" aria-selected="true" aria-controls="ks-panel-collect" id="ks-tab-collect" onclick="switchKnowledgeSettingsTab(this.dataset.tab)">采集</button>
          <button type="button" role="tab" class="ks-tab" data-tab="zsxq" aria-selected="false" aria-controls="ks-panel-zsxq" id="ks-tab-zsxq" tabindex="-1" onclick="switchKnowledgeSettingsTab(this.dataset.tab)">星球</button>
          <button type="button" role="tab" class="ks-tab" data-tab="storage" aria-selected="false" aria-controls="ks-panel-storage" id="ks-tab-storage" tabindex="-1" onclick="switchKnowledgeSettingsTab(this.dataset.tab)">存储</button>
          <button type="button" role="tab" class="ks-tab" data-tab="local" aria-selected="false" aria-controls="ks-panel-local" id="ks-tab-local" tabindex="-1" onclick="switchKnowledgeSettingsTab(this.dataset.tab)">本地库</button>
          <button type="button" role="tab" class="ks-tab" data-tab="feishu" aria-selected="false" aria-controls="ks-panel-feishu" id="ks-tab-feishu" tabindex="-1" onclick="switchKnowledgeSettingsTab(this.dataset.tab)">飞书文档</button>
        </div>
        <section class="section-panel ks-panel is-on" data-panel="collect" role="tabpanel" id="ks-panel-collect" aria-labelledby="ks-tab-collect">
          <header class="section-head"><div><h2 class="section-title">IMA 文档采集</h2>
          <p class="section-meta">勾选文件夹后同步其中新增 PDF。父目录包含以后新建的子目录。</p></div></header>
          <div class="cfg-stack ima-collector-stack">
            <div class="cfg-group ima-groups-block">
              <div class="ima-groups-head">
                <div>
                  <p class="cfg-group-title">共享知识库与文件夹</p>
                  <span id="ima-group-discovery-status" class="muted" aria-live="polite">${imaGroupDiscoveryStatusText(imaCollector)}</span>
                </div>
                <div class="toolbar ima-groups-toolbar">
                  <button type="button" class="btn-ghost" id="ima-discover-btn" onclick="discoverImaGroups()" aria-label="重新发现共享知识库">${REFRESH_ICON}<span>重新发现</span></button>
                </div>
              </div>
              <div class="ima-mount-layout" id="ima-mount-layout">
                <aside class="ima-mount-rail" aria-labelledby="ima-kb-pane-title">
                  <header class="ima-mount-pane-head"><strong id="ima-kb-pane-title">知识库</strong><span id="ima-kb-count" class="muted"></span></header>
                  <select id="ima-kb-select" class="form-control ima-kb-select" aria-label="选择知识库" onchange="selectImaMountGroup(this.value)"></select>
                  <div id="ima-kb-list" class="ima-kb-list" role="listbox" aria-label="共享知识库"></div>
                </aside>
                <section class="ima-mount-detail" aria-labelledby="ima-selected-group-name">
                  <header class="ima-selected-head">
                    <div><strong id="ima-selected-group-name">选择知识库</strong><span id="ima-selected-group-state" class="muted">${imaCollectorStatusText(imaCollector)}</span></div>
                    <button type="button" class="btn-ghost" id="ima-sync-btn" onclick="triggerImaCollector()" aria-label="同步当前库">${REFRESH_ICON}<span>同步当前库</span></button>
                  </header>
                  <section class="ima-detail-section ima-frequency-section">
                    <div><h3>同步频率</h3><p class="section-meta">修改后需要保存。</p></div>
                    <div id="ima-selected-interval" class="ima-selected-interval"></div>
                  </section>
                  <section class="ima-detail-section" id="ima-group-acl-block">
                    <header><h3>查看权限</h3><p class="section-meta">添加或移除即时生效；管理员始终可看。</p></header>
                    <div id="ima-group-acl"><p class="muted">加载中…</p></div>
                  </section>
                  <section class="ima-detail-section ima-folder-section">
                    <button type="button" class="ima-folder-panel-toggle" id="ima-folder-panel-toggle"
                      aria-expanded="false" aria-controls="ima-folder-panel" onclick="toggleImaFolderPanel(this)">
                      <span><strong>采集文件夹</strong><span id="ima-folder-summary" class="muted">未选择文件夹</span></span>
                      ${CHEVRON_RIGHT_ICON}
                    </button>
                    <div id="ima-folder-panel" hidden>
                      <header class="ima-mount-pane-head"><strong id="ima-folder-title">选择知识库</strong><span id="ima-folder-count" class="muted"></span></header>
                      <div id="ima-folder-tree" class="ima-folder-tree" role="tree" aria-label="知识库文件夹" aria-live="polite"></div>
                    </div>
                  </section>
                </section>
              </div>
            </div>
          </div>
          <div class="ima-collector-runtime">
            <div id="ima-sync-progress">${imaCollectorProgressHtml(imaCollector)}</div>
            <span id="ima-collector-status" class="muted">${imaCollectorStatusText(imaCollector)}</span>
          </div>
          <div class="ima-collector-savebar" id="ima-collector-savebar" hidden>
            <span class="ima-unsaved-status"><span aria-hidden="true"></span>有未保存的采集配置修改</span>
            <div class="toolbar">
              <button type="button" class="btn-ghost" id="ima-collector-discard" onclick="discardImaCollectorChanges()">放弃修改</button>
              <button type="button" class="btn-normal" id="ima-collector-save"${imaMountState.saveOwner ? " disabled" : ""} onclick="saveImaCollector()">保存采集配置</button>
            </div>
          </div>
        </section>
        <section class="section-panel ks-panel" data-panel="feishu" role="tabpanel" id="ks-panel-feishu" aria-labelledby="ks-tab-feishu">
          <header class="section-head"><div><h2 class="section-title">飞书文档</h2>
          <p class="section-meta">订阅私有 Wiki 或 Docx，按文档 revision 自动更新为时间线。移除只撤下阅读入口，历史归档永久保留。</p></div></header>
          <form class="feishu-source-add" onsubmit="event.preventDefault();addFeishuDocumentSource()">
            <label for="feishu-document-url" class="sr-only">飞书文档链接</label>
            <input id="feishu-document-url" class="form-control" type="url" inputmode="url" placeholder="https://example.feishu.cn/wiki/... 或 /docx/..." autocomplete="off" oninput="queueFeishuDocumentPreview(this.value)">
            <button type="submit" class="btn-normal" id="feishu-document-add">添加来源</button>
            <div id="feishu-document-preview" class="feishu-document-preview" aria-live="polite"></div>
          </form>
          <div id="feishu-documents-body"><div class="admin-skeleton" aria-hidden="true"></div></div>
        </section>
        <section class="section-panel ks-panel" data-panel="zsxq" role="tabpanel" id="ks-panel-zsxq" aria-labelledby="ks-tab-zsxq">
        <header class="section-head"><div><h2 class="section-title">知识星球</h2>
        <p class="section-meta">Cookie 与抓取分开保存，互不覆盖。</p></div></header>
        <div class="ima-source-stack">
        <div class="ima-source-block">
          <header class="ima-source-block-head"><div><h3 class="ima-source-title">Cookie</h3>
          <p class="section-meta">${cookieUpdatedLabel(zq)}${zq.preview ? ` · 预览 ${escapeHtml(zq.preview)}` : ""}。登录 wx.zsxq.com 复制整串（含 zsxq_access_token）。</p></div></header>
          <label class="field-label" for="zq-cookie">知识星球 Cookie</label>
          <textarea id="zq-cookie" class="form-control cookie-paste" rows="3" placeholder="zsxq_access_token=..."></textarea>
          <div class="ima-credential-actions toolbar">
            <button type="button" class="btn-normal" onclick="saveZsxqCookie()">保存知识星球 Cookie</button>
            <button type="button" class="btn-ghost" onclick="pasteCookieField('zq-cookie')">从剪贴板填入</button>
            ${zq.set && !zq.from_env ? `<button type="button" class="btn-ghost danger" onclick="clearSavedCookie('zsxq','知识星球')" aria-label="清除知识星球 Cookie">清除</button>` : ""}
          </div>
        </div>
        <div class="ima-source-block">
          <header class="ima-source-block-head"><div><h3 class="ima-source-title">抓取</h3>
          <p class="section-meta">日常只开关评论和预缓存；翻页、间隔、App 通道在高级里。</p></div></header>
          <div class="cfg-group cfg-group--zsxq">
            <div class="cfg-fields">
              <label class="cfg-field cfg-check" title="新帖自动抓评论入库（可一并推送）；旧帖不动">
                <input id="pc-zq-comments" type="checkbox" ${s.polling_config.zsxq_fetch_comments ? "checked" : ""}>
                <span class="cfg-flag-text">
                  <span>抓取评论</span>
                  <span class="cfg-check-desc">新主题的评论在抓帖时一并入库</span>
                </span>
              </label>
              <label class="cfg-field cfg-check" title="抓到新帖时就把 PDF 拉到本地；默认关闭，点开再下，省日限">
                <input id="pc-zq-prefetch" type="checkbox" ${s.polling_config.zsxq_prefetch_files ? "checked" : ""}>
                <span class="cfg-flag-text">
                  <span>抓取时预缓存附件</span>
                  <span class="cfg-check-desc">打开后新帖 PDF 会立刻落到本地，费配额；默认点开再下</span>
                </span>
              </label>
            </div>
            <details class="ks-advanced">
              <summary class="cfg-group-title">${CHEVRON_RIGHT_ICON}<span>高级（翻页、间隔、App 通道）</span></summary>
              <div class="cfg-fields">
                <label class="cfg-field" title="每星球每轮最多翻几页，每页 20 条">
                  <span>单轮翻页<span class="cfg-unit">页</span></span>
                  <input id="pc-zq-pages" type="number" class="form-control" min="1" max="20" value="${s.polling_config.zsxq_max_pages ?? 3}">
                </label>
                <label class="cfg-field" title="列表/详情请求间隔，过短容易触发 1059">
                  <span>请求间隔<span class="cfg-unit">秒</span></span>
                  <input id="pc-zq-delay" type="number" class="form-control" min="0.2" max="10" step="0.1" value="${s.polling_config.zsxq_fetch_delay_seconds ?? 1}">
                </label>
                <label class="cfg-field" title="附件 download_url 请求间隔，过短容易撞日限">
                  <span>附件间隔<span class="cfg-unit">秒</span></span>
                  <input id="pc-zq-file-delay" type="number" class="form-control" min="0.2" max="10" step="0.1" value="${s.polling_config.zsxq_file_delay_seconds ?? 1}">
                </label>
                <label class="cfg-field" title="单主题评论最多翻几页（每页 20 条）">
                  <span>评论翻页<span class="cfg-unit">页</span></span>
                  <input id="pc-zq-comment-pages" type="number" class="form-control" min="1" max="10" value="${s.polling_config.zsxq_max_comment_pages ?? 3}">
                </label>
                <label class="cfg-field" title="每轮最多发起的评论请求数，保护限流">
                  <span>评论预算<span class="cfg-unit">次/轮</span></span>
                  <input id="pc-zq-comment-budget" type="number" class="form-control" min="1" max="200" value="${s.polling_config.zsxq_comment_budget ?? 30}">
                </label>
                <label class="cfg-field cfg-check" title="用 App 通道请求头（xiaomiquan UA + X-Request-Id/X-Version）代替浏览器头；默认关，等你复测日限差异确认有收益再开">
                  <input id="pc-zq-app" type="checkbox" ${s.polling_config.zsxq_app_channel ? "checked" : ""}>
                  <span class="cfg-flag-text">
                    <span>App 通道头</span>
                    <span class="cfg-check-desc">伪称 Android 客户端请求；与 web 通道共用账号配额</span>
                  </span>
                </label>
                <label class="cfg-field cfg-field--wide" title="App 通道 UA 里的设备标识：Android 版本 + 品牌_型号，空格自动压成下划线">
                  <span>设备标识<span class="cfg-unit">RELEASE BRAND_MODEL</span></span>
                  <input id="pc-zq-app-device" type="text" class="form-control" maxlength="64" value="${escapeHtml(s.polling_config.zsxq_app_device ?? "16 OnePlus_PJD110")}">
                </label>
              </div>
            </details>
            <div class="cfg-foot">
              <p class="muted" id="zq-cache-stat">附件缓存 ${zcSize} / ${zc.files || 0} 个文件</p>
              <div class="toolbar">
                <button type="button" class="btn-ghost" onclick="purgeZsxqCache()">清理未引用</button>
                <button type="button" class="btn-normal" id="pc-zq-save" onclick="saveZsxqPollingConfig()">保存星球设置</button>
              </div>
            </div>
          </div>
        </div>
        </div>
      </section>
      ${imaStoragePanelHtml(imaCollector.storage)}
      <section class="section-panel ks-panel" data-panel="local" role="tabpanel" id="ks-panel-local" aria-labelledby="ks-tab-local">
        <header class="section-head"><div><h2 class="section-title">本地库</h2>
        <p class="section-meta">存储机 <code>local/&lt;slug&gt;/</code> 下的文件夹研报库；中金研报由存储机采集脚本写入 cicc-research 库。启用并授权用户后即可在研报中心阅读。</p></div></header>
        <div id="local-libs-body"><p class="muted">加载中…</p></div>
      </section>
      </div>`;
    renderStatsData(s);
    if (statsLoadError) {
      const error = $("#stats-poll-error");
      const retry = `<div><button type="button" class="btn-normal" onclick="loadAdminKnowledge(${seq})">重试</button></div>`;
      if (error) error.innerHTML = `<div class="ima-folder-state ima-folder-error" role="alert">${escapeHtml(statsLoadError)}${retry}</div>`;
    }
    if (collectorDraft) initImaMountState(collectorGroups, true);
    else initImaMountState(pure.groups || [], preserveMountDraftForReload);
    renderImaMountGroups();
    renderImaGroupAcl();
    document.querySelectorAll(".ima-interval-seg").forEach((seg) => {
      const current = imaGroupIntervalSeconds(imaMountGroup(seg.dataset.groupId));
      seg.querySelectorAll("button[data-sec]").forEach((btn) => {
        btn.classList.toggle("is-on", Number(btn.dataset.sec) === current);
      });
    });
    restoreImaCollectorOwnerToken(owner, seq, pendingCollectorDraft);
    if (confirmedCollectorDraft) {
      const confirmedRevision = imaMountState.collectorConfirmedRevision;
      const confirmedLiveRevision = imaMountState.collectorConfirmedLiveRevision;
      if (imaMountState.collectorDraftRevision === confirmedRevision
        && imaMountState.collectorRevision === confirmedLiveRevision) {
        clearImaCollectorDraft(confirmedRevision);
        imaMountState.collectorConfirmedRevision = "";
        imaMountState.collectorConfirmedLiveRevision = -1;
        imaMountState.collectorConfirmedMountRevision = -1;
        imaMountState.dirty = false;
        const tokenInput = $("#ima-pure-token");
        if (tokenInput) tokenInput.value = "";
      }
    }
    let savedTab = "collect";
    try { savedTab = sessionStorage.getItem(KS_TAB_KEY) || "collect"; } catch { /* ignore */ }
    const knowledgeQuery = routeQuery();
    if (knowledgeQuery.get("tab") === "feishu") savedTab = "feishu";
    const oauthResult = knowledgeQuery.get("oauth");
    if (oauthResult) {
      flash(oauthResult === "success" ? "飞书文档授权已更新" : "飞书文档授权失败", oauthResult === "success" ? "success" : "error");
      history.replaceState(null, "", "/admin/knowledge");
    }
    switchKnowledgeSettingsTab(savedTab);
    if (imaCollector.running) startImaProgressPoll();
    else {
      stopImaProgressPoll();
      applyImaCollectorProgress(imaCollector);
    }
    startDashboardLiveTimer();
    return true;
  }

  return {
    onKnowledgeTabsKey,
    switchKnowledgeSettingsTab,
    loadAdminKnowledge,
  };
}
