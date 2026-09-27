import { escapeHtml, imgProxyUrl, imgSrcFor } from "../core/html.js";
import { PLATFORM_ICONS } from "../core/platforms.js";

const CARD_WIDTH = 600;
const EXPORT_SCALE = 2;
const PHOTO_MAX = 1440;
const AVATAR_MAX = 256;
const CARD_ACCENT = "#1668e0";
const CARD_ACCENT_DARK = "#5a9bf5";
const HOST_ID = "vpush-post-card-host";
const HANDLE_PLATFORMS = new Set(["twitter", "truth"]);
const PROXY_HOSTS = new Set([
  "pbs.twimg.com",
  "video.twimg.com",
  "abs.twimg.com",
  "xqimg.imedao.com",
  "xueqiuimg.com",
  "wx1.sinaimg.cn",
  "wx2.sinaimg.cn",
  "wx3.sinaimg.cn",
  "wx4.sinaimg.cn",
  "static-assets-1.truthsocial.com",
]);

const CARD_CSS = `
  :host{all:initial;}
  *{box-sizing:border-box;}
  .stage{width:${CARD_WIDTH}px;font-family:-apple-system,BlinkMacSystemFont,"SF Pro Display","PingFang SC","Hiragino Sans GB","Noto Sans SC","Microsoft YaHei",sans-serif;}
  .card{width:${CARD_WIDTH}px;background:#fff;color:#1d1d1f;overflow:hidden;}
  .card.is-dark{background:#171a20;color:#e4e6eb;}
  .main{display:flex;gap:16px;padding:32px 32px 20px;}
  .brand{display:flex;align-items:center;justify-content:flex-end;padding:12px 32px 20px;border-top:1px solid rgba(12,18,34,.06);}
  .brand-id{display:flex;align-items:center;gap:8px;}
  .brand-logo{width:18px;height:18px;display:block;flex-shrink:0;}
  .brand-name{font-size:13px;line-height:18px;font-weight:600;color:#222c3c;}
  .card.is-dark .brand{border-color:rgba(255,255,255,.06);}
  .card.is-dark .brand-name{color:#f2f4f8;}
  .avatar{width:56px;height:56px;border-radius:12px;overflow:hidden;flex-shrink:0;background:#f3f4f7;}
  .avatar img{width:100%;height:100%;object-fit:cover;display:block;}
  .avatar-fallback{width:100%;height:100%;display:flex;align-items:center;justify-content:center;font-weight:600;font-size:20px;background:#eef4ff;color:${CARD_ACCENT};}
  .card.is-dark .avatar{background:#1a1e25;}
  .card.is-dark .avatar-fallback{background:#1a2433;color:${CARD_ACCENT_DARK};}
  .col{flex:1;min-width:0;display:flex;flex-direction:column;}
  .head{display:flex;align-items:flex-start;justify-content:space-between;gap:16px;}
  .who{display:flex;flex-direction:column;gap:2px;min-width:0;flex:1;}
  .name-row{display:flex;align-items:center;gap:6px;min-width:0;}
  .name{font-size:18px;line-height:28px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;color:#222c3c;}
  .handle,.time{font-size:14px;line-height:20px;color:#6e6e73;white-space:nowrap;}
  .mark{width:22px;height:22px;color:${CARD_ACCENT};flex-shrink:0;--platform-icon-fill:${CARD_ACCENT};}
  .mark svg{width:22px;height:22px;display:block;color:inherit;}
  .title{margin-top:12px;font-size:16px;line-height:24px;font-weight:600;color:#222c3c;word-break:break-word;}
  .text{margin-top:12px;font-size:16px;line-height:24px;color:#1d1d1f;white-space:pre-wrap;word-break:break-word;}
  .title + .text{margin-top:8px;}
  .entity{color:${CARD_ACCENT};}
  .media{margin-top:16px;display:grid;gap:2px;border-radius:12px;overflow:hidden;background:#f3f4f7;}
  .media img{width:100%;height:100%;object-fit:cover;display:block;background:#f3f4f7;}
  .media.n1{grid-template-columns:1fr;}
  .media.n1 img{max-height:480px;object-fit:contain;height:auto;}
  .media.n2{grid-template-columns:1fr 1fr;min-height:200px;}
  .media.n2 > *{min-height:200px;}
  .media.n3{grid-template-columns:1.15fr 1fr;grid-template-rows:1fr 1fr;min-height:248px;}
  .media.n3 > :first-child{grid-row:1 / span 2;min-height:248px;}
  .media.n3 > :not(:first-child){min-height:123px;}
  .media.n4{grid-template-columns:1fr 1fr;grid-template-rows:1fr 1fr;min-height:248px;}
  .media.n4 > *{min-height:123px;}
  .combo{margin-top:16px;display:grid;gap:10px;}
  .combo-stats{display:flex;flex-wrap:wrap;gap:4px 16px;padding:6px 10px;border:1px solid rgba(12,18,34,.06);background:#f3f4f7;font-size:13px;}
  .combo-stat{color:#1d1d1f;}
  .combo-stat b{color:#222c3c;font-variant-numeric:tabular-nums;}
  .combo-section{display:grid;gap:4px;}
  .combo-section-title{margin:0;font-size:13px;font-weight:600;color:#222c3c;}
  .combo-actions,.combo-holdings{border-top:1px solid rgba(12,18,34,.06);}
  .combo-action{display:grid;grid-template-columns:3.5em minmax(0,1fr) auto;align-items:baseline;column-gap:12px;padding:5px 0;border-bottom:1px solid rgba(12,18,34,.06);font-size:13px;}
  .combo-action-type{color:#6e6e73;white-space:nowrap;}
  .combo-action-name{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:600;color:#222c3c;}
  .combo-sym{color:#6e6e73;font-weight:400;font-variant-numeric:tabular-nums;}
  .combo-action-meta{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap;color:#6e6e73;}
  .combo-action-price{color:${CARD_ACCENT};margin-left:10px;}
  .combo-holdings{display:grid;grid-template-columns:1fr 1fr;column-gap:20px;}
  .combo-holding{display:table;table-layout:fixed;width:100%;padding:4px 0;border-bottom:1px solid rgba(12,18,34,.06);font-size:13px;}
  .combo-holding > span:first-child{display:table-cell;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;}
  .combo-w{display:table-cell;width:4.5em;text-align:right;white-space:nowrap;color:#222c3c;font-variant-numeric:tabular-nums;font-weight:600;}
  .combo-cash{color:#6e6e73;font-size:13px;}
  .combo-cash b{color:#222c3c;font-variant-numeric:tabular-nums;}
  .card.is-dark .name,.card.is-dark .title,.card.is-dark .combo-section-title,.card.is-dark .combo-action-name,.card.is-dark .combo-w,.card.is-dark .combo-stat b,.card.is-dark .combo-cash b{color:#f2f4f8;}
  .card.is-dark .text,.card.is-dark .combo-stat{color:#e4e6eb;}
  .card.is-dark .handle,.card.is-dark .time{color:#9aa3b2;}
  .card.is-dark .mark{color:${CARD_ACCENT_DARK};--platform-icon-fill:${CARD_ACCENT_DARK};}
  .card.is-dark .entity,.card.is-dark .combo-action-price{color:${CARD_ACCENT_DARK};}
  .card.is-dark .media,.card.is-dark .media img{background:#1a1e25;}
  .card.is-dark .combo-stats{background:#1a1e25;border-color:rgba(255,255,255,.06);}
  .card.is-dark .combo-actions,.card.is-dark .combo-holdings,.card.is-dark .combo-action,.card.is-dark .combo-holding{border-color:rgba(255,255,255,.06);}
  .card.is-dark .combo-action-type,.card.is-dark .combo-action-meta,.card.is-dark .combo-sym,.card.is-dark .combo-cash{color:#9aa3b2;}
`;

