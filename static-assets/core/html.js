export function escapeHtml(text) {
  return String(text ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

export function shellRevision(doc) {
  const imports = doc.querySelector('script[type="importmap"]')?.textContent;
  const style = doc.querySelector('link[rel="stylesheet"][href^="/style."]')?.getAttribute("href");
  const app = doc.querySelector('script[type="module"][src^="/app."]')?.getAttribute("src");
  return imports && style && app ? `${imports}|${style}|${app}` : "";
}

export function jsString(text) {
  return escapeHtml(JSON.stringify(String(text ?? "")));
}

export function imgProxyUrl(url) {
  return `/api/img-proxy?url=${encodeURIComponent(url)}`;
}

// 这些图床在大陆被墙且封锁形态不定：连接重置会触发 onerror 兜底，
// DNS 黑洞挂起则永远不触发——已知图床直接渲染为代理地址，不尝试直连
const IMG_PROXY_ALWAYS_HOSTS = new Set([
  "pbs.twimg.com",
  "video.twimg.com",
  "abs.twimg.com",
  // Truth Social 图床：大陆直连被 CDN 403，必须一开始就走代理
  "static-assets-1.truthsocial.com",
]);

export function imgSrcFor(url) {
  try {
    const host = new URL(url, globalThis.location?.href).hostname.toLowerCase();
    return IMG_PROXY_ALWAYS_HOSTS.has(host) ? imgProxyUrl(url) : url;
  } catch {
    return url;
  }
}

export function imgOnError(img) {
  // 第三方图床直连失败（大陆访问 X 图床被墙等）→ 经服务端代理转发
  if (!img) return;
  const src = img.getAttribute("src") || "";
  if (img.dataset.avatarFallback !== undefined && (img.dataset.proxied || src.startsWith("/"))) {
    const fallback = document.createElement("div");
    fallback.className = "kol-avatar";
    fallback.textContent = img.dataset.avatarFallback;
    img.replaceWith(fallback);
    return;
  }
  if (img.dataset.proxied || src.startsWith("/api/img-proxy")) return;
  img.dataset.proxied = "1";
  img.src = imgProxyUrl(src);
  img.onerror = img.dataset.avatarFallback !== undefined ? () => imgOnError(img) : null;
}
