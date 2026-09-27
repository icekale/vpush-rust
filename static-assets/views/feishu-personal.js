export function createFeishuPersonalView(dependencies) {
  const {
    $,
    state,
    api,
    currentRouteSeq,
    routeStillActive,
    sessionOwnerStillActive,
    flash,
    escapeHtml,
    imaMountState,
    feishuChannelBound,
    startSettingsPoll,
    reloadSettings,
    switchSettingsTab,
    userKeywordSet,
    isReportWatchableTag,
    KEYWORDS_MAX_COUNT,
    CHANNEL_ICONS,
  } = dependencies;

  // ---------- 飞书个人机器人（扫码注册） ----------
  const fsPersonalState = {
    sessionId: "", bindCommand: "", bindExpiresAt: 0, verificationUri: "", qrUri: "",
    pollTimer: null, countdownTimer: null, owner: null, refreshInFlight: false,
  };

  function fsPersonalOwnerActive(owner) {
    return !!owner && fsPersonalState.owner === owner
      && owner.token === state.token
      && owner.sessionGeneration === imaMountState.sessionGeneration
      && routeStillActive(owner.routeSeq);
  }


  function feishuPersonalHtml(fs) {
    fs = fs || {};
    if (!fs.available) {
      return `<p class="muted">⚠️ 个人机器人功能未启用（服务端未配置 FEISHU_CREDENTIAL_KEY），请使用上方共享机器人。</p>`;
    }
    if (fs.status === "active") {
      return `<div class="row" style="gap:10px;flex-wrap:wrap;align-items:center">
        <span class="channel-status status-ok"><i class="dot"></i>个人机器人已激活${fs.app_id_masked ? " · " + escapeHtml(fs.app_id_masked) : ""}</span>
        <button class="channel-btn secondary" onclick="unbindChannel('feishu_personal')">解绑个人机器人</button>
      </div>
      <p class="muted" style="margin-top:8px">推送将使用你的个人应用发送，配额独立、不受共享应用限制；共享机器人绑定保留，个人机器人失效时自动回退。</p>`;
    }
    if (fs.status === "degraded") {
      return `<div class="row" style="gap:10px;flex-wrap:wrap;align-items:center">
        <span class="channel-status status-warn"><i class="dot"></i>个人机器人已降级（暂用共享推送）</span>
        <button class="channel-btn primary" onclick="startFeishuPersonal()">重新扫码绑定</button>
      </div>`;
    }
    if (fsPersonalState.sessionId) {
      return fsPersonalStateHtml();
    }
    return `<div class="row" style="gap:10px;flex-wrap:wrap;margin-top:8px">
      <button class="btn-normal" onclick="startFeishuPersonal()">扫码创建个人机器人</button>
      <span class="muted">需要飞书扫码；个人应用会创建在你自己的飞书租户里。</span>
    </div>`;
  }

  function fsPersonalStateHtml() {
    const st = fsPersonalState;
    const secs = Math.max(0, Math.ceil((st.bindExpiresAt - Date.now()) / 1000));
    const uri = st.verificationUri || "";
    return `<div id="fs-personal-flow">
      <div style="text-align:center;padding:8px 0 12px">
        ${st.qrUri ? `<img class="qr-frame" src="${st.qrUri}" alt="扫码二维码">` : ""}
        <p class="muted qr-status">用飞书「扫一扫」扫码；或点链接打开：
          <a href="${escapeHtml(uri)}" target="_blank" rel="noopener">${escapeHtml(uri)}</a>
        </p>
      </div>
      ${st.bindCommand ? `
      <p style="line-height:1.9">下一步：打开刚创建的个人机器人<b>私聊窗口</b>，发送：<br>
        <code class="bind-code">${escapeHtml(st.bindCommand)}</code>
        <span id="fs-bind-countdown" class="muted" style="margin-left:8px">${secs}s</span>
      </p>
      <div class="row" style="gap:10px;margin-top:8px;flex-wrap:wrap">
        <button class="btn-normal btn-sm" onclick="refreshFeishuBindCode()">重新生成绑定码</button>
        <button class="btn-ghost btn-sm" onclick="cancelFeishuPersonal()">取消</button>
      </div>` : `
      <p class="muted">⏳ 等待扫码…（扫完码会自动进入下一步）</p>
      <button class="btn-ghost btn-sm" onclick="cancelFeishuPersonal()">取消</button>`}
    </div>`;
  }

  async function startFeishuPersonal() {
    const owner = {
      routeSeq: currentRouteSeq(),
      token: state.token,
      sessionGeneration: imaMountState.sessionGeneration,
    };
    fsPersonalState.owner = owner;
    stopFeishuPersonalPoll(owner);
    try {
      const data = await api("/api/me/feishu-personal/register", { method: "POST" });
      if (!fsPersonalOwnerActive(owner)) return;
      fsPersonalState.sessionId = data.session_id;
      fsPersonalState.bindCommand = "";
      fsPersonalState.verificationUri = data.verification_uri;
      fsPersonalState.qrUri = data.qr_uri || "";
      fsPersonalRender(owner);
      startFeishuPersonalPoll(data.session_id, owner);
    } catch (err) {
      if (fsPersonalOwnerActive(owner)) flash("发起个人机器人注册失败: " + err.message, "error");
    }
  }

  function fsPersonalRender(owner = fsPersonalState.owner) {
    // 局部重绘个人机器人区块（不整页重绘：renderSettings 需要路由序号，轮询里拿不到）
    if (!fsPersonalOwnerActive(owner)) return;
    const el = $("#fs-personal-block");
    if (el) el.innerHTML = feishuPersonalHtml(state.user?.feishu_personal);
  }

  function startFeishuPersonalPoll(sessionId, owner = fsPersonalState.owner) {
    if (!fsPersonalOwnerActive(owner)) return;
    stopFeishuPersonalPoll(owner);
    const timer = setInterval(async () => {
      if (!fsPersonalOwnerActive(owner) || fsPersonalState.pollTimer !== timer) return;
      try {
        const data = await api(`/api/me/feishu-personal/register/${sessionId}`);
        if (!fsPersonalOwnerActive(owner) || fsPersonalState.pollTimer !== timer) return;
        fsPersonalState.verificationUri = data.verification_uri;
        fsPersonalState.qrUri = data.qr_uri || fsPersonalState.qrUri;
        // 同步个人机器人展示状态（轮询期间 /api/me 不会刷新）
        state.user.feishu_personal = state.user.feishu_personal || {};
        if (data.personal_bot_status) state.user.feishu_personal.status = data.personal_bot_status;
        if (data.status === "awaiting_bind") {
          if (data.bind_command) {
            const expiresAt = (data.bind_code_expires_at || 0) * 1000;
            const changed = fsPersonalState.bindCommand !== data.bind_command
              || fsPersonalState.bindExpiresAt !== expiresAt;
            fsPersonalState.bindCommand = data.bind_command;
            fsPersonalState.bindExpiresAt = expiresAt;
            if (changed) {
              fsPersonalRender(owner);
              startFeishuBindCountdown(owner);
            }
          } else {
            refreshFeishuBindCode();
          }
        } else if (data.status === "active") {
          stopFeishuPersonalPoll(owner);
          fsPersonalState.sessionId = "";
          fsPersonalRender(owner);
          flash("个人机器人已绑定");
        } else if (["expired", "cancelled", "degraded"].includes(data.status)) {
          stopFeishuPersonalPoll(owner);
          fsPersonalState.sessionId = "";
          fsPersonalRender(owner);
          if (data.status === "degraded") flash("个人机器人绑定失败：" + (data.last_error || "未知错误"), "error");
        } else {
          // pending / credentials_created：局部刷新等待扫码区域
          fsPersonalRender(owner);
        }
      } catch (err) {
        // 轮询失败静默，下轮再试；会话不存在则结束
        if (fsPersonalOwnerActive(owner) && fsPersonalState.pollTimer === timer
          && String(err.message).includes("404")) {
          stopFeishuPersonalPoll(owner);
          fsPersonalState.sessionId = "";
          fsPersonalRender(owner);
        }
      }
    }, 1000);  // 绑定轮询：1s 一次，扫码/绑定完成及时反映（状态接口很轻量）
    fsPersonalState.pollTimer = timer;
  }

  function stopFeishuPersonalPoll(owner = null) {
    if (owner && fsPersonalState.owner !== owner) return;
    if (fsPersonalState.pollTimer) {
      clearInterval(fsPersonalState.pollTimer);
      fsPersonalState.pollTimer = null;
    }
    if (fsPersonalState.countdownTimer) {
      clearInterval(fsPersonalState.countdownTimer);
      fsPersonalState.countdownTimer = null;
    }
  }

  function startFeishuBindCountdown(owner = fsPersonalState.owner) {
    if (!fsPersonalOwnerActive(owner)) return;
    if (fsPersonalState.countdownTimer) clearInterval(fsPersonalState.countdownTimer);
    const timer = setInterval(() => {
      if (!fsPersonalOwnerActive(owner) || fsPersonalState.countdownTimer !== timer) return;
      const secs = Math.max(0, Math.ceil((fsPersonalState.bindExpiresAt - Date.now()) / 1000));
      const el = $("#fs-bind-countdown");
      if (el) el.textContent = `${secs}s`;
      if (secs <= 0) {
        clearInterval(timer);
        fsPersonalState.countdownTimer = null;
        if (fsPersonalState.sessionId) refreshFeishuBindCode();
      }
    }, 1000);
    fsPersonalState.countdownTimer = timer;
  }

  async function refreshFeishuBindCode() {
    const owner = fsPersonalState.owner;
    const sessionId = fsPersonalState.sessionId;
    if (!sessionId || !fsPersonalOwnerActive(owner) || fsPersonalState.refreshInFlight) return;
    fsPersonalState.refreshInFlight = true;
    try {
      const data = await api(`/api/me/feishu-personal/register/${sessionId}/refresh-code`, { method: "POST" });
      if (!fsPersonalOwnerActive(owner) || fsPersonalState.sessionId !== sessionId) return;
      fsPersonalState.bindCommand = data.bind_command;
      fsPersonalState.bindExpiresAt = (data.bind_code_expires_at || 0) * 1000;
      fsPersonalRender(owner);
      startFeishuBindCountdown(owner);
    } catch (err) {
      if (fsPersonalOwnerActive(owner)) flash(err.message, "error");
    } finally {
      fsPersonalState.refreshInFlight = false;
    }
  }

  async function cancelFeishuPersonal() {
    const owner = fsPersonalState.owner;
    const sessionId = fsPersonalState.sessionId;
    if (!sessionId || !fsPersonalOwnerActive(owner)) return;
    try {
      await api(`/api/me/feishu-personal/register/${sessionId}/cancel`, { method: "POST" });
    } catch { /* 忽略 */ }
    if (!fsPersonalOwnerActive(owner) || fsPersonalState.sessionId !== sessionId) return;
    stopFeishuPersonalPoll(owner);
    fsPersonalState.sessionId = "";
    fsPersonalRender(owner);
  }

  function openBindGuide(sectionId) {
    // 渠道绑定块在独立「渠道绑定」分栏里，先切过去再滚动并展开步骤
    if (sectionId.endsWith("-bind")) {
      switchSettingsTab("bind");
    }
    const sec = document.getElementById(sectionId);
    if (!sec) return;
    sec.scrollIntoView({ behavior: "smooth", block: "start" });
    const details = sec.querySelector("details.bind-steps");
    if (details) details.open = true;
  }

  function pushChannelsHtml(user) {
    const opts = [];
    if (user.telegram_chat_id) opts.push(["telegram", "Telegram"]);
    if (feishuChannelBound(user)) opts.push(["feishu", "飞书"]);
    if (user.wecom_webhook) opts.push(["wecom", "企业微信"]);
    if (user.bark_key) opts.push(["bark", "Bark"]);
    if (user.webpush_bound) opts.push(["webpush", "浏览器通知"]);
    if (!opts.length) return `<p class="muted">还没有绑定推送渠道，先完成上方任一渠道绑定后即可选择。</p>`;
    const selected = (user.push_channels || "").split(",").map((s) => s.trim()).filter(Boolean);
    const isChecked = (ch) => selected.length === 0 || selected.includes(ch);
    return opts.map(([ch, label]) => `
      <label class="channel-pick ${isChecked(ch) ? "selected" : ""}" data-channel="${ch}" title="${escapeHtml(label)}">
        <input type="checkbox" value="${ch}" ${isChecked(ch) ? "checked" : ""}
          onchange="this.closest('.channel-pick').classList.toggle('selected', this.checked)">
        <span class="ch-icon-wrap">${CHANNEL_ICONS[ch]}</span>
        <span class="ch-check">✓</span>
      </label>`).join("");
  }

  async function savePushChannels() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const boxes = [...document.querySelectorAll("#push-channels-box input[type=checkbox]")];
    if (!boxes.length) return;
    const channels = boxes.filter((b) => b.checked).map((b) => b.value);
    if (!channels.length) {
      flash("请至少保留一个推送通道；全部不想要可以关闭「新帖推送开关」", "error");
      return;
    }
    try {
      await api("/api/me", { method: "PUT", body: JSON.stringify({ push_channels: channels.join(",") }) });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      state.user.push_channels = channels.join(",");
      flash("已保存");
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function saveNotify() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    try {
      await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ notify_enabled: $("#set-notify").value === "1" }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash("已保存");
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function saveDailyReport() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    try {
      await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ daily_report_enabled: $("#set-daily").value === "1" }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash("已保存");
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function saveTranslateTwitter() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    try {
      const on = $("#set-x-translate").value === "1";
      await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ translate_twitter: on }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      state.user.translate_twitter = on;
      flash("已保存");
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  function toggleDnd() {
    // 免打扰开关与时段输入联动：关闭时时段输入禁用并置灰
    const on = $("#dnd-enabled").checked;
    const field = $("#dnd-range-field");
    if (field) field.classList.toggle("is-off", !on);
    $("#dnd-start").disabled = !on;
    $("#dnd-end").disabled = !on;
  }

  async function saveDnd() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const enabled = $("#dnd-enabled").checked;
    const start = $("#dnd-start").value;
    const end = $("#dnd-end").value;
    const allowFav = $("#dnd-fav").checked;
    if (enabled && (!start || !end || start === end)) {
      flash("请设置不同的开始与结束时间", "error");
      return;
    }
    try {
      await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({
          dnd_start: enabled ? start : "",
          dnd_end: enabled ? end : "",
          dnd_allow_favorite: allowFav,
        }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      state.user.dnd_start = enabled ? start : "";
      state.user.dnd_end = enabled ? end : "";
      state.user.dnd_allow_favorite = allowFav;
      flash("已保存");
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  let pendingBind = null; // { channel, code, expiresAt }——轮询重绘后恢复显示

  function renderBindResult(channel, code) {
    const el = channel === "telegram" ? $("#bind-result-telegram") : $("#bind-result-feishu");
    if (!el) return;
    const guide = state.user.push_guide || {};
    if (channel === "telegram" && guide.telegram_bot_username) {
      const link = `https://t.me/${encodeURIComponent(guide.telegram_bot_username)}?start=bind_${code}`;
      el.innerHTML = `
        <p style="margin:10px 0 6px">点击下方按钮，Telegram 会自动打开机器人并完成绑定：</p>
        <a class="btn-normal" href="${link}" target="_blank" rel="noopener">一键绑定 Telegram</a>
        <p class="muted" style="margin-top:8px">按钮没反应？复制 <b>${escapeHtml(code)}</b> 粘贴给机器人也可以。</p>`;
    } else {
      const label = channel === "telegram" ? "Telegram" : "飞书";
      el.innerHTML = `
        <p style="margin:10px 0 6px">复制绑定码，粘贴发送给${label}机器人（自动识别，无需命令）：</p>
        <b style="font-size:var(--text-icon);letter-spacing:3px;font-family:var(--font-mono);font-variant-numeric:tabular-nums">${escapeHtml(code)}</b>`;
    }
  }

  async function bindChannel(channel) {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    try {
      const data = await api("/api/me/bind-code", { method: "POST" });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      pendingBind = {
        channel,
        code: data.code,
        expiresAt: Date.now() + (data.expires_in_seconds || 600) * 1000,
      };
      renderBindResult(channel, data.code);
      startSettingsPoll();
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function saveCustomTgBot() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const botToken = ($("#set-custom-tg").value || "").trim();
    if (!botToken) {
      flash("请先粘贴你的 bot token", "error");
      return;
    }
    try {
      await api("/api/me", { method: "PUT", body: JSON.stringify({ telegram_bot_token: botToken }) });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash("自建机器人已绑定");
      await reloadSettings(routeSeq);
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function unbindChannel(channel) {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const label = channel === "telegram_chat_id" ? "Telegram"
      : channel === "telegram_bot_token" ? "Telegram（自建机器人）"
      : channel === "feishu_personal" ? "飞书个人机器人"
      : channel === "wecom" ? "企业微信"
      : channel === "bark" ? "Bark" : "飞书";
    if (!confirm(`确认解绑 ${label}？解绑后将不再通过该方式推送（共享机器人绑定不受影响）。`)) return;
    try {
      if (channel === "feishu_personal") {
        stopFeishuPersonalPoll();
        fsPersonalState.sessionId = "";
        await api("/api/me/feishu-personal", { method: "DELETE" });
        if (!routeStillActive(routeSeq) || token !== state.token
          || sessionGeneration !== imaMountState.sessionGeneration) return;
        flash(`已解绑 ${label}`);
        await reloadSettings(routeSeq);
        return;
      }
      const body = channel === "feishu"
        ? { feishu_open_id: "", feishu_chat_id: "" }
        : channel === "wecom"
          ? { wecom_webhook: "" }
          : channel === "bark"
            ? { bark_key: "" }
          : channel === "telegram_bot_token"
            ? { telegram_bot_token: "", telegram_chat_id: "" }
          : { telegram_chat_id: "" };
      await api("/api/me", { method: "PUT", body: JSON.stringify(body) });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(`已解绑 ${label}`);
      await reloadSettings(routeSeq);
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function saveWecomWebhook() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const webhook = ($("#set-wecom-webhook").value || "").trim();
    if (webhook && !/^https:\/\/qyapi\.weixin\.qq\.com\/cgi-bin\/webhook\/send\?key=/.test(webhook)) {
      flash("webhook 地址无效，应为 https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=... 格式", "error");
      return;
    }
    try {
      await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ wecom_webhook: webhook }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(webhook ? "企业微信已绑定" : "企业微信已解绑");
      await reloadSettings(routeSeq);
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function saveBarkKey() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const key = ($("#set-bark-key").value || "").trim();
    try {
      await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ bark_key: key }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(key ? "Bark 已绑定" : "Bark 已解绑");
      await reloadSettings(routeSeq);
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  function webPushSupported() {
    return window.isSecureContext
      && "Notification" in window
      && "serviceWorker" in navigator
      && "PushManager" in window;
  }

  function urlBase64ToUint8Array(b64) {
    const pad = "=".repeat((4 - (b64.length % 4)) % 4);
    const raw = atob((b64 + pad).replace(/-/g, "+").replace(/_/g, "/"));
    return Uint8Array.from([...raw].map((c) => c.charCodeAt(0)));
  }

  async function enableWebPush() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    if (!webPushSupported()) {
      flash("当前环境不支持浏览器通知，请用 Chrome 或 Edge，并打开 HTTPS", "error");
      return;
    }
    const btns = document.querySelectorAll('[data-channel="webpush"] .channel-btn');
    btns.forEach((b) => { b.disabled = true; });
    try {
      const perm = await Notification.requestPermission();
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      if (perm !== "granted") {
        flash("未授予通知权限，请在浏览器设置里允许本站通知", "error");
        return;
      }
      const key = state.user && state.user.vapid_public_key;
      if (!key) {
        flash("服务端未就绪，请刷新后再试", "error");
        return;
      }
      const reg = await navigator.serviceWorker.ready;
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      const sub = await reg.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: urlBase64ToUint8Array(key),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      const json = sub.toJSON();
      await api("/api/me/webpush", {
        method: "POST",
        body: JSON.stringify({ endpoint: json.endpoint, keys: json.keys }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash("浏览器通知已开启");
      await reloadSettings(routeSeq);
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message || "开启失败", "error");
    } finally {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      btns.forEach((b) => { b.disabled = false; });
    }
  }

  async function disableWebPush() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    if (!confirm("关闭后，所有已开启的 Chrome / Edge 都不再弹出通知。")) return;
    try {
      const reg = await navigator.serviceWorker.ready;
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      const sub = await reg.pushManager.getSubscription();
      if (sub) await sub.unsubscribe();
    } catch {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      /* 本地订阅清不掉也不挡服务端关闭 */
    }
    if (!routeStillActive(routeSeq) || token !== state.token
      || sessionGeneration !== imaMountState.sessionGeneration) return;
    try {
      await api("/api/me/webpush", { method: "DELETE" });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash("浏览器通知已关闭");
      await reloadSettings(routeSeq);
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function saveKeywords() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const keywords = ($("#set-keywords").value || "")
      .split(/[\n,]/)
      .map((k) => k.trim())
      .filter(Boolean);
    try {
      await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ keywords }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      if (state.user) state.user.keywords = keywords;
      flash(`已保存 ${keywords.length} 个关键词`);
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function saveKeywordsMatchReports() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const input = $("#set-kw-reports");
    const on = !!(input && input.checked);
    try {
      const data = await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ keywords_match_reports: on }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      if (state.user) state.user.keywords_match_reports = !!(data && data.keywords_match_reports);
      flash(on ? "已开启研报匹配" : "已关闭研报匹配");
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      if (input) input.checked = !on;
      flash(err.message, "error");
    }
  }

  async function saveKeywordsMatchNews() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const input = $("#set-kw-news");
    const on = !!(input && input.checked);
    try {
      const data = await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ keywords_match_news: on }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      if (state.user) state.user.keywords_match_news = !!(data && data.keywords_match_news);
      flash(on ? "已开启财经资讯匹配" : "已关闭财经资讯匹配");
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      if (input) input.checked = !on;
      flash(err.message, "error");
    }
  }

  function syncReportWatchChips() {
    const watching = userKeywordSet();
    document.querySelectorAll(".ima-doc-tag.is-watch").forEach((btn) => {
      const on = watching.has(String(btn.dataset.tag || "").trim());
      btn.classList.toggle("is-selected", on);
      btn.setAttribute("aria-pressed", on ? "true" : "false");
      btn.title = on ? "已在关键词提醒中" : "加入关键词提醒";
    });
  }

  async function toggleReportKeyword(tag) {
    const name = String(tag || "").trim();
    if (!name || !isReportWatchableTag(name)) return;
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const current = [...userKeywordSet()];
    if (current.includes(name)) {
      flash("已在关键词提醒中");
      return;
    }
    if (current.length >= KEYWORDS_MAX_COUNT) {
      flash(`关键词最多 ${KEYWORDS_MAX_COUNT} 个`, "error");
      return;
    }
    const keywords = [...current, name];
    try {
      const data = await api("/api/me", {
        method: "PUT",
        body: JSON.stringify({ keywords, keywords_match_reports: true }),
      });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      if (state.user) {
        state.user.keywords = keywords;
        state.user.keywords_match_reports = data?.keywords_match_reports !== false;
      }
      syncReportWatchChips();
      flash("已加入关键词提醒，每日更新后推送");
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function testLlm() {
    const routeSeq = currentRouteSeq();
    try {
      const data = await api("/api/me/llm-test", {
        method: "POST",
        body: JSON.stringify({
          llm_api_base: ($("#set-llm-base").value || "").trim(),
          llm_api_key: ($("#set-llm-key").value || "").trim(),
          llm_model: ($("#set-llm-model").value || "").trim(),
          llm_api_format: ($("#set-llm-format").value || "chat").trim(),
        }),
      });
      if (!routeStillActive(routeSeq)) return;
      if (data.ok) {
        const tokens = data.usage && data.usage.total_tokens;
        flash(`可用 ${data.latency_ms}ms` + (tokens != null ? ` · ${tokens} tokens` : ""));
      } else {
        flash(data.error || "测试失败", "error");
      }
    } catch (err) {
      if (routeStillActive(routeSeq)) flash(err.message || "测试失败", "error");
    }
  }

  async function saveLlm() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const payload = {
      llm_api_base: ($("#set-llm-base").value || "").trim(),
      llm_api_key: ($("#set-llm-key").value || "").trim(),
      llm_model: ($("#set-llm-model").value || "").trim(),
      llm_api_format: ($("#set-llm-format").value || "chat").trim(),
    };
    try {
      await api("/api/me", { method: "PUT", body: JSON.stringify(payload) });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(payload.llm_api_key ? "已保存，将用你的模型" : "已保存，未填 Key 时用普通摘要");
      await reloadSettings(routeSeq);
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }

  async function loadLlmModels() {
    const routeSeq = currentRouteSeq();
    const select = $("#set-llm-model-select");
    const input = $("#set-llm-model");
    try {
      const data = await api("/api/me/llm-models", {
        method: "POST",
        body: JSON.stringify({
          llm_api_base: ($("#set-llm-base").value || "").trim(),
          llm_api_key: ($("#set-llm-key").value || "").trim(),
        }),
      });
      if (!routeStillActive(routeSeq)) return;
      const models = Array.isArray(data.models) ? data.models : [];
      const current = (input && input.value) || "";
      if (select) {
        select.innerHTML = [`<option value="">手填模型名</option>`]
          .concat(models.map((id) => {
            const selected = id === current ? " selected" : "";
            return `<option value="${escapeHtml(id)}"${selected}>${escapeHtml(id)}</option>`;
          }))
          .join("");
        select.hidden = models.length === 0;
        if (models.length && input && !input.value) {
          input.value = models[0];
          select.value = models[0];
        }
      }
      flash(models.length ? `已加载 ${models.length} 个模型` : "接口未返回模型，可手填模型名");
    } catch (err) {
      if (routeStillActive(routeSeq)) flash(err.message || "拉取模型失败", "error");
    }
  }

  async function savePassword() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const oldPw = $("#pw-old").value;
    const newPw = $("#pw-new").value;
    const confirmPw = $("#pw-confirm").value;
    if (!oldPw || newPw.length < 10) {
      flash("请填写原密码，新密码至少 10 位", "error");
      return;
    }
    if (newPw !== confirmPw) {
      flash("两次输入的新密码不一致", "error");
      return;
    }
    try {
      const data = await api("/api/me/password", {
        method: "POST",
        body: JSON.stringify({ old_password: oldPw, new_password: newPw }),
      });
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      if (data.token) {
        state.token = data.token;
        localStorage.setItem("dav_token", data.token);
      }
      $("#pw-old").value = $("#pw-new").value = $("#pw-confirm").value = "";
      flash("密码已修改");
    } catch (err) {
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash(err.message, "error");
    }
  }

  async function genBindCode() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    try {
      const data = await api("/api/me/bind-code", { method: "POST" });
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      pendingBind = {
        channel: "any",
        code: data.code,
        expiresAt: Date.now() + (data.expires_in_seconds || 600) * 1000,
      };
      $("#bind-result").innerHTML =
        `绑定码：<b style="font-size:var(--text-icon);letter-spacing:3px;font-family:var(--font-mono);font-variant-numeric:tabular-nums">${escapeHtml(data.code)}</b>` +
        `（${Math.floor(data.expires_in_seconds / 60)} 分钟内有效）<br>` +
        `发给机器人：<code>/bind ${escapeHtml(data.code)}</code>`;
      startSettingsPoll();
    } catch (err) {
      if (!routeStillActive(routeSeq) || token !== state.token
        || sessionGeneration !== imaMountState.sessionGeneration) return;
      flash(err.message, "error");
    }
  }


  return {
    fsPersonalState,
    stopFeishuPersonalPoll,
    feishuPersonalHtml,
    pushChannelsHtml,
    webPushSupported,
    renderBindResult,
    startFeishuPersonal,
    cancelFeishuPersonal,
    refreshFeishuBindCode,
    openBindGuide,
    savePushChannels,
    saveNotify,
    saveDailyReport,
    saveTranslateTwitter,
    toggleDnd,
    saveDnd,
    saveCustomTgBot,
    unbindChannel,
    saveWecomWebhook,
    saveBarkKey,
    enableWebPush,
    disableWebPush,
    saveKeywords,
    saveKeywordsMatchReports,
    saveKeywordsMatchNews,
    toggleReportKeyword,
    saveLlm,
    testLlm,
    loadLlmModels,
    savePassword,
    genBindCode,
    get pendingBind() { return pendingBind; },
    set pendingBind(v) { pendingBind = v; },
  };
}