function parseDetail(raw) {
  if (!raw) return null;
  if (typeof raw === "object") return raw;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

function parsePublished(s) {
  const raw = String(s || "").trim();
  if (!raw) return null;
  const m = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})(?::(\d{2}))?$/.exec(raw);
  if (m) return new Date(Date.UTC(+m[1], +m[2] - 1, +m[3], +m[4] - 8, +m[5], +(m[6] || 0)));
  const d = new Date(raw);
  return Number.isNaN(d.getTime()) ? null : d;
}

export function formatCardTime(s, now = new Date()) {
  const d = parsePublished(s);
  if (!d) return String(s || "");
  const p = (n) => String(n).padStart(2, "0");
  const sameDay = (a, b) => a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
  if (sameDay(d, now)) return `今天 ${p(d.getHours())}:${p(d.getMinutes())}`;
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1);
  if (sameDay(d, yesterday)) return `昨天 ${p(d.getHours())}:${p(d.getMinutes())}`;
  if (d.getFullYear() === now.getFullYear()) return `${d.getMonth() + 1}月${d.getDate()}日`;
  return `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日`;
}

export function cardHandle(post) {
  if (!HANDLE_PLATFORMS.has(post?.platform)) return "";
  let raw = String(post.kol_external_id || "").trim().replace(/^@/, "");
  if (!raw) return "";
  const fromUrl = raw.match(/(?:^|[/.])(?:x|twitter|truthsocial)\.com\/(?:@)?([A-Za-z0-9_]+)/i);
  if (fromUrl) return fromUrl[1];
  return /^[A-Za-z0-9_]+$/.test(raw) ? raw : "";
}

