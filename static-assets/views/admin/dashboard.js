export function createAdminDashboardView(dependencies) {
  const {
    $,
    state,
    api,
    flash,
    escapeHtml,
    routeStillActive,
    currentAdminSeq,
    emptyState,
    statsTabsHtml,
    switchStatsTab,
    statsTabFromHash,
    startDashboardLiveTimer,
    stopStatsTimer,
    fmtTs,
    fmtDbTime,
    parseDbUtcMs,
    PLATFORM_LABELS,
    CHANNEL_LABELS,
    imaMountState,
    sessionOwnerStillActive,
    imaCollectorStatusText,
    imaGroupDiscoveryStatusText,
    go,
    nextStatsLoadSeq,
    currentStatsLoadSeq,
    getStatsSnapshot,
    setStatsSnapshot,
    cookieRepairItems,
    cookieRepairBanner,
    cookieUpdatedLabel,
    imgbedStatusLabel,
    setPageTitle,
    STALE_KOL_HOURS,
    STALE_KOL_LIMIT,
    PLATFORM_ICONS,
    fmtCacheBytes,
    loadAdminNews,
    replaceRoute,
    imaCollectorFormSnapshot,
    rememberImaCollectorDraft,
    imaCollectorFormRevision,
    rateBar,
    currentRouteSeq,
  } = dependencies;

  async function loadAdminStats(seq = currentAdminSeq(), authoritativeImaStatus = null) {
    if (!routeStillActive(seq)) return false;
    const tab = statsTabFromHash();
    if (tab === "news") return loadAdminNews(seq);
    if (tab === "legacy-dashboard") {
      replaceRoute("admin/dashboard");
      return false;
    }
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
        const retry = `<div><button type="button" class="btn-normal" onclick="loadAdminStats(${seq})">重试</button></div>`;
        const error = $("#stats-poll-error");
        if (error && document.body.contains(error)) {
          error.innerHTML = `<div class="ima-folder-state ima-folder-error" role="alert">${escapeHtml(message)}${retry}</div>`;
        } else {
          const body = $("#admin-body");
          if (body) body.innerHTML = emptyState(message, retry);
        }
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
    const xq = s.xueqiu_cookie || {};
    const tw = s.twitter_cookie || {};
    const ima = s.ima_credentials || {};
    const imaCollector = s.ima_collector || {};
    const pure = imaCollector.config || {};
    const collectorDraft = confirmedCollectorDraft ? null
      : (ownerSnapshot || (ownerIsCurrent && owner.putCompleted ? null : pendingCollectorDraft));
    const collector = collectorDraft || pure;
    const collectorGroups = collectorDraft?.groups || pure.groups || [];
    const zq = s.zsxq_cookie || {};
    const zc = s.zsxq_cache || { files: 0, bytes: 0 };
    const zcSize = fmtCacheBytes(zc.bytes);
    $("#admin-body").innerHTML = `
      <div id="stats-poll-error"></div>
      ${statsTabsHtml("config")}
      <div id="st-plaza" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-plaza" style="display:none">
        <section class="section-panel">
          <header class="section-head">
            <div><h2 class="section-title">动态广场显示</h2>
            <p class="section-meta">控制时间线角标和「全部」里的内容。自动：该源启用大V 为 0 时隐藏；也可手动显示或隐藏。</p></div>
          </header>
          <div id="plaza-sources">${plazaSourceRowsHtml(s.plaza_sources)}</div>
        </section>
      </div>
      <div id="st-config" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-config">
        <section class="section-panel">
          <header class="section-head">
            <div><h2 class="section-title">抓取设置</h2>
            <p class="section-meta">按抓取档位分组配置；保存后即时生效，无需重启。</p></div>
          </header>
          <div class="cfg-stack">
            <div class="cfg-grid">
              <div class="cfg-group">
                <p class="cfg-group-title">基础轮询</p>
                <div class="cfg-fields">
                  <label class="cfg-field" title="全局轮询间隔（所有大V的最低抓取频率）">
                    <span>轮询间隔<span class="cfg-unit">秒</span></span>
                    <input id="pc-interval" type="number" class="form-control" min="1" max="3600" value="${s.polling_config.interval_seconds}">
                  </label>
                  <label class="cfg-field" title="标记为「优先」的大V用更短间隔抓取，新帖更早送达">
                    <span>优先大V间隔<span class="cfg-unit">秒</span></span>
                    <input id="pc-priority" type="number" class="form-control" min="1" max="600" value="${s.polling_config.priority_interval_seconds}">
                  </label>
                  <label class="cfg-field" title="Truth Social 专属抓取间隔，直连官方 API 实时拉取（匿名限速约 6 次/10 秒，建议不低于 10）；0 = 跟随优先大V间隔">
                    <span>Truth 间隔<span class="cfg-unit">秒</span></span>
                    <input id="pc-truth" type="number" class="form-control" min="0" max="600" value="${s.polling_config.truth_interval_seconds}">
                  </label>
                  <label class="cfg-field" title="普通大V帖子按此周期合并推送摘要；0 = 实时单条推送">
                    <span>合并推送周期<span class="cfg-unit">秒</span></span>
                    <input id="pc-digest" type="number" class="form-control" min="0" max="86400" value="${s.polling_config.digest_interval_seconds}">
                  </label>
                </div>
              </div>
              <div class="cfg-group">
                <p class="cfg-group-title">自适应降频 <span class="hint">无新帖自动拉长</span></p>
                <div class="cfg-fields">
                  <label class="cfg-field" title="普通大V长期无新帖时封顶的空轮间隔，控制对平台的请求频率">
                    <span>普通大V空轮封顶<span class="cfg-unit">秒</span></span>
                    <input id="pc-nc" type="number" class="form-control" min="5" max="86400" value="${s.polling_config.normal_idle_cap_seconds}">
                  </label>
                  <label class="cfg-field" title="优先大V长期无新帖时封顶的空轮间隔">
                    <span>优先大V空轮封顶<span class="cfg-unit">秒</span></span>
                    <input id="pc-pc" type="number" class="form-control" min="5" max="86400" value="${s.polling_config.priority_idle_cap_seconds}">
                  </label>
                  <label class="cfg-field" title="X 直抓失败期间封顶的抓取间隔，失败期放慢以免空打接口">
                    <span>X失败封顶<span class="cfg-unit">秒</span></span>
                    <input id="pc-xc" type="number" class="form-control" min="5" max="86400" value="${s.polling_config.x_fallback_cap_seconds}">
                  </label>
                </div>
              </div>
              <div class="cfg-group">
                <p class="cfg-group-title">雪球组合 <span class="hint">调仓实时推送</span></p>
                <div class="cfg-fields">
                  <label class="cfg-field" title="组合抓取频率；无新帖时自动拉长（2 倍步进），调仓出现即恢复">
                    <span>组合基础间隔<span class="cfg-unit">秒</span></span>
                    <input id="pc-cb" type="number" class="form-control" min="5" max="3600" value="${s.polling_config.combination_base_seconds}">
                  </label>
                  <label class="cfg-field" title="组合长期无调仓时封顶的空轮间隔，避免空转刷接口">
                    <span>组合空轮封顶<span class="cfg-unit">秒</span></span>
                    <input id="pc-cc" type="number" class="form-control" min="5" max="86400" value="${s.polling_config.combination_idle_cap_seconds}">
                  </label>
                </div>
              </div>
              <div class="cfg-group">
                <p class="cfg-group-title">次要大V <span class="hint">低频合并</span></p>
                <div class="cfg-fields">
                  <label class="cfg-field" title="次要大V基础抓取间隔（低于普通大V频率）">
                    <span>抓取间隔<span class="cfg-unit">秒</span></span>
                    <input id="pc-si" type="number" class="form-control" min="60" max="86400" value="${s.polling_config.secondary_interval_seconds}">
                  </label>
                  <label class="cfg-field" title="次要大V长期无新帖时封顶的空轮间隔">
                    <span>空轮封顶<span class="cfg-unit">秒</span></span>
                    <input id="pc-sc" type="number" class="form-control" min="60" max="86400" value="${s.polling_config.secondary_idle_cap_seconds}">
                  </label>
                  <label class="cfg-field" title="次要大V帖子按此周期合并推送；0 = 实时推送">
                    <span>推送周期<span class="cfg-unit">秒</span></span>
                    <input id="pc-sd" type="number" class="form-control" min="0" max="86400" value="${s.polling_config.secondary_digest_interval_seconds}">
                  </label>
                  <label class="cfg-field" title="合并推送最低条数：周期内积压不足此数则不推送、继续攒，够数才推">
                    <span>最低条数<span class="cfg-unit">条</span></span>
                    <input id="pc-sd-min" type="number" class="form-control" min="1" max="100" value="${s.polling_config.secondary_min_digest_count ?? 1}">
                  </label>
                </div>
              </div>
            </div>
            <div class="cfg-group">
              <p class="cfg-group-title">通道</p>
              <div class="cfg-flags">
                <label class="cfg-field cfg-check" title="X / Truth 内容自动翻译成中文（配置 TWITTER_COOKIE 后走 X 官方翻译，质量同网页版）">
                  <input id="pc-translate" type="checkbox" ${s.polling_config.translate_twitter_content ? "checked" : ""}>
                  <span class="cfg-flag-text">
                    <span>X / Truth 内容自动翻译成中文</span>
                    <span class="cfg-check-desc">抓取时保存译文和原文；用户可在推送设置里选看哪一种</span>
                  </span>
                </label>
                <label class="cfg-field cfg-check" title="关闭后全部退回旧版 sendMessage + HTML，配图走相册">
                  <input id="pc-tg-rich" type="checkbox" ${s.polling_config.telegram_rich_messages !== false ? "checked" : ""}>
                  <span class="cfg-flag-text">
                    <span>Telegram Rich Message</span>
                    <span class="cfg-check-desc">标题分层、表格、图文一条；关掉则用原来的 HTML</span>
                  </span>
                </label>
              </div>
            </div>
            <div class="cfg-group">
              <p class="cfg-group-title">保活与定时</p>
              <div class="cfg-fields">
                <label class="cfg-field" title="雪球保活探测间隔；0 = 关闭自动保活">
                  <span>雪球探测<span class="cfg-unit">秒</span></span>
                  <input id="pc-probe" type="number" class="form-control" min="0" max="86400" value="${s.polling_config.source_probe_interval_seconds}">
                </label>
                <label class="cfg-field" title="登录态自动保活间隔；0 = 关闭">
                  <span>cookie保活<span class="cfg-unit">秒</span></span>
                  <input id="pc-keepalive" type="number" class="form-control" min="0" max="86400" value="${s.polling_config.cookie_keepalive_interval_seconds}">
                </label>
                <label class="cfg-field" title="每日精选推送的小时（0-23，北京时间）">
                  <span>每日精选<span class="cfg-unit">时</span></span>
                  <input id="pc-daily" type="number" class="form-control" min="0" max="23" value="${s.polling_config.daily_report_hour}">
                </label>
              </div>
            </div>
          </div>
          <div class="cfg-save-row">
            <button type="button" class="btn-normal" id="pc-save" onclick="savePollingConfig()">保存抓取设置</button>
          </div>
          <p class="section-meta"><a href="/admin/knowledge" onclick="event.preventDefault();go('admin/knowledge')">IMA 与知识星球设置已移至研报设置</a></p>
        </section>
      </div>
      <div id="st-cookies" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-cookies" style="display:none">
        <div id="cookie-repair-inline"></div>
        <section class="section-panel">
          <header class="section-head">
            <div><h2 class="section-title">雪球 Cookie</h2>
            <p class="section-meta">${cookieUpdatedLabel(xq)}${xq.preview ? ` · 预览 ${escapeHtml(xq.preview)}` : ""}${s.keepalive_interval_seconds > 0 ? ` · 每 ${Math.round(s.keepalive_interval_seconds / 3600)} 小时探测` : ""}。登录 xueqiu.com → F12 → Application → Cookies，复制整串后保存，即时生效。</p></div>
          </header>
          <label class="field-label" for="xq-cookie">雪球 Cookie</label>
          <textarea id="xq-cookie" class="form-control cookie-paste" rows="4" placeholder="xq_a_token=...; u=..."></textarea>
          <div class="toolbar" style="margin-top:12px">
            <button type="button" class="btn-normal" onclick="saveXueqiuCookie()">保存雪球 Cookie</button>
            <button type="button" class="btn-ghost" onclick="pasteCookieField('xq-cookie')">从剪贴板填入</button>
            ${xq.set && !xq.from_env ? `<button type="button" class="btn-ghost danger" onclick="clearSavedCookie('xueqiu','雪球')" aria-label="清除雪球 Cookie">清除</button>` : ""}
          </div>
        </section>
        <section class="section-panel">
          <header class="section-head"><div><h2 class="section-title">微博 Cookie</h2>
          <p class="section-meta">${cookieUpdatedLabel(s.weibo_cookie)}。用微博 App 扫码后自动保存，无需复制。</p></div></header>
          <div class="toolbar">
            <button type="button" class="btn-normal" id="wb-qr-start" onclick="startWeiboQr()">微博扫码登录</button>
            ${s.weibo_cookie?.set && !s.weibo_cookie.from_env ? `<button type="button" class="btn-ghost danger" onclick="clearSavedCookie('weibo','微博')" aria-label="清除微博 Cookie">清除</button>` : ""}
          </div>
          <div id="wb-qr-box" class="qr-box"></div>
        </section>
        <section class="section-panel">
          <header class="section-head">
            <div><h2 class="section-title">X 抓取通道</h2>
            <p class="section-meta">${xChannelSummary(s.x_channels || {})}</p></div>
          </header>
          <header class="section-head">
            <div><h2 class="section-title">X Cookie</h2>
            <p class="section-meta">${cookieUpdatedLabel(tw)}${tw.preview ? ` · 预览 ${escapeHtml(tw.preview)}` : ""}。登录 x.com → F12 → Application → Cookies，复制整串（需含 auth_token 与 ct0），保存即时生效。</p></div>
          </header>
          <label class="field-label" for="tw-cookie">X Cookie</label>
          <textarea id="tw-cookie" class="form-control cookie-paste" rows="4" placeholder="auth_token=...; ct0=..."></textarea>
          <div class="toolbar" style="margin-top:12px">
            <button type="button" class="btn-normal" onclick="saveTwitterCookie()">保存 X Cookie</button>
            <button type="button" class="btn-ghost" onclick="pasteCookieField('tw-cookie')">从剪贴板填入</button>
            ${tw.set && !tw.from_env ? `<button type="button" class="btn-ghost danger" onclick="clearSavedCookie('twitter','X')" aria-label="清除 X Cookie">清除</button>` : ""}
          </div>
        </section>
        <p class="section-meta"><a href="/admin/knowledge" onclick="event.preventDefault();go('admin/knowledge')">IMA 与知识星球设置已移至研报设置</a></p>
      </div>
      <div id="st-imgbed" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-imgbed" style="display:none">
        ${imgbedSettingsHtml(s.imgbed || {})}
      </div>
      <div id="st-proxies" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-proxies" style="display:none"></div>`;
    renderStatsData(s);
    if (statsLoadError) {
      const error = $("#stats-poll-error");
      const retry = `<div><button type="button" class="btn-normal" onclick="loadAdminStats(${seq})">重试</button></div>`;
      if (error) error.innerHTML = `<div class="ima-folder-state ima-folder-error" role="alert">${escapeHtml(statsLoadError)}${retry}</div>`;
    }
    switchStatsTab(statsTabFromHash());
    return true;
  }

  // X 抓取通道总览文案：模式 / 分流覆盖 / 429 冷却 / 凭证就位情况。
  // 两条通道的凭证来源与续期方式不同，后台必须分得清，否则排查会指错方向。
  function xChannelSummary(ch) {
    if (!ch || !ch.mode) return "正在加载通道状态…";
    const split = ch.split || {};
    const cs = ch.channels || {};
    const cool = [];
    if (cs.app?.cooling) {
      cool.push(`App 通道 429 冷却 ${Math.round((cs.app.cooling_left_seconds || 0) / 60)} 分钟`);
    }
    if (cs.cookie?.cooling) {
      cool.push(`Cookie 通道 429 冷却 ${Math.round((cs.cookie.cooling_left_seconds || 0) / 60)} 分钟`);
    }
    let mode;
    if (ch.mode === "split") {
      mode = `双通道分流 —— App ${split.app || 0} 个 / Cookie ${split.cookie || 0} 个`
        + "（两侧配额在服务端独立计数，容量约翻倍）";
    } else if (ch.mode === "app_only") {
      mode = `仅 App 通道 —— 全部 ${ch.kol_total || 0} 个大V（Cookie 未配置）`;
    } else {
      mode = `仅 Cookie 通道 —— 全部 ${ch.kol_total || 0} 个大V（App 凭证未配置）`;
    }
    const creds = [];
    if (ch.app_ready) creds.push(`App 凭证已配置（${ch.app_token_len || 0} 字符）`);
    if (ch.cookie_ready) creds.push(`Cookie 已配置（${ch.cookie_len || 0} 字符）`);
    const coolTail = cool.length ? `　·　${cool.join("；")}` : "";
    const credTail = creds.length ? `　·　${creds.join("　·　")}` : "";
    return `${mode}${coolTail}${credTail}`;
  }

  function imgbedSettingsHtml(info) {
    const connected = !!info.enabled;
    const tokenHint = info.token_set
      ? (info.token_from_env ? "已从环境变量读取，留空保持原密钥" : "已配置，留空保持原密钥")
      : "在图床后台「安全设置 → API Token 管理」创建，勾上传和删除权限";
    const counts = connected
      ? `已镜像 ${info.ready_count || 0} 张 · 排队 ${info.pending_count || 0} 张 · 失败 ${info.failed_count || 0} 张`
      : "未接入时 X 配图仍走服务端代理";
    const checkError = connected && info.last_check_error
      ? `<p class="cfg-field-error" role="alert">上次连通检查失败：${escapeHtml(info.last_check_error)}。保存后自动重查。</p>`
      : "";
    const failedNotice = info.failed_count
      ? `<div class="notice notice-warn" role="status">
          <div class="notice-warn-body">
            <strong>有 ${info.failed_count} 张配图镜像失败</strong>
            <p>会自动重试；持续失败先检查图床地址和密钥是否能打开图床。</p>
          </div>
        </div>`
      : "";
    return `${failedNotice}<section class="section-panel">
      <header class="section-head">
        <div>
          <h2 class="section-title">图床</h2>
          <p class="section-meta">${imgbedStatusLabel(info)} · ${counts}。把 X 配图镜像到自建图床，大陆直连可看；代理兜底一直在。接线：打开图床后台 → 创建 API 密钥（勾上传和删除）→ 地址和密钥贴到下面，保存即生效。开源项目 <a href="https://github.com/MarSeventh/CloudFlare-ImgBed" target="_blank" rel="noopener">CloudFlare-ImgBed</a>。</p>
        </div>
      </header>
      <label class="field-label" for="imgbed-base-url">图床地址</label>
      <input id="imgbed-base-url" class="form-control" type="url" inputmode="url" autocomplete="off" spellcheck="false" value="${escapeHtml(info.base_url || "")}" placeholder="https://img.example.com">
      ${checkError}
      <label class="field-label" for="imgbed-token">API 密钥</label>
      <input id="imgbed-token" class="form-control" type="password" autocomplete="new-password" placeholder="${escapeHtml(tokenHint)}">
      <div class="cfg-fields">
        <label class="cfg-field" for="imgbed-retention-days" title="超过后删除图床文件和镜像记录，展示退回原图；0 表示不清理">
          <span>图片保留时间<span class="cfg-unit">天</span></span>
          <input id="imgbed-retention-days" class="form-control" type="number" min="0" max="3650" value="${info.retention_days ?? 30}">
        </label>
      </div>
      <details class="imgbed-advanced">
        <summary class="cfg-group-title">高级</summary>
        <div class="cfg-fields">
          <label class="cfg-field" for="imgbed-channel-name">
            <span>上传渠道名<span class="cfg-unit">图床后台里的渠道</span></span>
            <input id="imgbed-channel-name" class="form-control" autocomplete="off" value="${escapeHtml(info.channel_name || "vpush-imgbed")}" placeholder="vpush-imgbed">
          </label>
          <label class="cfg-field" for="imgbed-folder">
            <span>目录<span class="cfg-unit">图床内的存放目录</span></span>
            <input id="imgbed-folder" class="form-control" autocomplete="off" value="${escapeHtml(info.folder || "vpush")}" placeholder="vpush">
          </label>
        </div>
      </details>
      <div class="toolbar" style="margin-top:12px">
        <button type="button" class="btn-normal" onclick="saveImgbedSettings()">保存图床设置</button>
        <button type="button" class="btn-ghost" onclick="pasteCookieField('imgbed-token')">从剪贴板填入</button>
        ${connected ? `<button type="button" class="btn-ghost danger" onclick="clearImgbedSettings()" aria-label="清除图床设置">清除</button>` : ""}
      </div>
    </section>`;
  }

  function plazaSourceEffect(row) {
    if (row.mode === "hide") return "已手动隐藏";
    if (row.mode === "show") return "已手动显示";
    return row.enabled_kols > 0 ? "有启用大V，自动显示" : "启用大V 为 0，自动隐藏";
  }

  function plazaSourceRowsHtml(rows) {
    const modes = [["auto", "自动"], ["show", "显示"], ["hide", "隐藏"]];
    return (rows || []).map((row) => {
      const label = PLATFORM_LABELS[row.platform] || row.platform;
      const shown = row.visible ? "广场显示中" : "广场已隐藏";
      return `<div class="plaza-src" data-platform="${escapeHtml(row.platform)}">
        <div class="plaza-src-head">
          <span class="plaza-src-icon" data-platform="${escapeHtml(row.platform)}">${PLATFORM_ICONS[row.platform] || ""}</span>
          <div class="plaza-src-copy">
            <p class="plaza-src-name">${escapeHtml(label)}</p>
            <p class="plaza-src-meta">启用大V ${row.enabled_kols} · ${shown} · ${plazaSourceEffect(row)}</p>
          </div>
        </div>
        <div class="plaza-src-modes" role="radiogroup" aria-label="${escapeHtml(label)} 广场显示">
          ${modes.map(([value, text]) => `
            <button type="button" class="plaza-src-mode ${row.mode === value ? "selected" : ""}" role="radio" data-platform="${escapeHtml(row.platform)}" data-mode="${value}" aria-checked="${row.mode === value}" onclick="setPlazaSourceMode(this.dataset.platform,this.dataset.mode)">${text}</button>`).join("")}
        </div>
      </div>`;
    }).join("");
  }

  function applyPlazaSources(sources) {
    const box = $("#plaza-sources");
    if (box) box.innerHTML = plazaSourceRowsHtml(sources);
    if (state.user) {
      state.user.plaza_platforms = (sources || []).filter((row) => row.visible).map((row) => row.platform);
      const vis = new Set(state.user.plaza_platforms);
      if (Array.isArray(state.user.timeline_platforms)) {
        state.user.timeline_platforms = state.user.timeline_platforms.filter((p) => vis.has(p));
      }
    }
  }

  async function setPlazaSourceMode(platform, mode) {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const current = document.querySelector(`.plaza-src[data-platform="${CSS.escape(platform)}"] .plaza-src-mode.selected`);
    if (current && current.dataset.mode === mode) return;
    try {
      const data = await api("/api/admin/plaza-sources", {
        method: "PUT",
        body: JSON.stringify({ visibility: { [platform]: mode } }),
      });
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      applyPlazaSources(data.sources);
      flash("广场显示已更新");
    } catch (err) {
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash(err.message, "error");
    }
  }

  function staleEnabledKolRows(rows, nowMs) {
    const cutoff = (nowMs || Date.now()) - STALE_KOL_HOURS * 3600 * 1000;
    const live = (rows || []).filter((k) => k.enabled && (k.subscriber_count == null || Number(k.subscriber_count) > 0));
    const stale = live.filter((k) => {
      if (k.fetch_state && k.fetch_state !== "ok") return false;
      if (!k.last_post_at) return true;
      const ts = parseDbUtcMs(k.last_post_at);
      return ts == null || ts < cutoff;
    });
    stale.sort((a, b) => String(a.last_post_at || "").localeCompare(String(b.last_post_at || "")));
    return stale;
  }

  function staleEnabledKols(rows, nowMs) {
    return staleEnabledKolRows(rows, nowMs).slice(0, STALE_KOL_LIMIT);
  }

  function openAdminKolFromHealth(name) {
    state.adminKolsQ = String(name || "").trim();
    state.adminKolsPage = 0;
    go("admin/kols");
  }

  function sourceNeverStarted(src) {
    return !src.ok
      && !src.last_ok_at
      && !(Number(src.ok_24h) || 0)
      && !(Number(src.fail_24h) || 0)
      && !(Number(src.consecutive_fails) || 0);
  }

  function sourceCredentialGap(src, cookieItems) {
    const keys = new Set((cookieItems || []).map((item) => item.key));
    const plat = src.platform;
    if ((plat === "xueqiu" || plat === "combination") && (keys.has("xq-missing") || keys.has("xq-bad"))) return true;
    if (plat === "weibo" && keys.has("wb-bad")) return true;
    if (plat === "twitter" && (keys.has("x-bad") || keys.has("x-missing"))) return true;
    if (plat === "zsxq" && (keys.has("zq-missing") || keys.has("zq-bad"))) return true;
    return false;
  }

  function sourceStatusNote(src) {
    if (src.platform !== "twitter" || src.direct_mode !== "fallback") return "";
    return ` <span class="status-warn" title="${escapeHtml(src.direct_fallback_reason || "")}">直抓失败</span>`;
  }

  function armStatusCell(src) {
    const arm = src.arm || {};
    if (!arm.configured && !arm.last_finished_at) {
      return `<td class="muted" data-label="状态">ARM 未配置</td>`;
    }
    if (arm.last_error || arm.failed) {
      return `<td class="status-warn" data-label="状态">ARM 异常</td>`;
    }
    if (!arm.last_finished_at) {
      return `<td class="muted" data-label="状态">ARM 中间层</td>`;
    }
    return `<td class="status-ok" data-label="状态">ARM 中间层</td>`;
  }

  function armRateCell(src) {
    const arm = src.arm || {};
    if (!arm.last_finished_at) {
      return `<td class="muted ak-hide-mobile dash-source-rate" data-label="24h 成功率">由 ARM 采集</td>`;
    }
    const when = fmtTs(arm.last_finished_at);
    return `<td class="ak-hide-mobile dash-source-rate" data-label="24h 成功率" title="上次采集 ${escapeHtml(when)}">上次 ${arm.downloaded || 0} 篇</td>`;
  }

  function armCauseCell(src) {
    const arm = src.arm || {};
    if (arm.last_error) {
      const text = String(arm.last_error);
      return `<td class="muted dash-source-cause" data-label="最近错误" title="${escapeHtml(text)}">${escapeHtml(text.slice(0, 40))}</td>`;
    }
    return `<td class="muted dash-source-cause" data-label="最近错误">不走本机轮询</td>`;
  }

  function sourceStatusCell(src, cookieItems) {
    if (src.managed_by === "arm") return armStatusCell(src);
    const note = sourceStatusNote(src);
    const health = src.health || "";
    if (health === "ok" || (!health && src.ok)) {
      return `<td class="status-ok" data-label="状态">正常${note}</td>`;
    }
    if (sourceCredentialGap(src, cookieItems)) {
      return `<td class="dash-status-cred" data-label="状态">凭据缺失${note}</td>`;
    }
    if (health === "idle") {
      return `<td class="muted" data-label="状态">未启用${note}</td>`;
    }
    if (health === "never" || sourceNeverStarted(src)) {
      return `<td class="muted" data-label="状态">未开始${note}</td>`;
    }
    if (health === "down" || src.consecutive_fails >= 3) {
      return `<td class="status-fail" data-label="状态">持续失败${note}</td>`;
    }
    if (health === "overdue") {
      return `<td class="status-warn" data-label="状态">逾期${note}</td>`;
    }
    return `<td class="status-warn" data-label="状态">失败${note}</td>`;
  }

  function sourceRowsHtml(sources, cookieItems) {
    const rows = sources || [];
    if (!rows.length) return '<tr class="ak-empty"><td colspan="4" class="muted">暂无数据源</td></tr>';
    return rows.map((src) => {
      if (src.managed_by === "arm") {
        const when = src.arm && src.arm.last_finished_at ? `上次采集 ${fmtTs(src.arm.last_finished_at)}` : "";
        return `
      <tr${when ? ` title="${escapeHtml(when)}"` : ""}>
        <td data-label="平台">${PLATFORM_LABELS[src.platform] || escapeHtml(src.platform)}</td>
        ${armStatusCell(src)}
        ${armRateCell(src)}
        ${armCauseCell(src)}
      </tr>`;
      }
      const warn = src.warn_24h ? ` <span class="status-warn">⚠${src.warn_24h}</span>` : "";
      const counts = `<span class="muted dash-source-counts">${src.ok_24h} / ${src.fail_24h}${warn}</span>`;
      const hint = [
        src.consecutive_fails ? `连续失败 ${src.consecutive_fails}` : "",
        src.next_retry_at ? `下次重试 ${fmtTs(src.next_retry_at)}` : "",
        src.last_ok_at ? `最近成功 ${fmtTs(src.last_ok_at)}` : "",
      ].filter(Boolean).join(" · ");
      return `
      <tr${hint ? ` title="${escapeHtml(hint)}"` : ""}>
        <td data-label="平台">${PLATFORM_LABELS[src.platform] || escapeHtml(src.platform)}</td>
        ${sourceStatusCell(src, cookieItems)}
        <td class="ak-hide-mobile dash-source-rate" data-label="24h 成功率">${rateBar(src.success_rate_24h)}${counts}</td>
        ${sourceCauseCell(src, cookieItems)}
      </tr>`;
    }).join("");
  }

  function sourceCauseCell(src, cookieItems) {
    if (src.last_error) {
      return `<td class="muted dash-source-cause" data-label="最近错误" title="${escapeHtml(src.last_error)}">${escapeHtml(src.last_error.slice(0, 40))}</td>`;
    }
    if (sourceCredentialGap(src, cookieItems)) {
      return `<td class="dash-source-cause" data-label="最近错误"><button type="button" class="linkish" onclick="go('admin/stats?tab=cookies')">去更新 Cookie</button></td>`;
    }
    if (src.health === "overdue") {
      return `<td class="muted dash-source-cause" data-label="最近错误">到点未抓</td>`;
    }
    if (src.health === "never" || sourceNeverStarted(src)) {
      return `<td class="muted dash-source-cause ak-hide-mobile" data-label="最近错误">还没跑过</td>`;
    }
    return `<td class="muted dash-source-cause" data-label="最近错误">—</td>`;
  }

  function abnormalSourceEvents(events, limit) {
    return (events || []).filter((e) => e.status !== "ok").slice(0, limit || 5);
  }

  function sourceEventRowsHtml(events) {
    const rows = abnormalSourceEvents(events);
    if (!rows.length) return "";
    return `<div class="dash-events">${rows.map((e) => `<div class="dash-event">
      <span class="dash-event-dot ${escapeHtml(e.status)}"></span>
      <span class="muted dash-event-time">${escapeHtml(fmtDbTime(e.created_at))}</span>
      <span class="dash-event-platform">${PLATFORM_LABELS[e.platform] || escapeHtml(e.platform)}</span>
      <span class="${e.status === "warn" ? "status-warn" : "status-fail"}">${e.status === "warn" ? "警告" : "失败"}</span>
      <span class="muted dash-event-detail" title="${escapeHtml(e.detail || "")}">${escapeHtml(e.detail || "")}</span>
    </div>`).join("")}</div>`;
  }

  function dashboardFetchMetaHtml(s) {
    const items = [];
    items.push(s.last_poll_at ? `最近抓取 ${fmtTs(s.last_poll_at)}` : "尚未轮询");
    if (s.last_poll_duration_ms) items.push(`耗时 ${(Number(s.last_poll_duration_ms) / 1000).toFixed(1)} 秒`);
    if (s.enabled_kols != null) items.push(`活跃抓取 ${s.active_kols || 0}/${s.enabled_kols}`);
    items.push(`轮询 ${s.polling_interval_seconds || "—"} 秒`);
    items.push(s.retry_pending
      ? `<span class="status-warn">待重试 ${s.retry_pending} 条</span>`
      : `<span class="status-ok">重试空闲</span>`);
    const alerts = s.alerts || {};
    if (alerts.push_alert_last_at) items.push(`<span class="status-warn">推送告警 ${escapeHtml(fmtTs(alerts.push_alert_last_at))}</span>`);
    if (alerts.x_direct_alert_at) items.push(`<span class="status-warn">X失败告警 ${escapeHtml(fmtTs(alerts.x_direct_alert_at))}</span>`);
    if (alerts.cookie_keepalive_alert_at) items.push(`<span class="status-warn">cookie保活告警 ${escapeHtml(fmtTs(alerts.cookie_keepalive_alert_at))}</span>`);
    if (alerts.xueqiu_probe_alert_at) items.push(`<span class="status-warn">雪球探测告警 ${escapeHtml(fmtTs(alerts.xueqiu_probe_alert_at))}</span>`);
    return `<p class="section-meta dash-fetch-meta" id="dash-fetch-meta">${items.map((bit) => `<span>${bit}</span>`).join("")}</p>`;
  }

  function fmtRelativeFromMs(ms, nowMs) {
    if (ms == null) return "从未";
    const hours = Math.floor(Math.max(0, (nowMs || Date.now()) - ms) / 3600000);
    if (hours < 1) return "不到 1 小时前";
    if (hours < 48) return `${hours} 小时前`;
    return `${Math.floor(hours / 24)} 天前`;
  }

  function staleKolsHtml(rows) {
    const all = staleEnabledKolRows(rows);
    const stale = all.slice(0, STALE_KOL_LIMIT);
    if (!all.length) {
      return `<p class="muted" id="kol-health-empty">有订阅的大V 在 ${STALE_KOL_HOURS} 小时内都抓到过新帖</p>`;
    }
    const extra = all.length > stale.length ? `，列出 ${stale.length} 个` : "";
    const nowMs = Date.now();
    return `<p class="dash-stale-verdict" id="kol-health-verdict">${all.length} 个有订阅大V超过 ${STALE_KOL_HOURS} 小时没抓到新帖${extra}</p>
      <ul class="dash-stale-list">${stale.map((h) => {
        const when = h.last_post_at ? fmtRelativeFromMs(parseDbUtcMs(h.last_post_at), nowMs) : "从未";
        const plat = PLATFORM_LABELS[h.platform] || h.platform || "";
        const subs = Number(h.subscriber_count);
        const subBit = Number.isFinite(subs) && subs > 0 ? ` · ${subs} 订` : "";
        return `<li>
          <button type="button" class="linkish" data-name="${escapeHtml(h.name)}" onclick="openAdminKolFromHealth(this.dataset.name)">${escapeHtml(h.name)}</button>
          <span class="muted">${escapeHtml(plat)} · ${escapeHtml(when)}${escapeHtml(subBit)}</span>
        </li>`;
      }).join("")}</ul>`;
  }

  function dutyStripHtml(s) {
    const sources = s.sources || [];
    const cookies = cookieRepairItems(s);
    let never = 0;
    let cred = 0;
    let failing = 0;
    let overdue = 0;
    sources.forEach((src) => {
      if (src.managed_by === "arm") return;
      const health = src.health || (src.ok ? "ok" : "");
      if (health === "ok" || health === "idle") return;
      if (sourceCredentialGap(src, cookies)) cred += 1;
      else if (health === "never" || sourceNeverStarted(src)) never += 1;
      else if (health === "overdue") overdue += 1;
      else failing += 1;
    });
    const staleAll = staleEnabledKolRows(s.kol_health).length;
    const pending = Number(s.pending_kol_requests) || 0;
    const imgbedFailed = Number((s.imgbed || {}).failed_count) || 0;
    const bits = [];
    if (failing) bits.push(`<li class="is-fail">${failing} 条管线持续失败</li>`);
    if (overdue) bits.push(`<li class="is-warn">${overdue} 条到点未抓</li>`);
    if (cred) bits.push(`<li class="is-warn">${cred} 条凭据缺失</li>`);
    if (never) bits.push(`<li class="is-idle">${never} 条尚未开始抓取</li>`);
    if (staleAll) bits.push(`<li class="is-fail">${staleAll} 个有订阅大V停更</li>`);
    if (imgbedFailed) bits.push(`<li class="is-warn"><button type="button" class="linkish" onclick="go('admin/stats?tab=imgbed')">${imgbedFailed} 张配图镜像失败</button></li>`);
    if (pending) bits.push(`<li class="is-warn"><button type="button" class="linkish" onclick="go('admin/requests')">${pending} 条待审批</button></li>`);
    if (!bits.length) bits.push(`<li class="is-ok">管线正常，没有停更例外</li>`);
    return `<ul class="dash-duty-strip" id="dash-duty-strip">${bits.join("")}</ul>`;
  }

  function renderStatsData(s) {
    const banner = cookieRepairBanner(s);
    const dashCookie = $("#dash-cookie-slot");
    if (dashCookie) dashCookie.innerHTML = banner;
    const cookieInline = $("#cookie-repair-inline");
    if (cookieInline) cookieInline.innerHTML = banner;
    const meta = $("#dash-fetch-meta");
    if (meta) meta.outerHTML = dashboardFetchMetaHtml(s);
    const pollErr = $("#stats-poll-error");
    if (pollErr) {
      pollErr.innerHTML = s.last_poll_error
        ? `<div class="notice">最近轮询异常：${escapeHtml(s.last_poll_error)}</div>`
        : "";
    }
    if (s.plaza_sources) applyPlazaSources(s.plaza_sources);
    const dutySlot = $("#dash-duty-strip-slot");
    if (dutySlot) dutySlot.innerHTML = dutyStripHtml(s);
    const tbody = $("#sources-table");
    if (tbody) tbody.innerHTML = sourceRowsHtml(s.sources, cookieRepairItems(s));
    const events = $("#dash-source-events");
    if (events) events.innerHTML = sourceEventRowsHtml(s.recent_source_events);
    const kh = $("#kol-health");
    if (kh) kh.innerHTML = staleKolsHtml(s.kol_health);
    const stalePanel = $("#dash-stale-panel");
    if (stalePanel) stalePanel.hidden = !staleEnabledKolRows(s.kol_health).length;
    const imaCollectorStatus = $("#ima-collector-status");
    if (imaCollectorStatus && s.ima_collector) imaCollectorStatus.textContent = imaCollectorStatusText(s.ima_collector);
    const imaGroupDiscoveryStatus = $("#ima-group-discovery-status");
    if (imaGroupDiscoveryStatus && s.ima_collector) {
      imaGroupDiscoveryStatus.innerHTML = imaGroupDiscoveryStatusText(s.ima_collector);
    }
  }
  function statCard(label, value) {
    return `
      <div class="dash-stat">
        <div class="dash-stat-label">${escapeHtml(label)}</div>
        <div class="dash-stat-value">${escapeHtml(String(value))}</div>
      </div>`;
  }

  async function loadAdminDashboard() {
    try {
      const [d, st] = await Promise.all([api("/api/admin/dashboard"), api("/api/stats")]);
      state.pendingKolRequests = Number(st.pending_kol_requests) || 0;
      const u = d.users || {};
      const s = d.subscriptions || {};
      const p = d.posts || {};
      const pu = d.pushes || {};
      const CHANNEL_LABELS_LOOKUP = { telegram: "Telegram", feishu: "飞书", wecom: "企业微信" };
      const rate = pu.success_rate != null ? `${pu.success_rate}%` : "—";

      // 14 天推送趋势柱状图（纯 CSS，零依赖）
      const trend = pu.trend_14d || [];
      const maxPushed = Math.max(1, ...trend.map((t) => t.pushed));
      const trendHtml = trend.length
        ? `<div class="dash-trend" role="list" aria-label="近 14 天推送趋势">${trend.map((t) => {
            const fail = Math.max(0, t.pushed - t.ok);
            // 红/绿分别按失败数/成功数相对最大值定高，二者之和 = 总推送量高度，不会溢出
            const failPct = Math.floor((fail / maxPushed) * 100);
            const okPct = Math.floor((t.ok / maxPushed) * 100);
            const tip = `${t.date}：推送 ${t.pushed} 条，成功 ${t.ok}，失败 ${fail}`;
            return `<div class="dash-trend-col" role="listitem" title="${escapeHtml(tip)}" aria-label="${escapeHtml(tip)}">
              <div class="dash-trend-bar">
                <div class="dash-trend-fail" style="height:${failPct}%"></div>
                <div class="dash-trend-ok" style="height:${okPct}%"></div>
              </div>
              <div class="dash-trend-date">${escapeHtml(t.date.slice(5))}</div>
            </div>`;
          }).join("")}</div>`
        : "";

      // 平台来源分布
      const platformRows = Object.entries(p.by_platform || {}).map(([k, v]) => {
        const total = p.total || 1;
        const w = Math.round((v / total) * 100);
        return `<div class="dash-bar-row">
          <span class="dash-bar-label">${PLATFORM_LABELS[k] || escapeHtml(k)}</span>
          <div class="dash-bar-track"><div class="dash-bar-fill" style="width:${w}%"></div></div>
          <span class="dash-bar-value">${v}</span>
        </div>`;
      }).join("");

      // 渠道推送成功率
      const channelRows = Object.entries(pu.by_channel || {}).map(([k, v]) => {
        const r = v.total ? Math.round((v.ok / v.total) * 100) : 0;
        return `<div class="dash-bar-row">
          <span class="dash-bar-label">${CHANNEL_LABELS_LOOKUP[k] || escapeHtml(k)}</span>
          <div class="dash-bar-track"><div class="dash-bar-fill ${r < 90 ? "warn" : ""}" style="width:${r}%"></div></div>
          <span class="dash-bar-value">${v.ok}/${v.total}（${r}%）</span>
        </div>`;
      }).join("");

      const platformSection = platformRows
        ? `<section class="section-panel">
            <header class="section-head"><div><h2 class="section-title">帖子来源分布</h2></div></header>
            ${platformRows}
          </section>`
        : "";
      const channelSection = channelRows
        ? `<section class="section-panel">
            <header class="section-head"><div><h2 class="section-title">渠道推送成功率（7 天）</h2></div></header>
            ${channelRows}
          </section>`
        : "";
      const splitSection = (platformSection || channelSection)
        ? `<div class="dash-split">${platformSection}${channelSection}</div>`
        : "";
      const volumeStats = `<div class="dash-stats">
            ${statCard("近 7 天推送", pu.total_7d || 0)}
            ${statCard("推送成功率", rate)}
            ${statCard("绑定渠道用户", u.bound || 0)}
          </div>`;
      const volumeBody = trendHtml
        ? `<div class="dash-volume">${volumeStats}${trendHtml}</div>`
        : volumeStats;

      if (!routeStillActive(currentAdminSeq())) return;
      setPageTitle("全景概览");
      $("#admin-body").innerHTML = `
        <div id="dash-cookie-slot"></div>
        <div class="dash-duty-grid">
          <section class="section-panel dash-source-panel">
            <header class="section-head">
              <div>
                <h2 class="section-title">数据源健康</h2>
                <div id="dash-duty-strip-slot"></div>
                ${dashboardFetchMetaHtml(st)}
              </div>
              <div class="toolbar"><button type="button" class="btn-ghost" onclick="refreshDashboardLive()">立即刷新</button></div>
            </header>
            <div id="stats-poll-error"></div>
            <div class="table-wrap">
              <table class="ak-table dash-source-table">
                <thead><tr>
                  <th scope="col">平台</th><th scope="col">状态</th>
                  <th class="ak-hide-mobile" scope="col">24h 成功率</th>
                  <th scope="col">最近错误</th>
                </tr></thead>
                <tbody id="sources-table"></tbody>
              </table>
            </div>
            <div id="dash-source-events"></div>
          </section>
          <section class="section-panel dash-stale-panel" id="dash-stale-panel" hidden>
            <header class="section-head"><div><h2 class="section-title">停更大V</h2></div></header>
            <div id="kol-health"></div>
          </section>
        </div>
        <section class="section-panel dash-volume-panel">
          <header class="section-head"><div><h2 class="section-title">核心指标</h2>
          <p class="section-meta">今日新帖 ${p.today || 0} · 今日推送 ${pu.today || 0} · 7 日新用户 ${u.new_7d || 0} · 注册 ${u.total || 0}</p></div></header>
          ${volumeBody}
        </section>
        ${splitSection}`;
      renderStatsData(st);
      startDashboardLiveTimer();
    } catch (err) {
      if (!routeStillActive(currentAdminSeq())) return;
      $("#admin-body").innerHTML = emptyState("加载失败: " + err.message,
        `<div><button class="btn-normal" onclick="loadAdminDashboard()">重试</button></div>`);
    }
  }
  let _adminLogsSeq = 0;
  async function loadAdminLogs() {
    const seq = ++_adminLogsSeq;
    const users = await api("/api/users");
    const logs = await api(`/api/push-logs?limit=100${state.adminLogsFilter || ""}`);
    if (seq !== _adminLogsSeq) return; // 筛选条件已变，丢弃过期响应
    if (!routeStillActive(currentAdminSeq())) return;
    $("#admin-body").innerHTML = `
      <section class="section-panel">
        <header class="section-head">
          <div><h2 class="section-title">推送记录</h2></div>
          <div class="toolbar" style="margin-top:12px">
            <select id="ad-logs-user" class="form-control" style="margin:0;width:auto">
              <option value="">全部用户</option>
              ${users.map((u) => `<option value="${u.id}" ${state.adminLogsUserId == u.id ? "selected" : ""}>${escapeHtml(u.username)}</option>`).join("")}
            </select>
            <select id="ad-logs-channel" class="form-control" style="margin:0;width:auto">
              <option value="">全部渠道</option>
              <option value="telegram" ${state.adminLogsChannel === "telegram" ? "selected" : ""}>Telegram</option>
              <option value="feishu" ${state.adminLogsChannel === "feishu" ? "selected" : ""}>飞书</option>
              <option value="wecom" ${state.adminLogsChannel === "wecom" ? "selected" : ""}>企业微信</option>
            </select>
            <select id="ad-logs-status" class="form-control" style="margin:0;width:auto">
              <option value="">全部状态</option>
              <option value="success" ${state.adminLogsStatus === "success" ? "selected" : ""}>成功</option>
              <option value="failed" ${state.adminLogsStatus === "failed" ? "selected" : ""}>失败</option>
            </select>
            <button class="btn-normal" onclick="adminFilterLogs()">筛选</button>
          </div>
        </header>
        <div class="table-wrap">
          <table>
            <thead><tr><th scope="col">时间</th><th scope="col">用户</th><th scope="col">大V</th><th scope="col">渠道</th><th scope="col">状态</th><th scope="col">错误</th></tr></thead>
            <tbody>${logs.map((l) => `
              <tr>
                <td>${escapeHtml(fmtDbTime(l.created_at))}</td>
                <td>${escapeHtml(l.user_name || "全局")}</td>
                <td>${escapeHtml(l.kol_name)}</td>
                <td>${l.channel}</td>
                <td class="${l.status === "success" ? "status-ok" : "status-fail"}">${escapeHtml(l.status)}</td>
                <td>${escapeHtml(l.error || "")}</td>
              </tr>`).join("")}</tbody>
          </table>
        </div>
      </section>`;
  }
  async function loadAdminErrorLogs() {
    try {
      const params = new URLSearchParams({ limit: "200" });
      const levelEl = $("#errlog-level");
      const level = levelEl ? levelEl.value : "";
      if (level) params.set("level", level);
      const data = await api(`/api/admin/error-logs?${params.toString()}`);
      const rows = data.logs || [];
      const body = $("#errlog-body");
      if (!body) return;
      body.innerHTML = rows.length
        ? rows.map((r) => `
            <tr>
              <td>${escapeHtml(fmtDbTime(r.created_at))}</td>
              <td class="${r.level === "ERROR" || r.level === "CRITICAL" ? "status-fail" : ""}">${escapeHtml(r.level)}</td>
              <td class="muted">${escapeHtml(r.logger)}</td>
              <td class="muted">${escapeHtml(r.message)}</td>
            </tr>`).join("")
        : `<tr><td colspan="4" class="muted">暂无错误记录 🎉</td></tr>`;
    } catch (err) {
      const body = $("#errlog-body");
      if (body) body.innerHTML = `<tr><td colspan="4" class="muted">加载失败: ${escapeHtml(err.message)}</td></tr>`;
    }
  }

  let sysLogsTimer = null;

  function stopSysLogsTimer() {
    if (sysLogsTimer) {
      clearInterval(sysLogsTimer);
      sysLogsTimer = null;
    }
  }

  async function loadAdminSysLogsPanel() {
    try {
      const params = new URLSearchParams({ limit: "500" });
      const levelEl = $("#syslog-level");
      const qEl = $("#syslog-q");
      const level = levelEl ? levelEl.value : "";
      const q = qEl ? qEl.value.trim() : "";
      if (level) params.set("level", level);
      if (q) params.set("q", q);
      const data = await api(`/api/admin/system-logs?${params.toString()}`);
      const lines = data.lines || [];
      const el = $("#syslog-pre");
      if (el) el.textContent = lines.join("\n") || "（没有匹配的日志）";
    } catch (err) {
      const el = $("#syslog-pre");
      if (el) el.textContent = "加载失败: " + err.message;
    }
  }

  async function adminFilterLogs() {
    const params = new URLSearchParams({ limit: "100" });
    const userId = $("#ad-logs-user").value;
    const channel = $("#ad-logs-channel").value;
    const status = $("#ad-logs-status").value;
    if (userId) params.set("user_id", userId);
    if (channel) params.set("channel", channel);
    if (status) params.set("status", status);
    state.adminLogsFilter = `&${params.toString()}`;
    state.adminLogsUserId = userId;
    state.adminLogsChannel = channel;
    state.adminLogsStatus = status;
    loadAdminLogs();
  }

  async function loadAdminAudit() {
    const logs = await api("/api/admin/logs?limit=100");
    if (!routeStillActive(currentAdminSeq())) return;
    $("#admin-body").innerHTML = `
      <section class="section-panel">
        <header class="section-head">
          <div>
            <h2 class="section-title">系统日志</h2>
            <p class="section-meta">内存环形缓冲的最近 500 条日志，每 5 秒自动刷新；更完整历史见 docker logs（LOG_LEVEL=DEBUG 可开启更详细日志）。</p>
          </div>
          <div class="toolbar" style="margin-top:12px">
            <select id="syslog-level" class="form-control" style="width:auto" onchange="loadAdminSysLogsPanel()">
              <option value="">全部级别</option>
              <option value="ERROR">ERROR+</option>
              <option value="WARNING">WARNING+</option>
              <option value="INFO">INFO+</option>
              <option value="DEBUG">DEBUG（仅LOG_LEVEL=DEBUG时产生）</option>
            </select>
            <input id="syslog-q" class="form-control" style="width:220px" placeholder="关键词过滤（如 推送失败 / 大V名）" onkeydown="if(event.key==='Enter')loadAdminSysLogsPanel()">
            <button class="btn-normal" onclick="loadAdminSysLogsPanel()">刷新</button>
          </div>
        </header>
        <pre class="syslog" id="syslog-pre">加载中…</pre>
      </section>
      <section class="section-panel">
        <header class="section-head">
          <div>
            <h2 class="section-title">错误记录</h2>
            <p class="section-meta">WARNING 及以上日志持久化存储（跨重启保留最近 5000 条），即使环形缓冲滚动或重启后仍可查。</p>
          </div>
          <div class="toolbar" style="margin-top:12px">
            <select id="errlog-level" class="form-control" style="width:auto" onchange="loadAdminErrorLogs()">
              <option value="">全部级别</option>
              <option value="ERROR">ERROR+</option>
              <option value="WARNING">WARNING+</option>
            </select>
            <button class="btn-normal" onclick="loadAdminErrorLogs()">刷新</button>
          </div>
        </header>
        <div class="table-wrap">
          <table>
            <thead><tr><th scope="col">时间</th><th scope="col">级别</th><th scope="col">来源</th><th scope="col">内容</th></tr></thead>
            <tbody id="errlog-body"><tr><td colspan="4" class="muted">加载中…</td></tr></tbody>
          </table>
        </div>
      </section>
      <section class="section-panel">
        <header class="section-head"><div><h2 class="section-title">操作日志</h2>
        <p class="section-meta">管理员关键操作、以及用户知识库超额（操作 ima_quota，目标是用户名）。</p></div></header>
        <div class="table-wrap">
          <table>
            <thead><tr><th scope="col">时间</th><th scope="col">管理员</th><th scope="col">操作</th><th scope="col">目标</th><th scope="col">详情</th></tr></thead>
            <tbody>${logs.length === 0 ? `<tr><td colspan="5" class="muted">暂无记录</td></tr>` : logs.map((l) => `
              <tr>
                <td>${escapeHtml(fmtDbTime(l.created_at))}</td>
                <td>${escapeHtml(l.username || "")}</td>
                <td>${escapeHtml(l.action)}</td>
                <td>${escapeHtml(l.target)}</td>
                <td class="muted">${escapeHtml(l.detail)}</td>
              </tr>`).join("")}</tbody>
          </table>
        </div>
      </section>`;
    stopSysLogsTimer();
    sysLogsTimer = setInterval(loadAdminSysLogsPanel, 5000);
    loadAdminSysLogsPanel();
    loadAdminErrorLogs();
  }

  return {
    loadAdminStats,
    loadAdminDashboard,
    loadAdminAudit,
    loadAdminLogs,
    loadAdminErrorLogs,
    loadAdminSysLogsPanel,
    adminFilterLogs,
    stopSysLogsTimer,
    renderStatsData,
    openAdminKolFromHealth,
    setPlazaSourceMode,
  };
}
