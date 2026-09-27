/**
 * 模态框无障碍焦点管理 (A11y / Focus Trap)：
 * 1. 捕获 Tab / Shift+Tab 循环；
 * 2. 响应 Escape 关闭；
 * 3. 弹窗销毁时自动还原焦点至触发元素。
 */
export function trapFocus(container, onEscape) {
  const previousActive = document.activeElement;
  const focusableSelector = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

  function handleKeydown(e) {
    if (e.key === "Escape") {
      if (typeof onEscape === "function") {
        e.preventDefault();
        onEscape();
      }
      return;
    }
    if (e.key !== "Tab") return;
    const focusables = Array.from(container.querySelectorAll(focusableSelector)).filter((el) => el.offsetParent !== null || el === document.activeElement);
    if (!focusables.length) {
      e.preventDefault();
      return;
    }
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    if (e.shiftKey) {
      if (document.activeElement === first || !container.contains(document.activeElement)) {
        e.preventDefault();
        last.focus();
      }
    } else {
      if (document.activeElement === last || !container.contains(document.activeElement)) {
        e.preventDefault();
        first.focus();
      }
    }
  }

  container.addEventListener("keydown", handleKeydown);

  const initialFocusTimer = setTimeout(() => {
    if (!container.contains(document.activeElement)) {
      const initial = container.querySelector("[autofocus]") || container.querySelector(focusableSelector);
      if (initial) initial.focus();
    }
  }, 0);

  const observer = new MutationObserver(() => {
    if (!document.body.contains(container)) {
      clearTimeout(initialFocusTimer);
      observer.disconnect();
      container.removeEventListener("keydown", handleKeydown);
      if (previousActive && previousActive.isConnected) {
        previousActive.focus();
      }
    }
  });
  observer.observe(document.body, { childList: true });

  return () => {
    clearTimeout(initialFocusTimer);
    observer.disconnect();
    container.removeEventListener("keydown", handleKeydown);
    if (previousActive && previousActive.isConnected) {
      previousActive.focus();
    }
  };
}