function combinationDetail(post) {
  if (post?.platform !== "combination") return null;
  const detail = parseDetail(post.detail);
  if (!detail || typeof detail !== "object") return null;
  const stats = Array.isArray(detail.stats) ? detail.stats.filter((row) => Array.isArray(row) && row.length >= 2) : [];
  const actions = Array.isArray(detail.actions) ? detail.actions.filter((row) => row && typeof row === "object") : [];
  const holdings = Array.isArray(detail.holdings)
    ? detail.holdings.filter((row) => row && row.name && row.weight != null)
    : [];
  const cash = detail.cash ? String(detail.cash) : "";
  if (!stats.length && !actions.length && !holdings.length && !cash) return null;
  return { stats, actions, holdings, cash };
}

export function postToCardModel(post, options = {}) {
  const showSrc = !!options.showSrc;
  const srcC = (post?.content_src || "").trim();
  const srcT = (post?.title_src || "").trim();
  const translated = !!(srcC && srcC !== (post?.content || "").trim());
  const useSrc = showSrc && translated;
  const title = (useSrc ? srcT : (post?.title || "")).trim();
  const body = ((useSrc ? srcC : (post?.content || "")) || "").trim();
  // 译文标题/正文来自两次独立翻译，措辞可能不同导致前缀匹配漏判；
  // 原文侧 title_src 是 content_src 的截断前缀，用原文比对兜住
  const titleDup = !!title && (
    title === body
    || body.startsWith(title)
    || (!!(srcT && srcC) && srcC.startsWith(srcT))
  );
  const images = Array.isArray(post?.images) ? post.images.filter(Boolean).slice(0, 4) : [];
  return {
    id: post?.id ?? "",
    platform: post?.platform || "",
    name: String(post?.kol_name || "").trim(),
    handle: cardHandle(post),
    avatar: String(post?.avatar_url || "").trim(),
    publishedAt: post?.published_at || "",
    title: titleDup ? "" : title,
    body,
    images,
    combo: combinationDetail(post),
  };
}

export function hasCardContent(model) {
  return Boolean(
    model?.title ||
      model?.body ||
      model?.images?.length ||
      model?.combo
  );
}

function decoratePlainText(text) {
  return escapeHtml(text).replace(
    /(https?:\/\/[^\s]+|@[\w_]+|#[^\s#@]+)/g,
    (match) => `<span class="entity">${match}</span>`
  );
}

function paintMarkSvg(svg, color) {
  return String(svg || "")
    .replace(/fill="var\(--platform-icon-fill,\s*[^"]+\)"/g, `fill="${color}"`)
    .replace(/fill="currentColor"/g, `fill="${color}"`)
    .replace(/stroke="currentColor"/g, `stroke="${color}"`);
}

function vpushMark(darkCard) {
  const ink = darkCard ? "#e4e6eb" : "#222c3c";
  const dot = darkCard ? CARD_ACCENT_DARK : CARD_ACCENT;
  return `<svg class="brand-logo" viewBox="0 0 24 24" aria-hidden="true"><path d="M6.8 8.1 L12 16.4 L17.2 8.1" fill="none" stroke="${ink}" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round"/><circle cx="18.5" cy="5.6" r="1.7" fill="${dot}"/></svg>`;
}

function brandFooter(darkCard) {
  return `<div class="brand"><div class="brand-id">${vpushMark(darkCard)}<span class="brand-name">VPush</span></div></div>`;
}

