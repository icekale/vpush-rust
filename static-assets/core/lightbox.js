export function createLightbox(dependencies) {
  const { escapeHtml, imgOnError, trapFocus, CHEVRON_LEFT_ICON, CHEVRON_RIGHT_ICON, X_ICON } = dependencies;

  // ---------- 图片灯箱（点击放大原图，背景变暗，多图可左右切换） ----------
  let _lightboxImages = [];
  let _lightboxIndex = 0;

  function openLightbox(img) {
    if (!img) return;
    // 收集当前帖子（同一 .post-images 容器）里的全部图片，支持左右切换
    const container = img.closest(".post-images");
    if (container) {
      _lightboxImages = [...container.querySelectorAll("img")]
        .map((im) => im.currentSrc || im.src || "")
        .filter(Boolean);
    } else {
      _lightboxImages = [(img.currentSrc || img.src || "")].filter(Boolean);
    }
    if (!_lightboxImages.length) return;
    _lightboxIndex = Math.max(0, _lightboxImages.indexOf(img.currentSrc || img.src || ""));
    closeLightbox(); // 防重复打开
    const overlay = document.createElement("div");
    overlay.className = "lightbox";
    overlay.setAttribute("role", "dialog");
    overlay.setAttribute("aria-modal", "true");
    overlay.setAttribute("aria-label", "查看大图");
    overlay.innerHTML = `
      <button class="lightbox-close" aria-label="关闭" title="关闭" onclick="event.stopPropagation();closeLightbox()">${X_ICON}</button>
      <img class="lightbox-img" src="${escapeHtml(_lightboxImages[_lightboxIndex])}" alt="动态配图" onerror="imgOnError(this)">
      ${_lightboxImages.length > 1 ? `
        <button class="lightbox-nav lightbox-prev" aria-label="上一张" title="上一张" onclick="event.stopPropagation();lightboxStep(-1)">${CHEVRON_LEFT_ICON}</button>
        <button class="lightbox-nav lightbox-next" aria-label="下一张" title="下一张" onclick="event.stopPropagation();lightboxStep(1)">${CHEVRON_RIGHT_ICON}</button>
        <span class="lightbox-count">${_lightboxIndex + 1} / ${_lightboxImages.length}</span>` : ""}`;
    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) closeLightbox();
    });
    // 移动端滑动手势：左右滑动切换图片（与「点击遮罩关闭」的 tap 区分）
    overlay.addEventListener("touchstart", lightboxTouchStart, { passive: true });
    overlay.addEventListener("touchmove", lightboxTouchMove, { passive: false });
    overlay.addEventListener("touchend", lightboxTouchEnd, { passive: true });
    document.body.appendChild(overlay);
    document.body.classList.add("lightbox-open");
    document.addEventListener("keydown", lightboxKeyHandler);
    trapFocus(overlay, closeLightbox);
    overlay.querySelector(".lightbox-close")?.focus();
  }

  let _lbTouchStart = null;

  function lightboxTouchStart(e) {
    // 从箭头/关闭按钮上开始的滑动不拦截（按钮有自己的事件）
    if (e.target.closest(".lightbox-nav, .lightbox-close")) return;
    _lbTouchStart = { x: e.touches[0].clientX, y: e.touches[0].clientY };
  }

  function lightboxTouchMove(e) {
    if (!_lbTouchStart) return;
    const dx = e.touches[0].clientX - _lbTouchStart.x;
    const dy = e.touches[0].clientY - _lbTouchStart.y;
    // 水平滑动占优时拦截：阻止页面滚动，也阻止松手后合成 click（避免误关灯箱）
    if (Math.abs(dx) > 10 && Math.abs(dx) > Math.abs(dy)) e.preventDefault();
  }

  function lightboxTouchEnd(e) {
    if (!_lbTouchStart) return;
    const dx = e.changedTouches[0].clientX - _lbTouchStart.x;
    const dy = e.changedTouches[0].clientY - _lbTouchStart.y;
    _lbTouchStart = null;
    if (Math.abs(dx) < 40 || Math.abs(dx) < Math.abs(dy) * 1.5) return; // 阈值：水平 40px 且明显占优
    lightboxStep(dx < 0 ? 1 : -1);
  }

  function lightboxStep(dir) {
    if (_lightboxImages.length < 2) return;
    _lightboxIndex = (_lightboxIndex + dir + _lightboxImages.length) % _lightboxImages.length;
    const img = document.querySelector(".lightbox-img");
    if (!img) return;
    img.style.opacity = "0";
    setTimeout(() => {
      img.src = _lightboxImages[_lightboxIndex];
      img.style.opacity = "";
      img.onerror = imgOnError;
      const count = document.querySelector(".lightbox-count");
      if (count) count.textContent = `${_lightboxIndex + 1} / ${_lightboxImages.length}`;
    }, 120); // 与淡出过渡衔接
  }

  function lightboxKeyHandler(e) {
    if (e.key === "ArrowLeft") lightboxStep(-1);
    else if (e.key === "ArrowRight") lightboxStep(1);
  }

  function closeLightbox() {
    const overlay = document.querySelector(".lightbox:not(.closing)");
    if (!overlay) return;
    overlay.classList.add("closing"); // 触发淡出+轻微缩小动画
    // 动画结束后移除 DOM；reduced-motion 下 animation 被禁用（animationend 不触发），用超时兜底
    const remove = () => overlay.remove();
    overlay.addEventListener("animationend", remove, { once: true });
    setTimeout(remove, 240); // 略大于关闭动画 200ms；reduced-motion 下 animationend 不触发时兜底
    document.body.classList.remove("lightbox-open");
    document.removeEventListener("keydown", lightboxKeyHandler);
  }

  return { openLightbox, closeLightbox, lightboxStep };
}
