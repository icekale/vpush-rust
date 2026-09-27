export function createPushSettingsView(dependencies) {
  const {
    $,
    state,
    api,
    currentRouteSeq,
    routeStillActive,
    imaMountState,
    flash,
    escapeHtml,
    emptyState,
    go,
    isRoute,
    setPageTitle,
    CHANNEL_ICONS,
    PLATFORM_LABELS,
    SEARCH_ICON,
    avatarHtml,
    feishuPersonalView,
    feishuPersonalHtml,
    pushChannelsHtml,
    webPushSupported,
    toggleDnd,
    CHEVRON_RIGHT_ICON,
  } = dependencies;

  // ---------- 推送设置 ----------
  let settingsPollTimer = null;
  let settingsPollSeq = 0;
  let _pushStatusHtml = "";
  const SETTINGS_TABS = ["push", "bind", "llm", "account"];
  let _kolImageSubscriptions = [];
  const _kolImagePendingIds = new Set();
  let _kolImageLoadGeneration = 0;
  let _kolImageDataRevision = 0;
  let _kolImageReloadNeeded = false;

  function stopSettingsPoll() {
    settingsPollSeq += 1;
    if (settingsPollTimer) {
      clearInterval(settingsPollTimer);
      settingsPollTimer = null;
    }
  }

  function startSettingsPoll() {
    stopSettingsPoll();
    const pollSeq = settingsPollSeq;
    settingsPollTimer = setInterval(() => refreshSettingsStatus(pollSeq), 10000);
  }

  function pendingBindActive() {
    return !!(feishuPersonalView.pendingBind && Date.now() < feishuPersonalView.pendingBind.expiresAt);
  }

  function settingsTargetBound(user) {
    return !!(user && (user.telegram_chat_id || feishuChannelBound(user)));
  }

  async function reloadSettings(routeSeq) {
    if (!routeStillActive(routeSeq)) return;
    stopSettingsPoll();
    await renderSettings(routeSeq);
  }

  function feishuChannelBound(user) {
    return !!(user.feishu_open_id || user.feishu_chat_id || user.feishu_personal?.status === "active");
  }

  function channelStatusHtml(user) {
    const tg = user.telegram_chat_id;
    const tgCustom = user.custom_telegram_bot;
    const fsOpen = user.feishu_open_id;
    const fsChat = user.feishu_chat_id;
    const fsPersonal = user.feishu_personal || {};
    const fsPersonalActive = fsPersonal.status === "active";
    const wc = user.wecom_webhook;
    const bk = user.bark_key;
    const wp = !!user.webpush_bound;
    const wpCount = user.webpush_count || 0;
    const wpOk = webPushSupported();
    const fsOk = !!(fsOpen && fsChat);
    const statusPill = (cls, text) => `<span class="channel-status ${cls}"><i class="dot"></i>${text}</span>`;
    return `
      <div class="channel-grid">
        <div class="channel-card" data-channel="telegram">
          <div class="channel-head">
            <span class="channel-title">${CHANNEL_ICONS.telegram}<b>Telegram${tgCustom ? ' <span class="tag">自建</span>' : ""}</b></span>
            ${statusPill(tg ? "status-ok" : "status-fail", tg ? "已绑定" : "未绑定")}
          </div>
          <p class="muted channel-desc">${tg ? (tgCustom ? "使用你自己的机器人推送" : "官方机器人推送已启用") : "按下方步骤操作"}</p>
          <div class="channel-actions">
            ${tg ? "" : `<div id="bind-result-telegram"></div>`}
            ${tg
              ? `<button class="channel-btn secondary" onclick="unbindChannel('${tgCustom ? "telegram_bot_token" : "telegram_chat_id"}')">解绑</button>`
              : `<button class="channel-btn primary" onclick="openBindGuide('custom-bots-bind')">去绑定</button>`}
          </div>
        </div>
        <div class="channel-card" data-channel="feishu">
          <div class="channel-head">
            <span class="channel-title">${CHANNEL_ICONS.feishu}<b>飞书${fsPersonalActive ? ' <span class="tag">个人</span>' : (fsOk ? ' <span class="tag">共享</span>' : "")}</b></span>
            ${fsPersonalActive ? statusPill("status-ok", "已绑定")
              : fsOk ? statusPill("status-ok", "已绑定")
              : fsPersonal?.status === "degraded" || fsPersonal?.status === "disabled"
                ? statusPill("status-warn", fsPersonal.status === "degraded" ? "已降级" : "已停用")
              : fsOpen ? statusPill("status-warn", "未完成")
              : statusPill("status-fail", "未绑定")}
          </div>
          <p class="muted channel-desc">
            ${fsPersonalActive ? `个人机器人推送已启用（免共享限频）${fsPersonal.app_id_masked ? " · " + escapeHtml(fsPersonal.app_id_masked) : ""}`
              : fsOk ? "共享机器人推送（不推荐，受限频影响）；建议升级个人机器人"
              : (fsOpen ? "已关联账号，请先在飞书私聊机器人发一条消息"
              : "推荐个人机器人：扫码自动创建，免共享限频")}
          </p>
          <div class="channel-actions">
            ${fsOpen || fsPersonalActive ? "" : `<div id="bind-result-feishu"></div>`}
            ${fsPersonalActive
              ? `<button class="channel-btn secondary" onclick="unbindChannel('feishu_personal')">解绑</button>`
              : fsOpen
                ? `<button class="channel-btn secondary" onclick="unbindChannel('feishu')">解绑</button>`
                : `<button class="channel-btn primary" onclick="openBindGuide('custom-bots-bind')">去绑定</button>`}
          </div>
        </div>
        <div class="channel-card" data-channel="wecom">
          <div class="channel-head">
            <span class="channel-title">${CHANNEL_ICONS.wecom}<b>企业微信</b></span>
            ${wc ? statusPill("status-ok", "已绑定") : statusPill("status-fail", "未绑定")}
          </div>
          <p class="muted channel-desc">${wc ? "群机器人推送已启用" : "在企业微信群添加群机器人，把 webhook 粘贴到下方输入框即可"}</p>
          <div class="channel-actions">
            ${wc
              ? `<button class="channel-btn secondary" onclick="unbindChannel('wecom')">解绑</button>`
              : `<button class="channel-btn primary" onclick="openBindGuide('wecom-bind')">去绑定</button>`}
          </div>
        </div>
        <div class="channel-card" data-channel="bark">
          <div class="channel-head">
            <span class="channel-title">${CHANNEL_ICONS.bark}<b>Bark</b></span>
            ${bk ? statusPill("status-ok", "已绑定") : statusPill("status-fail", "未绑定")}
          </div>
          <p class="muted channel-desc">${bk ? "iOS 推送已启用" : "iPhone 装 Bark App，把推送 key 粘贴到下方输入框即可"}</p>
          <div class="channel-actions">
            ${bk
              ? `<button class="channel-btn secondary" onclick="unbindChannel('bark')">解绑</button>`
              : `<button class="channel-btn primary" onclick="openBindGuide('bark-bind')">去绑定</button>`}
          </div>
        </div>
        <div class="channel-card" data-channel="webpush">
          <div class="channel-head">
            <span class="channel-title">${CHANNEL_ICONS.webpush}<b>浏览器通知</b></span>
            ${!wpOk
              ? statusPill("status-fail", "当前环境不可用")
              : wp ? statusPill("status-ok", wpCount > 1 ? `已开启 · ${wpCount} 台设备` : "已开启")
              : statusPill("status-fail", "未开启")}
          </div>
          <p class="muted channel-desc">${!wpOk
            ? "请用 Chrome 或 Edge，并打开 HTTPS（本机 localhost 也可以）"
            : wp
              ? "Chrome / Edge 系统通知已启用，关掉标签页也能收到"
              : "在本页一键开启；Chrome / Edge 关掉标签页也能弹系统通知（需 HTTPS）"}</p>
          <div class="channel-actions">
            ${!wpOk
              ? `<button type="button" class="channel-btn primary" disabled>开启</button>`
              : wp
                ? `<button type="button" class="channel-btn primary" onclick="enableWebPush()">在此浏览器开启</button>
                   <button type="button" class="channel-btn secondary" onclick="disableWebPush()">关闭</button>`
                : `<button type="button" class="channel-btn primary" onclick="enableWebPush()">开启</button>`}
          </div>
        </div>
      </div>`;
  }

  function paintPushStatus(user) {
    const el = $("#push-status");
    if (!el) return;
    const html = channelStatusHtml(user);
    if (html === _pushStatusHtml) return;
    const active = document.activeElement;
    let restore = null;
    if (active && el.contains(active)) {
      const card = active.closest("[data-channel]");
      restore = {
        channel: card && card.dataset.channel,
        text: (active.textContent || "").trim(),
        tag: active.tagName,
      };
    }
    el.innerHTML = html;
    _pushStatusHtml = html;
    if (!restore) return;
    const card = restore.channel ? el.querySelector(`[data-channel="${restore.channel}"]`) : el;
    if (!card) return;
    const nodes = [...card.querySelectorAll(restore.tag)];
    const match = nodes.find((n) => (n.textContent || "").trim() === restore.text) || nodes[0];
    if (match && typeof match.focus === "function") match.focus();
  }

  async function refreshSettingsStatus(pollSeq = settingsPollSeq) {
    const seq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    try {
      const user = await api("/api/me");
      if (pollSeq !== settingsPollSeq || !routeStillActive(seq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      state.user = user;
      const el = $("#push-status");
      if (!el) {
        stopSettingsPoll();
        return;
      }
      paintPushStatus(user);
      // 状态轮询会重绘卡片，把未过期的绑定码重新显示，避免刚生成的码被刷掉
      if (feishuPersonalView.pendingBind && Date.now() < feishuPersonalView.pendingBind.expiresAt) {
        renderBindResult(feishuPersonalView.pendingBind.channel, feishuPersonalView.pendingBind.code);
      } else if (feishuPersonalView.pendingBind) {
        feishuPersonalView.pendingBind = null;
      }
      if (!pendingBindActive() || settingsTargetBound(user)) stopSettingsPoll();
    } catch {
      /* 轮询失败忽略 */
    }
  }

  async function renderSettings(seq) {
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    setPageTitle("个人设置");
    try {
      const user = await api("/api/me");
      if (!routeStillActive(seq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      state.user = user;
      stopSettingsPoll();
      const guide = state.user.push_guide || {};
      const tgBot = guide.telegram_bot_username || "";
      const fsBot = guide.feishu_bot_name || "";
      const tgTarget = tgBot
        ? `<a href="https://t.me/${encodeURIComponent(tgBot)}" target="_blank" rel="noopener">@${escapeHtml(tgBot)}</a>`
        : "你的机器人";
      const fsTarget = fsBot ? `<b>${escapeHtml(fsBot)}</b>` : "你的机器人应用名";
      $("#main").innerHTML = `
        <div class="settings-tabs" role="tablist" aria-label="设置分页">
          <button type="button" class="settings-tab active" role="tab" id="tab-push" aria-selected="true" aria-controls="st-push" data-tab="push" onclick="switchSettingsTab('push')">推送设置</button>
          <button type="button" class="settings-tab" role="tab" id="tab-bind" aria-selected="false" aria-controls="st-bind" data-tab="bind" onclick="switchSettingsTab('bind')">渠道绑定</button>
          <button type="button" class="settings-tab" role="tab" id="tab-llm" aria-selected="false" aria-controls="st-llm" data-tab="llm" onclick="switchSettingsTab('llm')">AI 网关</button>
          <button type="button" class="settings-tab" role="tab" id="tab-account" aria-selected="false" aria-controls="st-account" data-tab="account" onclick="switchSettingsTab('account')">账号设置</button>
        </div>
        <div id="st-push" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-push">
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">推送开关</h2>
              <p class="section-meta">总开关与每日精选摘要。</p>
            </div>
          </header>
          <div class="form-row">
            <label for="set-notify">新帖推送开关</label>
            <select id="set-notify" class="form-control" onchange="saveNotify()">
              <option value="1" ${state.user.notify_enabled ? "selected" : ""}>开启</option>
              <option value="0" ${!state.user.notify_enabled ? "selected" : ""}>关闭</option>
            </select>
          </div>
          <p class="muted">关闭后不会向任何渠道推送新帖，订阅关系保留。</p>
          <div class="form-row" style="margin-top:16px">
            <label for="set-daily">每日精选摘要</label>
            <select id="set-daily" class="form-control" onchange="saveDailyReport()">
              <option value="1" ${state.user.daily_report_enabled ? "selected" : ""}>开启（每天 20:00 推一次 AI 每日精选）</option>
              <option value="0" ${!state.user.daily_report_enabled ? "selected" : ""}>关闭</option>
            </select>
          </div>
          <p class="muted">开启后，每天 20:00 把你订阅大V当天的新动态汇总成一条推送。</p>
          <div class="form-row" style="margin-top:16px">
            <label for="set-x-translate">X / Truth 帖文</label>
            <select id="set-x-translate" class="form-control" onchange="saveTranslateTwitter()">
              <option value="1" ${state.user.translate_twitter !== false ? "selected" : ""}>翻译成中文</option>
              <option value="0" ${state.user.translate_twitter === false ? "selected" : ""}>保留原文</option>
            </select>
          </div>
          <p class="muted">只影响你的时间线和推送。管理员需开启「X / Truth 内容自动翻译」后，新抓的帖才会同时留下原文。</p>
        </section>
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">免打扰时段</h2>
              <p class="section-meta">时段内不推送新帖（支持跨午夜），结束后一次性补一条汇总；系统告警不受影响。特别关注可设为穿透免打扰。</p>
            </div>
          </header>
          <div class="dnd-form">
            <label class="switch">
              <input id="dnd-enabled" type="checkbox" ${state.user.dnd_start ? "checked" : ""} onchange="toggleDnd()">
              <span class="track"></span>
              <span>开启免打扰</span>
            </label>
            <div class="dnd-range-field" id="dnd-range-field">
              <span class="dnd-range-label">免打扰时段</span>
              <div class="dnd-range">
                <input id="dnd-start" type="time" class="form-control" value="${escapeHtml(state.user.dnd_start || "23:00")}">
                <span class="dnd-sep">至</span>
                <input id="dnd-end" type="time" class="form-control" value="${escapeHtml(state.user.dnd_end || "07:00")}">
              </div>
            </div>
            <label class="switch">
              <input id="dnd-fav" type="checkbox" ${state.user.dnd_allow_favorite ? "checked" : ""}>
              <span class="track"></span>
              <span>特别关注可穿透免打扰</span>
            </label>
            <div class="dnd-actions">
              <button class="btn-normal" onclick="saveDnd()">保存</button>
            </div>
          </div>
        </section>
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">关键词提醒</h2>
              <p class="section-meta">每行一个，最多 20 个，每个不超过 50 字。动态命中会加标记并实时推送（穿透免打扰）；研报命中在每日更新后合并成一条，不穿透免打扰。</p>
            </div>
          </header>
          <div class="form-row">
            <label for="set-keywords">关键词（每行一个）</label>
            <textarea id="set-keywords" class="form-control" rows="4"
              placeholder="ETF&#10;降息&#10;中概股">${escapeHtml((state.user.keywords || []).join("\n"))}</textarea>
          </div>
          <label class="switch kw-report-switch">
            <input id="set-kw-reports" type="checkbox" ${state.user.keywords_match_reports ? "checked" : ""} onchange="saveKeywordsMatchReports()">
            <span class="track"></span>
            <span>匹配研报中心</span>
          </label>
          <label class="switch kw-report-switch">
            <input id="set-kw-news" type="checkbox" ${state.user.keywords_match_news ? "checked" : ""} onchange="saveKeywordsMatchNews()">
            <span class="track"></span>
            <span>匹配财经资讯</span>
          </label>
          <div class="toolbar" style="margin-top:10px">
            <button class="btn-normal" onclick="saveKeywords()">保存关键词</button>
          </div>
          <p class="muted">动态命中即实时送达。开启「匹配研报中心」后，每日研报入库结束会把命中篇目合成一条推送；需要管理员已授权对应研报库。开启「匹配财经资讯」后，新入库的财经资讯命中关键词会合并推送（约每分钟一批）。</p>
        </section>
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">动态图片</h2>
              <p class="section-meta">关闭某位大V的图片显示后，该大V的动态图片会从网页和推送中隐藏；头像仍会显示。仅影响当前账号。</p>
            </div>
          </header>
          <div id="kol-images-settings" class="muted kol-images-state" role="status">正在加载已订阅大V…</div>
        </section>
        </div>
        <div id="st-bind" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-bind">
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">推送渠道</h2>
              <p class="section-meta">绑定状态每 10 秒自动刷新；绑定了多个渠道时，可在下方勾选要接收推送的渠道（不选则全部推送）。</p>
            </div>
          </header>
          <div id="push-status">${channelStatusHtml(state.user)}</div>
          <div class="channel-picks" id="push-channels-box" style="margin-top:18px;padding-top:18px;border-top:var(--border-default)">${pushChannelsHtml(state.user)}</div>
          ${(state.user.telegram_chat_id || feishuChannelBound(state.user) || state.user.wecom_webhook || state.user.bark_key || state.user.webpush_bound)
            ? `<div class="toolbar" style="margin-top:14px">
                 <button class="btn-normal" onclick="savePushChannels()">保存推送通道</button>
               </div>` : ""}
        </section>
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">渠道绑定</h2>
              <p class="section-meta">按序绑定想用的推送渠道，每个渠道的步骤可展开；绑定状态在「推送渠道状态」卡片查看。</p>
            </div>
          </header>
          <div class="channel-bind-block" id="custom-bots-bind">
            <h4 class="section-title">自建机器人（推荐，免共享限频）</h4>
            <p class="section-meta">共享机器人所有用户共用一个应用配额；自建/个人机器人是<b>属于你自己的机器人应用</b>，推送配额独立、不受共享应用限制。Telegram 自建约 1 分钟，飞书个人扫码自动创建。</p>
            <div class="channel-bind-block" style="padding-top:8px">
              <h4 class="section-title">Telegram 自建机器人</h4>
              <ol style="padding-left:20px;line-height:2">
                <li>打开 Telegram 搜索 <b>@BotFather</b>，发 <code>/newbot</code> 创建机器人，拿到 token</li>
                <li>给你的新机器人发任意消息（如 <code>/start</code>）</li>
                <li>把 token 粘贴到下方点「保存」，系统自动识别你的会话，无需手动填 ID</li>
              </ol>
              <div class="row" style="gap:8px;margin-top:10px">
                <input id="set-custom-tg" class="form-control" style="flex:1;min-width:220px" type="password" placeholder="123456:ABC-DEF...">
                <button class="btn-normal" onclick="saveCustomTgBot()">保存</button>
              </div>
            </div>
            <div class="channel-bind-block" style="padding-top:8px">
              <h4 class="section-title">飞书个人机器人（扫码自动创建）</h4>
              <div id="fs-personal-block">${feishuPersonalHtml(state.user.feishu_personal)}</div>
            </div>
          </div>
          <div class="channel-bind-block" id="telegram-bind">
            <h4 class="section-title">1. Telegram 机器人</h4>
            ${bindGuideHtml(!!state.user.telegram_chat_id, `
          <ol style="padding-left:20px;line-height:2">
            <li>打开 Telegram，搜索并进入 ${tgTarget}（找不到就点上方链接）。</li>
            <li>点击「开始」或发送任意消息（如 <code>/start</code>），系统自动记录你的会话。</li>
            <li>回到本页，状态几秒内自动变成「已绑定 ✅」。</li>
            <li>发 <code>/list</code> 可查看大V目录，<code>/sub 大VID</code> 直接订阅。</li>
          </ol>`)}
          </div>
          <div class="channel-bind-block" id="feishu-bind">
            <h4 class="section-title">2. 飞书机器人 · 共享备选</h4>
            <p class="section-meta">不推荐：所有用户共用一个应用，推送配额共享，人多可能被限频。仅作为没有个人机器人时的临时备选，建议优先用上方「自建机器人」里的个人机器人。</p>
            ${bindGuideHtml(!!(state.user.feishu_open_id && state.user.feishu_chat_id), `
          <ol style="padding-left:20px;line-height:2">
            <li>打开飞书 App，点顶部「搜索」，搜索 ${fsTarget} 并进入。</li>
            <li>关键：请在该机器人的<b>「私聊」会话</b>里发任意消息（如 <code>/start</code>）——群聊不会推送新帖，这一步只是建立会话。</li>
            <li>回到本页，在下方「与网页/小程序账号同步」里点「生成绑定码」，把 <code>/bind 8位码</code> 发给机器人。</li>
            <li>发送后本页状态会变成「已绑定 ✅」，网页订阅与飞书推送自动同步。</li>
            <li>发 <code>/list</code> 可查看大V目录，点卡片上的按钮即可订阅。</li>
          </ol>`)}
          </div>
          <div class="channel-bind-block" id="wecom-bind">
            <h4 class="section-title">3. 企业微信群机器人</h4>
            <p class="section-meta">无需申请应用；在企业微信任意群里添加「群机器人」即可，推送会发到这个群。</p>
            ${bindGuideHtml(!!state.user.wecom_webhook, `
          <ol style="padding-left:20px;line-height:2">
            <li>打开企业微信，进入一个群（没有就新建一个，例如「大V推送」）。</li>
            <li>点右上角 <code>...</code> → 「群机器人」→「添加机器人」，按提示创建并起名。</li>
            <li>创建完成后复制 webhook 地址（<code>https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=...</code>）。</li>
            <li>粘贴到下方输入框，点「保存绑定」，状态即变为「已绑定 ✅」。</li>
          </ol>`)}
            <div class="form-row" style="margin-top:14px">
              <label for="set-wecom-webhook">群机器人 webhook 地址</label>
              <div class="row" style="gap:10px;flex-wrap:wrap">
                <input id="set-wecom-webhook" class="form-control" style="flex:1;min-width:280px"
                  type="text" placeholder="https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=..."
                  value="${escapeHtml(state.user.wecom_webhook || "")}">
                <button class="btn-normal" onclick="saveWecomWebhook()">保存绑定</button>
              </div>
            </div>
            <p class="muted">⚠️ webhook 等同群管理权限，请勿泄露给他人；不同用户应使用各自的群机器人。</p>
          </div>
          <div class="channel-bind-block" id="bark-bind">
            <h4 class="section-title">4. Bark（iPhone 推送）</h4>
            <p class="section-meta">iOS 自托管用户神器：Bark App 免登录、免费、推送直达锁屏，无需申请任何开发者资质。</p>
            ${bindGuideHtml(!!state.user.bark_key, `
          <ol style="padding-left:20px;line-height:2">
            <li>iPhone 在 App Store 搜索「Bark」安装，打开后主屏会显示你的推送 key（形如 <code>AaBbCcDdEe...</code>）。</li>
            <li>把这个 key 粘贴到下方输入框，点「保存绑定」即可。</li>
            <li>想用自建 Bark 服务器？直接把服务器里的完整地址（<code>https://bark.example.com/xxx</code>）粘贴进来也行。</li>
          </ol>`)}
            <div class="form-row" style="margin-top:14px">
              <label for="set-bark-key">Bark 推送 key 或完整地址</label>
              <div class="row" style="gap:10px;flex-wrap:wrap">
                <input id="set-bark-key" class="form-control" style="flex:1;min-width:280px"
                  type="text" placeholder="AaBbCcDdEeFf...（Bark App 里的 key）"
                  value="${escapeHtml(state.user.bark_key || "")}">
                <button class="btn-normal" onclick="saveBarkKey()">保存绑定</button>
              </div>
            </div>
            <p class="muted">🔔 key 等同推送权限，请勿泄露；系统告警不依赖此 key（管理员另配系统级 Bark）。</p>
          </div>
          <div class="channel-bind-block" id="webpush-bind">
            <h4 class="section-title">5. 浏览器通知（Chrome / Edge）</h4>
            <p class="section-meta">网页系统通知：在「推送渠道」卡片点「开启」授权即可。关掉标签页也能收到；需 HTTPS。可在多台 Chrome / Edge 分别点「在此浏览器开启」。点「关闭」会关掉该功能（所有已开启的浏览器都不再弹）。</p>
          </div>
        </section>
        </div>
        <div id="st-llm" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-llm">
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">AI 网关</h2>
              <p class="section-meta">填了 Key 才给你做 AI 摘要。不填就普通列表。打标等全站功能仍由站点 LLM 维护。</p>
            </div>
          </header>
          <div class="form-row">
            <label for="set-llm-format">接口</label>
            <select id="set-llm-format" class="form-control">
              <option value="chat" ${state.user.llm_api_format !== "responses" ? "selected" : ""}>Chat Completions（/chat/completions）</option>
              <option value="responses" ${state.user.llm_api_format === "responses" ? "selected" : ""}>Responses（/responses）</option>
            </select>
          </div>
          <div class="form-row">
            <label for="set-llm-base">API 地址（Base URL）</label>
            <input id="set-llm-base" class="form-control" type="text"
              placeholder="https://api.openai.com/v1"
              value="${escapeHtml(state.user.llm_api_base || "")}">
            <p class="muted">OpenAI 兼容的 http(s) Base URL。不填则无法用 AI 摘要。</p>
          </div>
          <div class="form-row">
            <label for="set-llm-key">API Key</label>
            <input id="set-llm-key" class="form-control" type="password"
              placeholder="sk-...（清空 = 不用 AI 摘要）"
              value="${escapeHtml(state.user.llm_api_key || "")}" autocomplete="off">
          </div>
          <div class="form-row">
            <label for="set-llm-model">模型</label>
            <select id="set-llm-model-select" class="form-control" hidden
              onchange="if(this.value){document.getElementById('set-llm-model').value=this.value}">
              <option value="">手填模型名</option>
            </select>
            <div class="llm-model-row">
              <input id="set-llm-model" class="form-control" type="text"
                placeholder="可手填，或先拉取再从上方选择"
                value="${escapeHtml(state.user.llm_model || "")}">
              <button type="button" class="btn-ghost" onclick="loadLlmModels()">拉取模型列表</button>
            </div>
            <p class="muted">列表来自该接口的 <code>/models</code>。点拉取后用系统下拉选择；没有列表的网关仍可手填。</p>
          </div>
          <div class="toolbar">
            <button class="btn-normal" onclick="saveLlm()">保存</button>
            <button type="button" class="btn-ghost" onclick="testLlm()">测试</button>
          </div>
          <p class="muted">推送摘要固定 Chat Completions；每日精选用上面选的接口。失败会回退为普通摘要，不影响推送。${state.user.llm_last_status === "ok" ? "上次摘要已用 AI。" : state.user.llm_last_status === "fallback" ? "上次摘要已回退为普通列表。" : ""}</p>
        </section>
        </div>
        <div id="st-account" class="settings-tab-panel" role="tabpanel" aria-labelledby="tab-account">
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">修改密码</h2>
              <p class="section-meta">定期更换密码，保护你的账号安全。</p>
            </div>
          </header>
          <div class="form-row">
            <label for="pw-old">原密码</label>
            <input id="pw-old" class="form-control" type="password" placeholder="输入当前密码" autocomplete="current-password">
          </div>
          <div class="form-row">
            <label for="pw-new">新密码</label>
            <input id="pw-new" class="form-control" type="password" placeholder="至少 10 位" autocomplete="new-password">
          </div>
          <div class="form-row">
            <label for="pw-confirm">确认新密码</label>
            <input id="pw-confirm" class="form-control" type="password" placeholder="再次输入新密码" autocomplete="new-password">
          </div>
          <button class="btn-normal" onclick="savePassword()">修改密码</button>
        </section>
        <section class="section-panel">
          <header class="section-head">
            <div>
              <h2 class="section-title">与网页/小程序账号同步（可选）</h2>
              <p class="section-meta">机器人是独立账号；想让机器人订阅与网页账号合并，用绑定码。</p>
            </div>
          </header>
          <details class="bind-steps">
            <summary>${CHEVRON_RIGHT_ICON}<span>展开查看同步步骤</span></summary>
          <ol style="padding-left:20px;line-height:2">
            <li>点下方「生成绑定码」。</li>
            <li>把 <code>/bind 8位码</code> 发给 Telegram / 飞书机器人（企业微信群机器人是单向 webhook，不支持指令）。</li>
            <li>绑定后机器人账号合并到当前账号，订阅与推送同步，一处订阅处处同步。</li>
          </ol>
          </details>
          <div class="row">
            <button class="btn-ghost" onclick="genBindCode()">生成绑定码</button>
          </div>
          <div id="bind-result" class="muted" style="margin-top:14px"></div>
        </section>
        </div>`;
      _pushStatusHtml = channelStatusHtml(state.user);
      if (pendingBindActive() && !settingsTargetBound(state.user)) startSettingsPoll();
      switchSettingsTab(state.settingsTab || "push"); // 恢复上次所在分栏
      toggleDnd(); // 根据开关初始状态同步时段输入框的禁用/置灰
      loadKolImageSettings(seq);
    } catch (err) {
      if (!routeStillActive(seq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      $("#main").innerHTML = emptyState(err.message);
    }
  }

  function reloadKolImageSettingsIfNeeded() {
    if (!_kolImageReloadNeeded || _kolImagePendingIds.size) return;
    if (!isRoute("settings")) return;
    _kolImageReloadNeeded = false;
    loadKolImageSettings(currentRouteSeq());
  }

  async function loadKolImageSettings(seq) {
    const target = $("#kol-images-settings");
    if (!target) return;
    const loadGeneration = ++_kolImageLoadGeneration;
    const loadRevision = _kolImageDataRevision;
    target.className = "muted kol-images-state";
    target.setAttribute("role", "status");
    target.textContent = "正在加载已订阅大V…";
    try {
      const subscriptions = await api("/api/my/subscriptions");
      if (loadGeneration !== _kolImageLoadGeneration || !routeStillActive(seq)) return;
      if (loadRevision !== _kolImageDataRevision || _kolImagePendingIds.size) {
        _kolImageReloadNeeded = true;
        reloadKolImageSettingsIfNeeded();
        return;
      }
      _kolImageReloadNeeded = false;
      _kolImageSubscriptions = subscriptions;
      renderKolImageSettings();
    } catch (err) {
      if (loadGeneration !== _kolImageLoadGeneration || !routeStillActive(seq)) return;
      if (loadRevision !== _kolImageDataRevision || _kolImagePendingIds.size) {
        _kolImageReloadNeeded = true;
        reloadKolImageSettingsIfNeeded();
        return;
      }
      _kolImageReloadNeeded = false;
      const current = $("#kol-images-settings");
      if (!current) return;
      current.className = "kol-images-local-state";
      current.setAttribute("role", "alert");
      current.innerHTML = `
        <p class="muted">加载失败: ${escapeHtml(err.message)}</p>
        <button type="button" class="btn-ghost" onclick="reloadKolImageSettings()">重试</button>`;
    }
  }

  function renderKolImageSettings() {
    const target = $("#kol-images-settings");
    if (!target) return;
    target.className = "";
    target.removeAttribute("role");
    if (!_kolImageSubscriptions.length) {
      target.innerHTML = emptyState(
        "还没有订阅大V",
        `<div><button type="button" class="btn-normal btn-add" onclick="go('home')">去订阅广场</button></div>`
      );
      return;
    }
    const search = _kolImageSubscriptions.length >= 12
      ? `<div class="search-bar kol-images-search">
           ${SEARCH_ICON}
           <input id="kol-images-search" type="search" aria-label="搜索已订阅大V"
             placeholder="搜索已订阅大V" oninput="filterKolImageSettings()">
         </div>`
      : "";
    target.innerHTML = `${search}<div id="kol-images-list" class="kol-images-list" role="region" aria-label="已订阅大V的动态图片"></div><p id="kol-images-more" class="section-meta" hidden></p>`;
    filterKolImageSettings();
  }

  function filterKolImageSettings() {
    const list = $("#kol-images-list");
    if (!list) return;
    const query = ($("#kol-images-search")?.value || "").trim().toLowerCase();
    const filtered = query
      ? _kolImageSubscriptions.filter((kol) => {
          const platform = PLATFORM_LABELS[kol.platform] || kol.platform || "";
          return [kol.name, kol.external_id, platform]
            .some((value) => String(value || "").toLowerCase().includes(query));
        })
      : _kolImageSubscriptions;
    list.innerHTML = filtered.length
      ? filtered.map(kolImageSettingsRowHtml).join("")
      : emptyState("没有匹配的已订阅大V");
    const more = $("#kol-images-more");
    if (more) {
      const extra = Math.max(0, filtered.length - 5);
      more.hidden = extra === 0;
      more.textContent = extra ? `还有 ${extra} 位` : "";
    }
  }

  function kolImageSettingsRowHtml(kol) {
    const platform = PLATFORM_LABELS[kol.platform] || kol.platform || "";
    return `
      <div class="kol-images-row">
        ${avatarHtml(kol.name, kol.avatar_url)}
        <div class="kol-images-info">
          <span class="kol-images-name" title="${escapeHtml(kol.name)}">${escapeHtml(kol.name)}</span>
          <span class="kol-images-platform">${escapeHtml(platform)}</span>
        </div>
        <label class="switch kol-images-switch">
          <input type="checkbox" ${!kol.hide_images ? "checked" : ""}
            ${_kolImagePendingIds.has(kol.id) ? "disabled" : ""}
            data-kol-id="${kol.id}"
            aria-label="显示${escapeHtml(kol.name)}（${escapeHtml(platform)}）的动态图片"
            onchange="toggleKolImages(${kol.id}, this)">
          <span class="track"></span>
          <span>显示</span>
        </label>
      </div>`;
  }

  async function toggleKolImages(kolId, input) {
    if (!input || input.disabled || _kolImagePendingIds.has(kolId)) return;
    const kol = _kolImageSubscriptions.find((item) => item.id === kolId);
    if (!kol) return;
    const seq = currentRouteSeq();
    const show = input.checked;
    const previousHideImages = kol.hide_images;
    const restoreFocus = document.activeElement === input && input.matches(":focus-visible");
    _kolImageDataRevision += 1;
    _kolImagePendingIds.add(kolId);
    kol.hide_images = !show;
    input.disabled = true;
    try {
      await api(`/api/subscriptions/${kolId}/hide-images`, {
        method: "PUT",
        body: JSON.stringify({ hide_images: !show }),
      });
      if (!routeStillActive(seq)) return;
      flash(`${show ? "已显示" : "已隐藏"}「${kol ? kol.name : "该大V"}」的动态图片`);
    } catch (err) {
      kol.hide_images = previousHideImages;
      if (!routeStillActive(seq)) return;
      input.checked = !previousHideImages;
      flash("保存失败: " + err.message, "error");
    } finally {
      _kolImagePendingIds.delete(kolId);
      _kolImageDataRevision += 1;
      const isCurrentSettings = isRoute("settings");
      if (!routeStillActive(seq) && isCurrentSettings) _kolImageReloadNeeded = true;
      if (_kolImagePendingIds.size === 0 && _kolImageReloadNeeded) {
        reloadKolImageSettingsIfNeeded();
      } else if (routeStillActive(seq)) {
        const mountedInput = document.querySelector(`#kol-images-list input[data-kol-id="${kolId}"]`);
        if (mountedInput) {
          mountedInput.checked = !kol.hide_images;
          mountedInput.disabled = false;
          if (restoreFocus && (document.activeElement === input || document.activeElement === document.body)) {
            mountedInput.focus({ preventScroll: true });
          }
        }
      }
    }
  }

  function switchSettingsTab(name) {
    // 设置页分段导航：推送 / 渠道绑定 / AI 网关 / 账号设置
    if (!SETTINGS_TABS.includes(name)) name = "push";
    state.settingsTab = name;
    document.querySelectorAll(".settings-tab[data-tab]").forEach((b) => {
      if (!SETTINGS_TABS.includes(b.dataset.tab)) return;
      const on = b.dataset.tab === name;
      b.classList.toggle("active", on);
      b.setAttribute("aria-selected", String(on));
    });
    SETTINGS_TABS.forEach((t) => {
      const el = document.getElementById("st-" + t);
      if (!el) return;
      const on = t === name;
      el.style.display = on ? "" : "none";
      el.hidden = !on;
    });
  }

  function bindGuideHtml(bound, stepsHtml) {
    // 渠道绑定步骤折叠：未绑定时默认展开引导，已绑定时收起来（页面不再一屏放不下）
    return `<details class="bind-steps" ${bound ? "" : "open"}>
      <summary>${CHEVRON_RIGHT_ICON}<span>${bound ? "已绑定 ✅ · 展开查看绑定步骤" : "展开查看绑定步骤"}</span></summary>
      ${stepsHtml}
    </details>`;
  }

  return {
    stopSettingsPoll,
    startSettingsPoll,
    reloadSettings,
    feishuChannelBound,
    renderSettings,
    switchSettingsTab,
    filterKolImageSettings,
    toggleKolImages,
    loadKolImageSettings,
  };
}