function cardPlatformMark(platform, darkCard) {
  const svg = PLATFORM_ICONS[platform];
  if (!svg) return "";
  const color = darkCard ? CARD_ACCENT_DARK : CARD_ACCENT;
  return `<div class="mark" data-platform="${escapeHtml(platform)}">${paintMarkSvg(svg, color)}</div>`;
}

function mediaHtml(model) {
  const shots = (model.images || []).filter(Boolean).slice(0, 4);
  if (!shots.length) return "";
  const n = shots.length;
  return `<div class="media n${n}">${shots.map((src) => `<img alt="" src="${escapeHtml(src)}">`).join("")}</div>`;
}

function comboHtml(combo) {
  if (!combo) return "";
  const parts = [];
  if (combo.stats?.length) {
    parts.push(`<div class="combo-stats">${combo.stats.map(([key, value]) => `<span class="combo-stat"><b>${escapeHtml(key)}</b> ${escapeHtml(value)}</span>`).join("")}</div>`);
  }
  if (combo.actions?.length) {
    const rows = combo.actions.map((action) => {
      const type = action.type || "调整";
      const stock = action.stock || action.symbol || "";
      const symbol = action.stock && action.symbol
        ? ` <span class="combo-sym">${escapeHtml(action.symbol)}</span>`
        : "";
      const price = action.price != null && String(action.price).trim()
        ? `<span class="combo-action-price">${escapeHtml(action.price)}</span>`
        : "";
      return `<div class="combo-action"><span class="combo-action-type">${escapeHtml(type)}</span><strong class="combo-action-name">${escapeHtml(stock)}${symbol}</strong><span class="combo-action-meta">${escapeHtml(action.prev || "0.0%")} → ${escapeHtml(action.target || "0.0%")}${price}</span></div>`;
    }).join("");
    parts.push(`<section class="combo-section"><h3 class="combo-section-title">调仓明细</h3><div class="combo-actions">${rows}</div></section>`);
  }
  if (combo.holdings?.length) {
    parts.push(`<section class="combo-section"><h3 class="combo-section-title">现有持仓</h3><div class="combo-holdings">${combo.holdings.map((holding) => {
      const name = holding.name || holding.symbol || "";
      const symbol = holding.name && holding.symbol ? ` <span class="combo-sym">${escapeHtml(holding.symbol)}</span>` : "";
      return `<div class="combo-holding"><span>${escapeHtml(name)}${symbol}</span><span class="combo-w">${escapeHtml(holding.weight)}%</span></div>`;
    }).join("")}</div></section>`);
  }
  if (combo.cash) parts.push(`<div class="combo-cash">现金 <b>${escapeHtml(combo.cash)}</b></div>`);
  return parts.length ? `<div class="combo">${parts.join("")}</div>` : "";
}

export function paintCardPreview(host, model, options = {}) {
  if (!host) throw new Error("missing host");
  const shadow = host.shadowRoot || host.attachShadow({ mode: "open" });
  shadow.innerHTML = `<style>${CARD_CSS}</style><div class="stage">${renderCardHtml(model, options)}</div>`;
  return shadow;
}

export function renderCardHtml(model, options = {}) {
  const darkCard = !!options.darkCard;
  const initial = (model.name || model.handle || "V").slice(0, 1).toUpperCase();
  const avatar = model.avatar
    ? `<div class="avatar"><img alt="" src="${escapeHtml(model.avatar)}"></div>`
    : `<div class="avatar"><div class="avatar-fallback">${escapeHtml(initial)}</div></div>`;
  const handle = model.handle ? `<div class="handle">@${escapeHtml(model.handle)}</div>` : "";
  const title = model.title ? `<div class="title">${escapeHtml(model.title)}</div>` : "";
  const body = model.body ? `<div class="text">${decoratePlainText(model.body)}</div>` : "";
  return `
    <div class="card${darkCard ? " is-dark" : ""}" data-card-root data-platform="${escapeHtml(model.platform || "")}">
      <div class="main">
        ${avatar}
        <div class="col">
          <div class="head">
            <div class="who">
              <div class="name-row">
                <div class="name">${escapeHtml(model.name || "")}</div>
                ${handle}
              </div>
              ${model.publishedAt ? `<div class="time">${escapeHtml(formatCardTime(model.publishedAt, options.now))}</div>` : ""}
            </div>
            ${cardPlatformMark(model.platform, darkCard)}
          </div>
          ${title}
          ${body}
          ${mediaHtml(model)}
          ${comboHtml(model.combo)}
        </div>
      </div>
      ${brandFooter(darkCard)}
    </div>
  `;
}

function fetchCandidates(url) {
  if (!url) return [];
  if (url.startsWith("data:")) return [url];
  const first = imgSrcFor(url);
  const candidates = [first];
  try {
    const host = new URL(url, globalThis.location?.href || "https://vpush.local").hostname.toLowerCase();
    const proxied = imgProxyUrl(url);
    if (PROXY_HOSTS.has(host) && first !== proxied) candidates.push(proxied);
  } catch {
    /* ignore */
  }
  return candidates;
}

function blobToDataUrl(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("读图失败"));
    reader.onloadend = () => resolve(String(reader.result || ""));
    reader.readAsDataURL(blob);
  });
}

function loadImage(src) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("图片解码失败"));
    img.src = src;
  });
}

async function readImageBlob(url) {
  const resp = await fetch(url, { mode: "cors", credentials: "omit" });
  if (!resp.ok) throw new Error(`图片请求失败 ${resp.status}`);
  const blob = await resp.blob();
  if (!String(blob.type || "").startsWith("image/")) throw new Error("非图片内容");
  return blob;
}

async function toDataUrl(url) {
  let lastError = null;
  for (const candidate of fetchCandidates(url)) {
    try {
      if (candidate.startsWith("data:")) return candidate;
      return await blobToDataUrl(await readImageBlob(candidate));
    } catch (err) {
      lastError = err;
    }
  }
  if (lastError) throw lastError;
  return "";
}

async function fitImage(url, maxEdge) {
  try {
    const dataUrl = await toDataUrl(url);
    if (!dataUrl) return "";
    const img = await loadImage(dataUrl);
    const scale = Math.min(1, maxEdge / Math.max(img.width || maxEdge, img.height || maxEdge));
    if (scale >= 1) return dataUrl;
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(img.width * scale));
    canvas.height = Math.max(1, Math.round(img.height * scale));
    const ctx = canvas.getContext("2d");
    if (!ctx) return dataUrl;
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL("image/jpeg", 0.92);
  } catch {
    return "";
  }
}

async function resolveCardModel(model) {
  const [avatar, images] = await Promise.all([
    model.avatar ? fitImage(model.avatar, AVATAR_MAX) : "",
    Promise.all((model.images || []).map((src) => fitImage(src, PHOTO_MAX))),
  ]);
  return { ...model, avatar, images: images.filter(Boolean) };
}

function pageIsDark() {
  return document.documentElement.classList.contains("theme-dark");
}

function renderer() {
  let host = document.getElementById(HOST_ID);
  if (host) return host.shadowRoot;
  host = document.createElement("div");
  host.id = HOST_ID;
  host.style.cssText = `position:fixed;left:-12000px;top:0;width:${CARD_WIDTH}px;pointer-events:none;`;
  const shadow = host.attachShadow({ mode: "open" });
  shadow.innerHTML = `<style>${CARD_CSS}</style><div class="stage"></div>`;
  document.documentElement.appendChild(host);
  return shadow;
}

function waitForImages(root) {
  return Promise.all(
    [...root.querySelectorAll("img")].map((img) =>
      img.complete
        ? Promise.resolve()
        : new Promise((resolve) => {
            img.onload = img.onerror = () => resolve();
          })
    )
  );
}

let htmlToImageReady = null;

function ensureHtmlToImage() {
  if (globalThis.htmlToImage?.toCanvas) return Promise.resolve();
  if (htmlToImageReady) return htmlToImageReady;
  htmlToImageReady = new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = "/vendor/html-to-image.js";
    script.async = true;
    script.onload = () => {
      if (!globalThis.htmlToImage?.toCanvas) {
        htmlToImageReady = null;
        reject(new Error("导出库未加载"));
        return;
      }
      resolve();
    };
    script.onerror = () => {
      htmlToImageReady = null;
      reject(new Error("导出库加载失败"));
    };
    document.head.appendChild(script);
  });
  return htmlToImageReady;
}

function canvasToBlob(canvas) {
  return new Promise((resolve, reject) => {
    canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("导出失败"))), "image/png");
  });
}

async function renderCanvas(model) {
  await ensureHtmlToImage();
  const resolved = await resolveCardModel(model);
  const darkCard = pageIsDark();
  const shadow = renderer();
  shadow.querySelector(".stage").innerHTML = renderCardHtml(resolved, { darkCard });
  await waitForImages(shadow);
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const node = shadow.querySelector("[data-card-root]");
  if (!node) throw new Error("图卡没有渲染出来");
  if (!globalThis.htmlToImage?.toCanvas) throw new Error("导出库未加载");
  return globalThis.htmlToImage.toCanvas(node, {
    pixelRatio: EXPORT_SCALE,
    skipFonts: true,
    width: CARD_WIDTH,
    backgroundColor: darkCard ? "#171a20" : "#ffffff",
    fetchRequestInit: { mode: "cors", credentials: "omit" },
  });
}

function renderPngBlob(model) {
  return renderCanvas(model).then((canvas) => canvasToBlob(canvas));
}

function downloadBlob(blob, filename) {
  const href = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = href;
  a.download = filename;
  a.click();
  setTimeout(() => URL.revokeObjectURL(href), 2000);
}

function cardFilename(model) {
  const stem = String(model.handle || model.name || model.platform || "vpush")
    .replace(/[\\/:*?"<>|]+/g, "")
    .trim() || "vpush";
  return `${stem}-${model.id || "card"}.png`;
}

async function copyPng(pngPromise) {
  const clipboard = navigator.clipboard;
  const Item = window.ClipboardItem;
  if (!clipboard?.write || !Item) throw new Error("clipboard-unavailable");
  await clipboard.write([new Item({ "image/png": pngPromise })]);
}

function isPhone() {
  return window.matchMedia("(max-width: 768px)").matches;
}

async function sharePng(blob, filename) {
  const file = new File([blob], filename, { type: "image/png" });
  const data = { files: [file] };
  if (typeof navigator.share !== "function") throw new Error("share-unavailable");
  if (typeof navigator.canShare === "function" && !navigator.canShare(data)) {
    throw new Error("share-unavailable");
  }
  await navigator.share(data);
}

export function createPostCardExport({ findPost, isShowSrc, flash }) {
  async function startExportFromClick(post, button) {
    if (button?.dataset.busy === "1") return;
    const model = postToCardModel(post, { showSrc: isShowSrc?.(post.id) });
    if (!hasCardContent(model)) {
      flash("没有读到贴文内容", "error");
      return;
    }
    const pngPromise = renderPngBlob(model);
    if (button) {
      button.dataset.busy = "1";
      button.setAttribute("aria-busy", "true");
    }
    try {
      if (isPhone() && typeof navigator.share === "function") {
        const blob = await pngPromise;
        try {
          await sharePng(blob, cardFilename(model));
          return;
        } catch (err) {
          if (err?.name === "AbortError") return;
        }
        try {
          await copyPng(Promise.resolve(blob));
          flash("已复制，去微信粘贴即可");
          return;
        } catch {
          downloadBlob(blob, cardFilename(model));
          flash("无法复制，已改为下载");
          return;
        }
      }
      let copied = Promise.reject(new Error("clipboard-unavailable"));
      try {
        copied = copyPng(pngPromise);
      } catch {
        copied = Promise.reject(new Error("clipboard-unavailable"));
      }
      const [pngResult, copyResult] = await Promise.allSettled([pngPromise, copied]);
      if (copyResult.status === "fulfilled") {
        flash("已复制，去微信粘贴即可");
        return;
      }
      if (pngResult.status === "fulfilled") {
        downloadBlob(pngResult.value, cardFilename(model));
        flash("无法复制，已改为下载");
        return;
      }
      flash(pngResult.reason?.message || "生成失败，请再试一次", "error");
    } catch (err) {
      flash(err?.message || "生成失败，请再试一次", "error");
    } finally {
      if (button) {
        button.dataset.busy = "0";
        button.removeAttribute("aria-busy");
      }
    }
  }

  function exportPostCard(id, event) {
    const post = findPost(id);
    const fromEvent = event?.currentTarget;
    const button = fromEvent?.classList?.contains("post-card-export")
      ? fromEvent
      : document.querySelector(`.post-item[data-post-id="${CSS.escape(String(id))}"] .post-card-export`);
    if (!post) {
      flash("没有读到贴文内容", "error");
      return;
    }
    return startExportFromClick(post, button);
  }

  return { exportPostCard };
}
