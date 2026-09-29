export function createImaView(dependencies) {
  const {
    $,
    state,
    api,
    apiBlob,
    flash,
    escapeHtml,
    emptyState,
    go,
    isRoute,
    isStandalonePwa,
    normalizeRoute,
    routePath,
    routeQuery,
    routeStillActive,
    sessionOwnerStillActive,
    currentRouteSeq,
    bumpRouteSeq,
    currentImaReaderSeq,
    bumpImaReaderSeq,
    setPageTitle,
    imaMountState,
    fmtCacheBytes,
    loadFeishuTimeline,
    feishuSourceDisplay,
    feishuSourcePillsHtml,
    resetFeishuTimelineMedia,
    userKeywordSet,
    isReportWatchableTag,
    SEARCH_ICON,
    REFRESH_ICON,
    X_ICON,
    EXTERNAL_LINK_ICON,
    CHEVRON_LEFT_ICON,
    CHEVRON_RIGHT_ICON,
    CHEVRON_DOWN_ICON,
  } = dependencies;

  function clearImaPdfUrl() {
    if (_imaPdfAbort) {
      _imaPdfAbort.abort();
      _imaPdfAbort = null;
    }
    const frame = $("#ima-pdf-frame");
    if (frame) frame.removeAttribute("src");
    const pdfUrl = window._imaPdfUrl;
    window._imaPdfUrl = "";
    if (pdfUrl) URL.revokeObjectURL(pdfUrl);
    resetFeishuTimelineMedia();
  }

  function _imaDocumentRoute(mediaId) {
    return `knowledge/${encodeURIComponent(mediaId).replace(/'/g, "%27")}`;
  }

  function imaDocumentReaderRoute(mediaId, groupId = "") {
    const listGroup = routeQuery().get("group") || state.imaDocumentsGroup || "";
    const listRoute = normalizeRoute(imaDocumentsRoute(
      listGroup,
      state.imaDocumentsQuery,
      state.imaDocumentsDay,
      state.imaDocumentsTag
    ));
    const params = new URLSearchParams(listRoute.split("?")[1] || "");
    if (groupId) params.set("doc_group", groupId);
    const query = params.toString();
    return `${_imaDocumentRoute(mediaId)}${query ? `?${query}` : ""}`;
  }

  function imaReaderDocumentGroup() {
    return routeQuery().get("doc_group")
      || routeQuery().get("group")
      || state.imaDocumentsGroup
      || "";
  }

  function openImaDocument(mediaId, groupId = "", replace = false) {
    const id = String(mediaId || "");
    if (!id) return;
    const listWasOpen = !!$("#ima-report-page");
    if (listWasOpen) captureImaListSnapshot(id, groupId, replace);
    const url = normalizeRoute(imaDocumentReaderRoute(id, groupId));
    if (location.pathname + location.search !== url) {
      if (replace) history.replaceState(null, "", url);
      else history.pushState(null, "", url);
    }
    mountKnowledgeReaderShell();
    const seq = bumpRouteSeq();
    renderImaDocument(seq, id);
  }

  function knowledgeMediaIdFromPath() {
    const [page, raw] = String(routePath() || "").split("/");
    if (page !== "knowledge" && page !== "ima-documents") return "";
    if (!raw) return "";
    try {
      return decodeURIComponent(raw);
    } catch {
      return raw;
    }
  }

  function knowledgeTypingTarget(el) {
    const tag = (el && el.tagName) || "";
    return /^(INPUT|TEXTAREA|SELECT)$/.test(tag) || !!(el && el.isContentEditable);
  }

  function onKnowledgeListKey(e) {
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return;
    if (!isRoute("knowledge") && !isRoute("ima-documents")) return;
    if (e.key === "Escape" && (_imaDayPicker.open || _imaTagMenu.open)) {
      e.preventDefault();
      closeImaDayPicker();
      closeImaTagMenu();
      return;
    }
    if (_imaDayPicker.open || _imaTagMenu.open) return;
    if (knowledgeTypingTarget(e.target)) return;
    if (e.key !== "j" && e.key !== "k") return;
    const rows = [...document.querySelectorAll("#kb-list .ima-doc-row")];
    if (rows.length) {
      const current = knowledgeMediaIdFromPath();
      let idx = rows.findIndex((row) => row.dataset.mediaId === current);
      if (e.key === "j") idx = idx < 0 ? 0 : idx + 1;
      else idx = idx < 0 ? rows.length - 1 : idx - 1;
      if (idx < 0 || idx >= rows.length) return;
      e.preventDefault();
      rows[idx].focus();
      return;
    }
    const snapshot = _imaListSnapshot;
    if (!snapshot || snapshot.items.length < 2) return;
    const idx = snapshot.items.findIndex((item) => String(item.media_id) === knowledgeMediaIdFromPath());
    if (idx < 0) return;
    const next = snapshot.items[idx + (e.key === "j" ? 1 : -1)];
    if (!next) return;
    e.preventDefault();
    openImaDocument(next.media_id, next.group_id || "", true);
  }

  function ensureKnowledgeKeys() {
    if (window._kbKeysBound) return;
    window._kbKeysBound = true;
    document.addEventListener("keydown", onKnowledgeListKey);
    document.addEventListener("pointerdown", onImaDayPickerDocDown);
  }

  function mountKnowledgeListShell() {
    ensureKnowledgeKeys();
    if ($("#ima-report-page") && $("#kb-list")) return;
    clearImaPdfUrl();
    $("#main").innerHTML = `
      <section class="section-panel ima-report-page" id="ima-report-page">
        <div id="kb-list" tabindex="-1"><div class="admin-skeleton" aria-hidden="true"></div></div>
      </section>`;
  }

  function mountKnowledgeReaderShell() {
    ensureKnowledgeKeys();
    if ($("#ima-reader-page")) return;
    $("#main").innerHTML = `
      <section class="section-panel ima-reader-page" id="ima-reader-page">
        <div id="kb-reader"><div class="admin-skeleton" aria-hidden="true"></div></div>
      </section>`;
  }

  const _imaItems = [];
  let _imaListSnapshot = null;
  let _imaStreamSnapshot = null;
  let _imaListSeq = 0;
  let _imaPdfAbort = null;

  let _imaLoadingMore = false;
  let _imaSearchTimer = null;
  let _imaDocsLoadObserver = null;
  let _imaDocsLoadFallback = null;
  let _imaSearchComposing = false;
  let _imaTagCounts = {};
  let _imaDocumentCount = 0;
  const IMA_TAG_COMMON_RATIO = 0.5;
  const IMA_ABSTRACT_CLAMP_CHARS = 120;

  function imaCountText(n) {
    return (Number(n) || 0).toLocaleString("zh-CN");
  }

  function imaResolvedCount(filtered, documentCount, itemCount, hasMore) {
    const loaded = Number(itemCount) || 0;
    const reported = Number(documentCount) || 0;
    // 读模型 document_count 是过滤后总数；JSON 回退路径仍可能给整库总量。
    // 已经一页装下、且上报数大于已加载数时，信已加载条数。
    if (filtered && !hasMore && loaded > 0 && reported > loaded) return loaded;
    return reported || loaded;
  }

  function imaDocumentsCountLabel(filtered, documentCount, itemCount, hasMore) {
    const total = imaResolvedCount(filtered, documentCount, itemCount, hasMore);
    return filtered ? `${imaCountText(total)} 条结果` : `${imaCountText(total)} 份`;
  }

  function imaSnapshotIsFiltered(snapshot) {
    const route = String((snapshot && snapshot.route) || "");
    return /[?&]q=/.test(route) || /[?&]tag=/.test(route);
  }

  function toggleImaAbstract(btn) {
    const box = btn && btn.closest(".ima-reader-abstract");
    if (!box) return;
    const expanded = box.classList.toggle("is-expanded");
    btn.textContent = expanded ? "收起" : "展开";
    btn.setAttribute("aria-expanded", expanded ? "true" : "false");
  }

  function fmtImaDay(day) {
    const raw = String(day || "").trim();
    let match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(raw);
    if (match) {
      const year = Number(match[1]);
      const base = `${Number(match[2])}月${Number(match[3])}日`;
      return year === new Date().getFullYear() ? base : `${year}年${base}`;
    }
    match = /^(\d{2})(\d{2})$/.exec(raw);
    if (!match) return raw === "unknown" ? "未知日期" : raw;
    return `${Number(match[1])}月${Number(match[2])}日`;
  }

  function fmtImaDayShort(day) {
    const raw = String(day || "").trim();
    let match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(raw);
    if (match) {
      const year = Number(match[1]);
      const base = `${Number(match[2])}/${Number(match[3])}`;
      return year === new Date().getFullYear() ? base : `${String(year).slice(2)}/${base}`;
    }
    match = /^(\d{2})(\d{2})$/.exec(raw);
    if (!match) return "";
    return `${Number(match[1])}/${Number(match[2])}`;
  }

  function imaDisplayTitle(name) {
    let s = String(name || "").replace(/\.pdf$/i, "").replace(/-副本$/u, "").trim();
    let lastCjk = -1;
    for (let i = 0; i < s.length; i += 1) {
      if (/[\u4e00-\u9fff]/.test(s[i])) lastCjk = i;
    }
    if (lastCjk >= 0) {
      const rest = s.slice(lastCjk + 1);
      const cut = rest.search(/\s+[A-Za-z]/);
      if (cut >= 0) s = s.slice(0, lastCjk + 1 + cut).trim();
    }
    return s;
  }

  function imaDocTicker(name) {
    const m = String(name || "").match(/[（(]([A-Z0-9][A-Z0-9.\-]*)[）)]/);
    return m ? m[1] : "";
  }

  function imaListTitle(name) {
    let s = imaDisplayTitle(name).replace(/[（(][A-Z0-9][A-Z0-9.\-]*[）)]/g, "");
    s = s.replace(/^[^\-–—]{1,8}[-–—]/, "");
    return s.replace(/\s+/g, " ").trim() || imaDisplayTitle(name);
  }

  function fmtDocSize(bytes) {
    const n = Number(bytes) || 0;
    return n > 0 ? fmtCacheBytes(n) : "";
  }

  function imaDocKindLabel(item) {
    if (item?.has_pdf) return "PDF";
    if (item?.has_txt) return "全文";
    return "仅摘要";
  }

  function imaDocumentTagsHtml(tags, interactive = false) {
    const list = Array.isArray(tags) ? tags.filter(Boolean) : [];
    if (!list.length) return "";
    const chips = list.map((tag) => {
      const label = escapeHtml(tag);
      return interactive
        ? `<button type="button" class="ima-doc-tag is-action" data-tag="${label}" onclick="event.stopPropagation();selectImaDocumentsTag(this.dataset.tag)">${label}</button>`
        : `<span class="ima-doc-tag">${label}</span>`;
    });
    return `<span class="ima-doc-tags">${chips.join("")}</span>`;
  }

  function imaWatchTagButton(tag) {
    const name = String(tag || "").trim();
    if (!name) return "";
    const watching = userKeywordSet().has(name);
    const pressed = watching ? "true" : "false";
    const selected = watching ? " is-selected" : "";
    const title = watching ? "已在关键词提醒中" : "加入关键词提醒";
    return `<button type="button" class="ima-doc-tag is-action is-watch${selected}" data-tag="${escapeHtml(name)}" aria-pressed="${pressed}" title="${title}" onclick="event.stopPropagation();toggleReportKeyword(this.dataset.tag)">${escapeHtml(name)}</button>`;
  }

  function imaReaderWatchHtml(tags) {
    const list = (Array.isArray(tags) ? tags : []).map((tag) => String(tag || "").trim()).filter(Boolean);
    if (!list.length) return "";
    const chips = list.map((tag) => (
      isReportWatchableTag(tag)
        ? imaWatchTagButton(tag)
        : `<span class="ima-doc-tag">${escapeHtml(tag)}</span>`
    ));
    return `<div class="ima-reader-watch" aria-label="研报标签">${chips.join("")}</div>`;
  }

  function imaTagCountsFromData(data) {
    const raw = data && data.tag_counts;
    if (raw && typeof raw === "object" && !Array.isArray(raw)) {
      const counts = {};
      for (const [tag, n] of Object.entries(raw)) {
        const name = String(tag || "").trim();
        const count = Number(n) || 0;
        if (name && count) counts[name] = count;
      }
      if (Object.keys(counts).length) return counts;
    }
    const counts = {};
    for (const item of data?.items || []) {
      for (const tag of Array.isArray(item.tags) ? item.tags : []) {
        const name = String(tag || "").trim();
        if (name) counts[name] = (counts[name] || 0) + 1;
      }
    }
    return counts;
  }

  function imaTagCoverageBase(counts = _imaTagCounts, documentCount = _imaDocumentCount) {
    const n = Number(documentCount) || 0;
    if (n > 0) return n;
    const values = Object.values(counts || {}).map((item) => Number(item) || 0);
    return values.length ? Math.max(...values) : 0;
  }

  function imaDistinctiveTags(tags, counts = _imaTagCounts, documentCount = _imaDocumentCount) {
    const list = (Array.isArray(tags) ? tags : []).map((tag) => String(tag || "").trim()).filter(Boolean);
    if (!list.length) return [];
    const freq = counts && typeof counts === "object" ? counts : {};
    const total = imaTagCoverageBase(freq, documentCount);
    const ranked = list
      .map((tag) => ({ tag, n: Number(freq[tag]) || 0 }))
      .sort((a, b) => a.n - b.n || a.tag.localeCompare(b.tag, "zh"));
    const rare = total
      ? ranked.filter((item) => item.n > 0 && item.n / total <= IMA_TAG_COMMON_RATIO)
      : ranked;
    return rare.slice(0, 2).map((item) => item.tag);
  }

  function imaExtractionBadgesHtml(item) {
    const ex = item?.extraction;
    if (!ex || ex.status !== "ok") return "";
    const parts = [];
    if (ex.rating) parts.push(`<span class="ima-ex-badge ima-ex-rating">${escapeHtml(ex.rating)}</span>`);
    if (ex.target_price) parts.push(`<span class="ima-ex-badge">${escapeHtml(ex.target_price)}</span>`);
    for (const t of (ex.tickers || []).slice(0, 3)) {
      // 标的徐章就是入口：/ticker/<code> 交给全局 <a href> 拦截器走 SPA 路由
      parts.push(`<a class="ima-ex-badge ima-ex-ticker" href="/ticker/${encodeURIComponent(t.code || t.name || "")}" title="${escapeHtml(ex.thesis || "")}">${escapeHtml(t.name || t.code)}</a>`);
    }
    return parts.join("");
  }

  function imaReportMetaHtml(item) {
    const parts = [];
    const ticker = imaDocTicker(item.name);
    if (ticker) parts.push(`<span>${escapeHtml(ticker)}</span>`);
    parts.push(imaExtractionBadgesHtml(item));
    for (const tag of imaDistinctiveTags(item?.tags)) {
      parts.push(isReportWatchableTag(tag) ? imaWatchTagButton(tag) : `<span>${escapeHtml(tag)}</span>`);
    }
    const size = fmtDocSize(item?.size);
    if (size) parts.push(`<span>${escapeHtml(size)}</span>`);
    return parts.length
      ? `<span class="ima-report-meta">${parts.join("")}</span>`
      : "";
  }

  function imaDocumentRow(item) {
    const day = fmtImaDayShort(item.sort_date || item.day) || "—";
    const source = String(item.group_name || "");
    const meta = imaReportMetaHtml(item); // .ima-report-meta
    const hasExtractionBadges = item.extraction?.status === "ok" && (
      item.extraction.rating || item.extraction.target_price || item.extraction.tickers?.length
    );
    const thesis = item.extraction?.status === "ok" && !hasExtractionBadges
      ? item.extraction?.thesis
      : "";
    const snippet = item.search_snippet ? `<span class="ima-report-snippet">${escapeHtml(item.search_snippet)}</span>`
      : thesis
        ? `<span class="ima-report-snippet">${escapeHtml(thesis)}</span>`
        : item.abstract
          ? `<span class="ima-report-snippet">${escapeHtml(item.abstract)}</span>`
          : "";
    return `
      <article class="ima-doc-row" role="button" tabindex="0" data-media-id="${escapeHtml(item.media_id)}" data-group-id="${escapeHtml(item.group_id || "")}" onclick="openImaDocument(this.dataset.mediaId, this.dataset.groupId)" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();openImaDocument(this.dataset.mediaId, this.dataset.groupId)}">
        <time class="ima-report-date">${escapeHtml(day)}</time>
        <span class="ima-report-copy"><strong class="ima-report-title">${escapeHtml(imaListTitle(item.name))}</strong>${snippet}${meta}</span>
        <span class="ima-report-source">${escapeHtml(source)}</span>
      </article>`;
  }

  function imaReportSkeletonHtml() {
    return Array.from({ length: 6 }, () => `
      <div class="ima-report-skeleton-row" aria-hidden="true">
        <span class="ima-report-date"></span>
        <span class="ima-report-copy"></span>
        <span class="ima-report-source"></span>
      </div>`).join("");
  }

  function imaDocumentsEmptyHtml(hasFilter) {
    if (hasFilter) {
      return emptyState(
        "没有找到相关研报",
        `<div><p class="section-meta">换个公司、代码或主题试试</p><button type="button" class="btn-normal" onclick="clearImaDocumentsFilters()">清除筛选</button></div>`
      );
    }
    return emptyState("这里还没有研报");
  }

  function imaDocumentGroups(items, showGroupLabel = false) {
    const groups = new Map();
    for (const item of items || []) {
      const key = item.sort_date || item.day;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(item);
    }
    return [...groups.entries()].map(([day, rows]) => `
      <section class="ima-doc-day">
        <header class="ima-doc-day-head"><h2>${escapeHtml(fmtImaDay(day) || day)}</h2><span>${rows.length} 份</span></header>
        <div class="ima-doc-list">${rows.map((item) => imaDocumentRow(item)).join("")}</div>
      </section>`).join("");
  }

  function imaDocumentsGroupFromRoute() {
    return routeQuery().get("group") || "";
  }

  function imaDocumentsRequestPath(options = {}) {
    const params = new URLSearchParams();
    const query = imaUsableSearchQuery(routeQuery().get("q") || "");
    const day = routeQuery().get("day") || "";
    const tag = routeQuery().get("tag") || "";
    const group = imaDocumentsGroupFromRoute() || "";
    if (query) params.set("q", query);
    if (tag) params.set("tag", tag);
    if (group) params.set("group", group);
    if (query || tag || !day) {
      params.set("limit", "50");
      params.set("offset", "0");
    } else {
      params.set("day", day);
    }
    // 首屏只取列表：无筛选的全量列表在冷页缓存下分面聚合比列表切片慢一个量级，改由 refreshImaListFacets() 随后单独取
    if (options.facetsOnly) params.set("facets_only", "1");
    else if (imaFacetsDeferred()) params.set("include_facets", "0");
    return `/api/ima-documents?${params.toString()}`;
  }

  // 只有无筛选的默认列表才延后分面（这种全量列表的分面聚合最贵）；搜索/标签/按日路径照旧一次拿全
  function imaFacetsDeferred() {
    const params = routeQuery();
    if (imaUsableSearchQuery(params.get("q") || "")) return false;
    return !params.get("tag") && !params.get("day");
  }

  function imaDocumentsRoute(group, query, day, tag) {
    const params = new URLSearchParams();
    if (group) params.set("group", group);
    if (query) params.set("q", query);
    if (day) params.set("day", day);
    const selectedTag = tag === undefined ? (state.imaDocumentsTag || "") : tag;
    if (selectedTag) params.set("tag", selectedTag);
    const routeQueryString = params.toString();
    return `knowledge${routeQueryString ? `?${routeQueryString}` : ""}`;
  }

  function imaDocumentKey(mediaId, groupId = "") {
    return `${String(groupId || "")}\u0000${String(mediaId || "")}`;
  }

  function cloneImaListSnapshotFields() {
    const body = $("#ima-docs-body");
    if (!body) return null;
    return {
      items: _imaItems.map((item) => ({ ...item, tags: [...(item.tags || [])] })),
      hasMore: !!state.imaDocumentsHasMore,
      days: [...(state.imaDocumentsDays || [])],
      tagCounts: { ..._imaTagCounts },
      documentCount: _imaDocumentCount,
      scrollTop: body.scrollTop,
      selectedKey: "",
      consumed: false,
    };
  }

  function captureImaListSnapshot(selectedMediaId = "", selectedGroupId = "", replaced = false) {
    const fields = cloneImaListSnapshotFields();
    if (!fields) return;
    _imaListSnapshot = {
      ...fields,
      route: location.pathname + location.search,
      selectedKey: imaDocumentKey(selectedMediaId, selectedGroupId),
      // replaced=true：阅读器条目是用 replaceState 顶掉列表条目打开的，上一条历史不是列表
      replaced,
    };
  }

  function stashImaStreamSnapshot() {
    if (routeQuery().get("q") || routeQuery().get("tag") || routeQuery().get("day")) return;
    const fields = cloneImaListSnapshotFields();
    if (!fields || !fields.items.length) return;
    _imaStreamSnapshot = fields;
  }

  function adoptImaStreamSnapshot() {
    if (!_imaStreamSnapshot || currentImaListSnapshot()) return;
    if (imaUsableSearchQuery(routeQuery().get("q") || "") || routeQuery().get("tag") || routeQuery().get("day")) return;
    _imaListSnapshot = {
      ..._imaStreamSnapshot,
      items: _imaStreamSnapshot.items.map((item) => ({ ...item, tags: [...(item.tags || [])] })),
      days: [..._imaStreamSnapshot.days],
      tagCounts: { ..._imaStreamSnapshot.tagCounts },
      route: location.pathname + location.search,
      consumed: false,
    };
  }

  function currentImaListSnapshot() {
    const snapshot = _imaListSnapshot;
    if (!snapshot || snapshot.consumed || snapshot.route !== location.pathname + location.search) return null;
    if (!snapshot.items.length) {
      // 快照在列表数据取回前被捕获（如飞书组直开时间线），空快照不还原，走正常加载
      snapshot.consumed = true;
      return null;
    }
    return snapshot;
  }

  function restoreImaListSnapshot(snapshot, body) {
    if (!snapshot || !body) return false;
    _imaItems.length = 0;
    _imaItems.push(...snapshot.items.map((item) => ({ ...item, tags: [...(item.tags || [])] })));
    state.imaDocumentsHasMore = snapshot.hasMore;
    state.imaDocumentsDays = [...snapshot.days];
    _imaTagCounts = { ...snapshot.tagCounts };
    _imaDocumentCount = snapshot.documentCount;
    const more = snapshot.hasMore
      ? `<div id="ima-docs-more" class="ima-docs-more" role="status" aria-live="polite">下滑加载更多</div>`
      : "";
    body.innerHTML = `<div class="ima-doc-list">${snapshot.items.map((item) => imaDocumentRow(item)).join("")}</div>${more}`;
    startImaDocumentsAutoLoad();
    requestAnimationFrame(() => {
      body.scrollTop = snapshot.scrollTop;
      const row = [...body.querySelectorAll(".ima-doc-row")].find((item) => imaDocumentKey(item.dataset.mediaId, item.dataset.groupId) === snapshot.selectedKey);
      if (row) {
        row.classList.add("is-selected");
        row.setAttribute("aria-current", "true");
      }
      if (snapshot.focusSearch) {
        snapshot.focusSearch = false;
        $("#ima-doc-q")?.focus();
      }
    });
    snapshot.consumed = true;
    return true;
  }

  function replaceImaDocumentsRoute(path) {
    const url = normalizeRoute(path);
    if (location.pathname + location.search !== url) history.replaceState(null, "", url);
  }

  async function selectImaDocumentGroup(value) {
    const groupId = String(value || "");
    state.imaDocumentsGroup = groupId;
    state.imaDocumentsDay = "";
    state.imaDocumentsDays = [];
    state.imaDocumentsTag = "";
    state.imaDocumentsQuery = $("#ima-doc-q")?.value?.trim() || state.imaDocumentsQuery || "";
    replaceImaDocumentsRoute(imaDocumentsRoute(groupId, state.imaDocumentsQuery, "", ""));
    const seq = bumpRouteSeq();
    // 飞书来源一库一文：选中即直接进时间线，省一次点击；带搜索词时仍回列表
    if (groupId.startsWith("feishu-") && !imaUsableSearchQuery(state.imaDocumentsQuery)) {
      renderImaDocuments(seq);
      try {
        const data = await api(`/api/ima-documents?group=${encodeURIComponent(groupId)}&limit=1`);
        if (!routeStillActive(seq)) return;
        const item = (data.items || [])[0];
        if (item) {
          openImaDocument(item.media_id, groupId, true);
          return;
        }
      } catch { /* 取不到就停留在列表 */ }
      return;
    }
    renderImaDocuments(seq);
  }

  function imaUsableSearchQuery(raw) {
    const query = String(raw || "").trim();
    if (!query) return "";
    if (query.length < 2 && /^[\x00-\x7F]*$/.test(query)) return "";
    return query;
  }

  function queueImaDocumentsSearch() {
    if (_imaSearchComposing) return;
    clearTimeout(_imaSearchTimer);
    _imaSearchTimer = setTimeout(() => submitImaDocumentsSearch(), 250);
  }

  function submitImaDocumentsSearch() {
    clearTimeout(_imaSearchTimer);
    _imaSearchTimer = null;
    if (!$("#ima-report-page") || !$("#ima-doc-q")) {
      _imaSearchComposing = false;
      return;
    }
    const nextQuery = imaUsableSearchQuery($("#ima-doc-q")?.value || "");
    if (nextQuery && !routeQuery().get("q") && !routeQuery().get("tag")) stashImaStreamSnapshot();
    state.imaDocumentsQuery = nextQuery;
    state.imaDocumentsDay = "";
    replaceImaDocumentsRoute(imaDocumentsRoute(state.imaDocumentsGroup, state.imaDocumentsQuery, state.imaDocumentsDay, state.imaDocumentsTag));
    const seq = bumpRouteSeq();
    renderImaDocuments(seq);
  }

  function selectImaDocumentsDay(value) {
    const day = String(value || "");
    state.imaDocumentsQuery = "";
    state.imaDocumentsTag = "";
    state.imaDocumentsDay = day;
    const input = $("#ima-doc-q");
    if (input) input.value = "";
    replaceImaDocumentsRoute(imaDocumentsRoute(state.imaDocumentsGroup, "", state.imaDocumentsDay, ""));
    const seq = bumpRouteSeq();
    renderImaDocuments(seq);
  }

  function selectImaDocumentsTag(value) {
    state.imaDocumentsTag = String(value || "");
    state.imaDocumentsDay = "";
    replaceImaDocumentsRoute(imaDocumentsRoute(state.imaDocumentsGroup, state.imaDocumentsQuery, state.imaDocumentsDay, state.imaDocumentsTag));
    const seq = bumpRouteSeq();
    renderImaDocuments(seq);
  }

  let _imaDayPicker = { open: false };

  function imaDayMenuDays(days) {
    return (Array.isArray(days) ? days : []).filter((day) => /^\d{4}$/.test(day));
  }

  function imaDayMenuHtml(day, days) {
    const current = String(day || "");
    const items = [`<button type="button" role="option" class="kb-desk-day-option${current ? "" : " is-selected"}" aria-selected="${!current}" data-ima-day="">最新</button>`];
    for (const key of imaDayMenuDays(days)) {
      const on = key === current;
      items.push(`<button type="button" role="option" class="kb-desk-day-option${on ? " is-selected" : ""}" aria-selected="${on}" data-ima-day="${escapeHtml(key)}">${escapeHtml(fmtImaDay(key))}</button>`);
    }
    return `<div class="kb-desk-day-menu" role="listbox" aria-label="日期">${items.join("")}</div>`;
  }

  function placeImaDayMenu(menu, trigger) {
    const box = trigger.getBoundingClientRect();
    const list = $("#kb-list")?.getBoundingClientRect();
    const width = Math.max(140, box.width);
    const minL = list ? list.left + 8 : 8;
    const maxL = (list ? list.right : window.innerWidth) - width - 8;
    const left = Math.min(Math.max(minL, box.right - width), Math.max(minL, maxL));
    menu.style.position = "fixed";
    menu.style.left = `${left}px`;
    menu.style.top = `${box.bottom + 4}px`;
    menu.style.minWidth = `${width}px`;
  }

  function closeImaDayPicker() {
    _imaDayPicker.open = false;
    document.querySelectorAll(".kb-desk-day-menu").forEach((el) => el.remove());
    const trigger = $("#ima-doc-day");
    if (trigger) trigger.setAttribute("aria-expanded", "false");
  }

  function renderImaDayMenu() {
    const trigger = $("#ima-doc-day");
    const host = trigger?.closest(".ima-report-searchbox") || trigger?.closest(".kb-desk-search") || trigger?.parentElement;
    if (!trigger || !host) return;
    host.querySelector(".kb-desk-day-menu")?.remove();
    host.insertAdjacentHTML("beforeend", imaDayMenuHtml(state.imaDocumentsDay || "", state.imaDocumentsDays));
    const menu = host.querySelector(".kb-desk-day-menu");
    if (menu) placeImaDayMenu(menu, trigger);
    trigger.setAttribute("aria-expanded", "true");
  }

  function toggleImaDayPicker(event) {
    event?.stopPropagation();
    if (_imaDayPicker.open) {
      closeImaDayPicker();
      return;
    }
    _imaDayPicker.open = true;
    renderImaDayMenu();
  }

  function pickImaDay(value) {
    closeImaDayPicker();
    selectImaDocumentsDay(value);
  }

  let _imaTagMenu = { open: false, keys: [] };

  function closeImaTagMenu() {
    _imaTagMenu.open = false;
    document.querySelectorAll(".kb-desk-day-menu[data-tag-menu]").forEach((el) => el.remove());
    $("#ima-doc-tag")?.setAttribute("aria-expanded", "false");
  }

  // 标签名是任意字符串，不内联进 onclick（防注入），菜单存 keys，点选传下标
  function imaTagMenuHtml(current) {
    const counts = _imaTagCounts || {};
    const keys = Object.keys(counts).sort((a, b) => (counts[b] || 0) - (counts[a] || 0));
    if (current && !keys.includes(current)) keys.unshift(current);
    _imaTagMenu.keys = keys;
    const items = [`<button type="button" role="option" class="kb-desk-day-option${current ? "" : " is-selected"}" aria-selected="${!current}" data-ima-tag-index="-1">全部</button>`];
    keys.forEach((key, i) => {
      const on = key === current;
      items.push(`<button type="button" role="option" class="kb-desk-day-option${on ? " is-selected" : ""}" aria-selected="${on}" data-ima-tag-index="${i}">${escapeHtml(key)}${counts[key] ? `（${counts[key]}）` : ""}</button>`);
    });
    return `<div class="kb-desk-day-menu" data-tag-menu role="listbox" aria-label="标签">${items.join("")}</div>`;
  }

  function toggleImaTagMenu(event) {
    event?.stopPropagation();
    if (_imaTagMenu.open) {
      closeImaTagMenu();
      return;
    }
    closeImaDayPicker();
    const trigger = $("#ima-doc-tag");
    const host = trigger?.closest(".ima-report-tag");
    if (!trigger || !host) return;
    _imaTagMenu.open = true;
    host.insertAdjacentHTML("beforeend", imaTagMenuHtml(String(state.imaDocumentsTag || "")));
    const menu = host.querySelector(".kb-desk-day-menu");
    if (menu) {
      menu.style.position = "fixed";
      const box = trigger.getBoundingClientRect();
      const width = Math.max(160, box.width);
      menu.style.left = `${Math.max(8, Math.min(box.left, window.innerWidth - width - 8))}px`;
      menu.style.top = `${box.bottom + 4}px`;
      menu.style.width = `${width}px`;
    }
    trigger.setAttribute("aria-expanded", "true");
  }

  function pickImaTag(i) {
    const key = i >= 0 ? (_imaTagMenu.keys?.[i] || "") : "";
    closeImaTagMenu();
    selectImaDocumentsTag(key);
  }

  function onImaDayPickerDocDown(event) {
    if (_imaTagMenu.open && !event.target?.closest?.(".ima-tag-trigger, .kb-desk-day-menu[data-tag-menu]")) closeImaTagMenu();
    if (!_imaDayPicker.open) return;
    if (event.target?.closest?.(".kb-desk-day, .kb-desk-day-menu")) return;
    closeImaDayPicker();
  }

  function imaDocumentsDayNavHtml(day, days) {
    const list = imaDayMenuDays(days);
    if (!list.length && !(Array.isArray(days) && days.length)) return "";
    const current = String(day || "");
    const label = current ? (fmtImaDay(current) || current) : "最新";
    return `<button type="button" class="kb-desk-day" id="ima-doc-day" aria-label="筛选日期" aria-haspopup="listbox" aria-expanded="false" onclick="toggleImaDayPicker(event)">${escapeHtml(label)}</button>`;
  }

  function refreshImaDocuments() {
    const seq = bumpRouteSeq();
    renderImaDocuments(seq, { keepOld: true });
  }

  function imaReportRefreshErrorHtml(message) {
    return `<div class="ima-report-refresh-error" role="alert"><span>最新研报暂时无法更新：${escapeHtml(message)}</span><button type="button" class="btn-ghost" onclick="refreshImaDocuments()">重试</button></div>`;
  }

  function knowledgeCardSummary(group) {
    const count = Number(group.document_count || 0);
    const day = fmtImaDay(group.latest_sort_date || group.latest_day);
    return count ? `${count} 份${day ? ` · ${day}` : ""}` : "还没有文档";
  }

  function knowledgeLibRowHtml(group, selected, mode) {
    const id = String(group.id || "");
    const name = feishuSourceDisplay(group.name || id).label;
    const summary = knowledgeCardSummary(group);
    if (mode === "available") {
      return `<div class="kb-lib-row is-available">
        <div class="kb-lib-copy"><strong class="kb-lib-name">${escapeHtml(name)}</strong><span class="kb-lib-meta">${escapeHtml(summary)}</span></div>
        <button type="button" class="btn-normal" data-group="${escapeHtml(id)}" onclick="subscribeKnowledge(this.dataset.group, this)">订阅</button>
      </div>`;
    }
    const on = id === String(selected || "");
    const unsub = !state.user?.is_admin
      ? `<button type="button" class="btn-ghost kb-lib-unsub" data-group="${escapeHtml(id)}" data-name="${escapeHtml(name)}" onclick="event.stopPropagation();unsubscribeKnowledge(this.dataset.group, this)">退订</button>`
      : "";
    return `<div class="kb-lib-row${on ? " is-selected" : ""}${unsub ? " has-unsub" : ""}">
      <button type="button" class="kb-lib-open" role="option" aria-selected="${on}" data-group="${escapeHtml(id)}" onclick="selectImaDocumentGroup(this.dataset.group)"><strong class="kb-lib-name">${escapeHtml(name)}</strong><span class="kb-lib-meta">${escapeHtml(summary)}</span></button>
      ${unsub}
    </div>`;
  }

  function knowledgeSourceControlsHtml(selectedGroup = "") {
    const subscribed = state.imaCatalogSubscribed || [];
    const sources = subscribed.map((group) => ({
      group_id: String(group.id || ""),
      title: group.name || group.id,
    }));
    const pillsHtml = `<div class="kb-source-pills-desk">${feishuSourcePillsHtml(sources, selectedGroup, "knowledge")}</div>`;
    const mobileOptions = [
      `<option value="" ${!selectedGroup ? "selected" : ""}>研报中心</option>`,
      ...sources.map((s) => `<option value="${escapeHtml(s.group_id)}" ${s.group_id === selectedGroup ? "selected" : ""}>${escapeHtml(s.title)}</option>`)
    ].join("");
    const mobileSelectHtml = `<div class="kb-source-select-wrap"><select class="kb-source-select-mobile" aria-label="切换研报库" onchange="selectImaDocumentGroup(this.value)">${mobileOptions}</select>${CHEVRON_DOWN_ICON}</div>`;
    return `<div class="ima-report-source">${pillsHtml}${mobileSelectHtml}</div>`;
  }

  function refreshKnowledge() {
    const seq = bumpRouteSeq();
    renderKnowledge(seq);
  }

  async function subscribeKnowledge(groupId, btn) {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    if (btn) btn.disabled = true;
    try {
      await api(`/api/ima-documents/groups/${encodeURIComponent(groupId)}/subscribe`, { method: "POST" });
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash("已订阅");
      replaceImaDocumentsRoute(imaDocumentsRoute(groupId, "", "", ""));
      refreshKnowledge();
    } catch (err) {
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash(err.message || "订阅失败", "error");
      if (btn) btn.disabled = false;
    }
  }

  async function unsubscribeKnowledge(groupId, btn) {
    const name = btn?.dataset?.name || "这个研报库";
    if (!confirm(`退订后将无法打开「${name}」。确定退订？`)) return;
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    if (btn) btn.disabled = true;
    try {
      await api(`/api/ima-documents/groups/${encodeURIComponent(groupId)}/subscribe`, { method: "DELETE" });
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash("已退订");
      replaceImaDocumentsRoute(imaDocumentsRoute("", "", "", ""));
      refreshKnowledge();
    } catch (err) {
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash(err.message || "退订失败", "error");
      if (btn) btn.disabled = false;
    }
  }

  function prefetchKnowledge(mediaId = "") {
    const catalog = api("/api/ima-documents/catalog");
    const documents = mediaId || currentImaListSnapshot() ? null : api(imaDocumentsRequestPath());
    return {
      catalog,
      documents,
      settled: Promise.allSettled(documents ? [catalog, documents] : [catalog]),
    };
  }

  function tickerDigestHtml(digest) {
    if (!digest) return "";
    const steps = (digest.evolution || []).map((step) => `
        <li class="ima-tk-step"><time>${escapeHtml(step.date || "")}</time><span>${escapeHtml(step.point)}</span></li>`).join("");
    if (!digest.consensus && !digest.divergence && !steps) return "";
    const updated = String(digest.updated_at || "").slice(0, 16);
    return `
      <section class="ima-tk-digest" aria-label="标的综述">
        <header class="ima-tk-digest-head">
          <strong>综述</strong>
          <span class="section-meta">由 ${digest.source_count || 0} 篇研报要点编译${updated ? ` · ${escapeHtml(updated)}` : ""}</span>
        </header>
        ${digest.consensus ? `<p class="ima-tk-consensus">${escapeHtml(digest.consensus)}</p>` : ""}
        ${steps ? `<ol class="ima-tk-steps">${steps}</ol>` : ""}
        ${digest.divergence ? `<p class="ima-tk-divergence"><strong>分歧与风险：</strong>${escapeHtml(digest.divergence)}</p>` : ""}
      </section>`;
  }

  function tickerTimelineRowHtml(item, groupNames) {
    const ex = item.extraction || {};
    const day = fmtImaDayShort(item.sort_date || item.day) || "—";
    const badges = [
      ex.rating ? `<span class="ima-ex-badge ima-ex-rating">${escapeHtml(ex.rating)}</span>` : "",
      ex.target_price ? `<span class="ima-ex-badge">${escapeHtml(ex.target_price)}</span>` : "",
    ].join("");
    const source = groupNames.get(String(item.group_id || "")) || "";
    return `
      <article class="ima-doc-row ima-tk-row" role="button" tabindex="0" data-media-id="${escapeHtml(item.media_id)}" data-group-id="${escapeHtml(item.group_id || "")}" onclick="openImaDocument(this.dataset.mediaId, this.dataset.groupId)" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();openImaDocument(this.dataset.mediaId,this.dataset.groupId)}">
        <time class="ima-report-date">${escapeHtml(day)}</time>
        <span class="ima-report-copy"><strong class="ima-report-title">${escapeHtml(imaListTitle(item.name))}</strong>${ex.thesis ? `<span class="ima-report-snippet">${escapeHtml(ex.thesis)}</span>` : ""}<span class="ima-report-meta">${badges}</span></span>
        <span class="ima-report-source">${escapeHtml(source)}</span>
      </article>`;
  }

  async function renderTickerPage(seq, encodedCode = "") {
    stopImaDocumentsAutoLoad();
    const code = encodedCode ? decodeURIComponent(encodedCode) : "";
    setPageTitle("标的综述", true, "knowledge", "回研报中心");
    if (!code) {
      $("#main").innerHTML = emptyState("缺少标的代码", `<div><a class="btn-normal" href="/knowledge">回研报中心</a></div>`);
      return;
    }
    $("#main").innerHTML = `<div class="admin-skeleton" aria-hidden="true"></div>`;
    let data;
    try {
      data = await api(`/api/ima-documents/tickers/${encodeURIComponent(code)}`);
    } catch (err) {
      if (!routeStillActive(seq)) return;
      $("#main").innerHTML = emptyState(`加载失败：${err.message}`, `<div><a class="btn-normal" href="/knowledge">回研报中心</a></div>`);
      return;
    }
    if (!routeStillActive(seq)) return;
    const groupNames = new Map();
    for (const group of [...(state.imaCatalogSubscribed || []), ...(state.imaCatalogAvailable || [])]) {
      groupNames.set(String(group.id), String(group.name || ""));
    }
    const digest = data.digest || {};
    const rows = (data.items || []).map((item) => tickerTimelineRowHtml(item, groupNames)).join("");
    const title = data.name ? `${data.name} ${data.code}` : String(data.code || code);
    $("#main").innerHTML = `
      <div class="ima-tk-page">
        <header class="ima-tk-head">
          <h2 class="ima-tk-title">${escapeHtml(title)}</h2>
          <p class="section-meta">${data.count || 0} 篇研报</p>
        </header>
        ${tickerDigestHtml(digest)}
        ${rows ? `<div class="ima-doc-list">${rows}</div>` : emptyState("该标的暂无可见研报", "")}
      </div>`;
  }

  async function renderKnowledge(seq, encodedMediaId = "", prefetched = null) {
    stopImaDocumentsAutoLoad();
    const mediaId = encodedMediaId ? decodeURIComponent(encodedMediaId) : "";
    setPageTitle("研报中心");
    if (mediaId && !$("#ima-reader-page")) {
      $("#main").innerHTML = `<div class="admin-skeleton" aria-hidden="true"></div>`;
    }
    if (!mediaId) mountKnowledgeListShell();
    const requests = prefetched || prefetchKnowledge(mediaId);
    const documentsPromise = requests.documents;
    const documentsRenderTask = mediaId
      ? null
      : renderImaDocuments(seq, { prefetched: documentsPromise });
    try {
      const settled = await requests.settled;
      if (!routeStillActive(seq)) return;
      const catalogResult = settled[0];
      const documentsResult = documentsPromise ? settled[1] : null;
      const catalogOk = catalogResult.status === "fulfilled";
      const documentsOk = documentsResult?.status === "fulfilled";
      const snapshot = currentImaListSnapshot();
      if (!catalogOk && !documentsOk && !mediaId && !snapshot) {
        const message = catalogResult.reason?.message || documentsResult?.reason?.message || "请求失败";
        $("#main").innerHTML = emptyState(`加载失败：${message}`, `<div><button type="button" class="btn-normal" onclick="refreshKnowledge()">重试</button></div>`);
        return;
      }
      let subscribed = [];
      let available = [];
      let catalogWarning = "";
      if (catalogOk) {
        const data = catalogResult.value;
        subscribed = Array.isArray(data.subscribed) ? data.subscribed : [];
        available = Array.isArray(data.available) ? data.available : [];
      } else if (documentsOk) {
        const groups = Array.isArray(documentsResult.value.groups) ? documentsResult.value.groups : [];
        subscribed = groups.map((group) => ({ id: group.id, name: group.name, enabled: true }));
        catalogWarning = "研报中心目录加载失败";
      } else {
        subscribed = Array.isArray(state.imaCatalogSubscribed) ? state.imaCatalogSubscribed : [];
        available = Array.isArray(state.imaCatalogAvailable) ? state.imaCatalogAvailable : [];
        catalogWarning = "研报中心目录加载失败";
      }
      state.imaCatalogSubscribed = subscribed;
      state.imaCatalogAvailable = available;
      const isAdmin = !!state.user?.is_admin;
      const selectedGroup = imaDocumentsGroupFromRoute();
      const sourceControl = $("#kb-list .ima-report-source");
      if (sourceControl) {
        sourceControl.outerHTML = knowledgeSourceControlsHtml(selectedGroup);
      }
      if (catalogOk && selectedGroup && !isAdmin && !subscribed.some((group) => String(group.id) === selectedGroup)) {
        setPageTitle("研报中心", true, "knowledge", "回研报中心");
        $("#main").innerHTML = emptyState("没有访问权限", `<div><button type="button" class="btn-normal" onclick="go('knowledge')">回研报中心</button></div>`);
        return;
      }
      state.imaDocumentsGroup = selectedGroup;
      if (mediaId) {
        mountKnowledgeReaderShell();
        await renderImaDocument(seq, mediaId);
        return;
      }
      if (!subscribed.length && catalogOk) {
        const list = $("#kb-list");
        const controls = `<div class="ima-report-head"><div class="ima-report-filters ima-report-filters-row">${knowledgeSourceControlsHtml("")}</div></div>`;
        if (isAdmin) {
          list.innerHTML = `${controls}${emptyState("还没有配置研报库", `<div><button type="button" class="btn-normal" onclick="go('admin/knowledge')">去配置采集</button></div>`)}`;
        } else {
          list.innerHTML = `${controls}${emptyState(
            "还没有可看的研报库",
            available.length
              ? `<div class="kb-lib-empty">${available.map((group) => knowledgeLibRowHtml(group, "", "available")).join("")}</div>`
              : `<div><p class="section-meta">找管理员在用户设置里勾选研报库后再来</p></div>`
          )}`;
        }
        if (catalogWarning) {
          list.insertAdjacentHTML("afterbegin", `<p class="section-meta ima-catalog-warning">${escapeHtml(catalogWarning)}</p>`);
        }
        return;
      }
      await documentsRenderTask;
      if (catalogWarning) {
        const warning = `<p class="section-meta ima-catalog-warning">${escapeHtml(catalogWarning)}</p>`;
        const head = $("#kb-list .ima-report-head");
        if (head) head.insertAdjacentHTML("afterend", warning);
        else $("#kb-list")?.insertAdjacentHTML("afterbegin", warning);
      }
    } catch (err) {
      if (routeStillActive(seq)) {
        $("#main").innerHTML = emptyState(`加载失败：${err.message}`, `<div><button type="button" class="btn-normal" onclick="refreshKnowledge()">重试</button></div>`);
      }
    }
  }

  async function renderImaDocuments(seq, { keepOld = false, prefetched = null } = {}) {
    stopImaDocumentsAutoLoad();
    if (!$("#ima-report-page")) {
      await renderKnowledge(seq);
      return;
    }
    const previousBody = $("#ima-docs-body");
    const oldHtml = keepOld ? previousBody?.innerHTML || "" : "";
    _imaListSeq += 1;
    _imaLoadingMore = false;
    if (!keepOld) {
      _imaItems.length = 0;
      state.imaDocumentsHasMore = false;
    }
    const selectedGroup = imaDocumentsGroupFromRoute() || state.imaDocumentsGroup || "";
    const query = imaUsableSearchQuery(routeQuery().get("q") || "");
    const day = routeQuery().get("day") || "";
    const tag = routeQuery().get("tag") || "";
    const searchMode = !!(query || tag);
    const streamMode = !day && !searchMode;
    const paged = searchMode || streamMode;
    state.imaDocumentsGroup = selectedGroup;
    state.imaDocumentsQuery = query;
    state.imaDocumentsDay = day;
    state.imaDocumentsTag = tag;
    if (!knowledgeMediaIdFromPath()) setPageTitle("研报中心");
    const listRoot = $("#kb-list");
    if (!listRoot) {
      await renderKnowledge(seq);
      return;
    }
    const existingHead = listRoot.querySelector(".ima-report-head");
    if (existingHead) {
      const input = $("#ima-doc-q");
      if (input && document.activeElement !== input) input.value = query;
      const source = existingHead.querySelector(".ima-report-source");
      if (source) source.outerHTML = knowledgeSourceControlsHtml(selectedGroup);
      let clearBtn = existingHead.querySelector(".ima-search-clear");
      if (query) {
        if (!clearBtn) {
          const searchBox = existingHead.querySelector(".ima-report-searchbox");
          if (searchBox) {
            searchBox.insertAdjacentHTML("beforeend", `<button type="button" class="ima-search-clear" onclick="clearImaDocumentsFilter('q')" aria-label="清除搜索">${X_ICON}</button>`);
          }
        }
      } else if (clearBtn) {
        clearBtn.remove();
      }
      const body = $("#ima-docs-body");
      if (body && !keepOld) body.innerHTML = imaReportSkeletonHtml();
    } else {
      _imaSearchComposing = false;
      const sourceControls = knowledgeSourceControlsHtml(selectedGroup);
      const clearBtn = query
        ? `<button type="button" class="ima-search-clear" onclick="clearImaDocumentsFilter('q')" aria-label="清除搜索">${X_ICON}</button>`
        : "";
      listRoot.innerHTML = `
    <header class="ima-report-head">
      <div class="ima-report-heading"><div><h2 id="ima-doc-title">最新研报</h2><p id="ima-doc-meta" class="section-meta"></p></div><button type="button" class="icon-btn" aria-label="刷新研报" title="刷新研报" onclick="refreshImaDocuments()">${REFRESH_ICON}</button></div>
      <form class="ima-report-search" onsubmit="event.preventDefault();submitImaDocumentsSearch()">
        <label class="ima-report-searchbox">${SEARCH_ICON}<input id="ima-doc-q" type="search" value="${escapeHtml(query)}" placeholder="搜标题、公司、代码、行业或资料源" aria-label="搜索研报" oninput="queueImaDocumentsSearch()" oncompositionstart="_imaSearchComposing=true" oncompositionend="_imaSearchComposing=false;queueImaDocumentsSearch()">${clearBtn}</label>
      </form>
      <div class="ima-report-filters">${sourceControls}<span id="ima-doc-day-nav-slot"></span><div class="ima-report-tag"><span class="sr-only">标签</span><button type="button" class="kb-desk-day ima-tag-trigger" id="ima-doc-tag" aria-haspopup="listbox" aria-expanded="false" onclick="toggleImaTagMenu(event)" hidden><span class="ima-tag-label">标签</span>${CHEVRON_DOWN_ICON}</button></div></div>
      <div id="ima-doc-filter-chips" class="ima-doc-filter-chips"></div>
      <div class="ima-report-columns" aria-hidden="true"><span>日期</span><span>标题</span><span>资料源</span></div>
    </header>
    <div id="ima-docs-body" class="ima-report-body">${keepOld && oldHtml ? oldHtml : imaReportSkeletonHtml()}</div>`;
    }
    const body = $("#ima-docs-body");
    adoptImaStreamSnapshot();
    const snapshot = currentImaListSnapshot();
    if (snapshot && body) {
      const tagTrigger = $("#ima-doc-tag");
      const uniqueTags = Object.keys(snapshot.tagCounts || {});
      if (tag && !uniqueTags.includes(tag)) uniqueTags.unshift(tag);
      if (tagTrigger) {
        const tagLabel = tagTrigger.querySelector(".ima-tag-label");
        if (tagLabel) tagLabel.textContent = tag || "标签";
        if (uniqueTags.length || tag) tagTrigger.removeAttribute("hidden");
        else tagTrigger.hidden = true;
      }
      const navSlot = $("#ima-doc-day-nav-slot");
      if (navSlot) {
        closeImaDayPicker();
        navSlot.innerHTML = imaDocumentsDayNavHtml(searchMode ? "" : day, snapshot.days);
      }
      restoreImaListSnapshot(snapshot, body);
      const title = $("#ima-doc-title");
      const meta = $("#ima-doc-meta");
      if (title) title.textContent = "最新研报";
      if (meta) {
        meta.textContent = imaDocumentsCountLabel(
          !!(query || tag),
          snapshot.documentCount,
          snapshot.items.length,
          snapshot.hasMore,
        );
      }
      syncImaDocumentsFilterStatus();
      $("#ima-report-page")?.removeAttribute("aria-busy");
      return;
    }
    $("#ima-report-page")?.setAttribute("aria-busy", "true");
    try {
      const data = prefetched != null ? await prefetched : await api(imaDocumentsRequestPath());
      if (!routeStillActive(seq)) return;
      $("#ima-report-page")?.removeAttribute("aria-busy");
      const groups = Array.isArray(data.groups) ? data.groups : [];
      const items = Array.isArray(data.items) ? data.items : [];
      const selectedGroupInfo = groups.find((group) => String(group.id || "") === selectedGroup);
      const selectedGroupName = selectedGroupInfo?.name || (selectedGroup ? selectedGroup : "全部");
      const title = $("#ima-doc-title");
      if (title) title.textContent = "最新研报";
      if (!knowledgeMediaIdFromPath()) setPageTitle(feishuSourceDisplay(selectedGroupName).label);
      applyImaListFacets(data, items.length, !!data.has_more);
      const hasFilter = !!(query || tag);
      _imaItems.length = 0;
      _imaItems.push(...items);
      state.imaDocumentsHasMore = !!(paged && data.has_more);
      void refreshImaListFacets(seq);
      const body = $("#ima-docs-body");
      if (!items.length) {
        body.innerHTML = imaDocumentsEmptyHtml(hasFilter);
        return;
      }
      const more = state.imaDocumentsHasMore
        ? `<div id="ima-docs-more" class="ima-docs-more" role="status" aria-live="polite">下滑加载更多</div>`
        : "";
      body.innerHTML = `<div class="ima-doc-list">${items.map((item) => imaDocumentRow(item)).join("")}</div>${more}`;
      startImaDocumentsAutoLoad();
    } catch (err) {
      if (!routeStillActive(seq)) return;
      const body = $("#ima-docs-body");
      $("#ima-report-page")?.removeAttribute("aria-busy");
      if (keepOld && oldHtml && body) {
        body.innerHTML = oldHtml;
        body.insertAdjacentHTML("afterbegin", imaReportRefreshErrorHtml(err.message || "请求失败"));
        return;
      }
      const denied = String(err.message || "").includes("知识库不存在");
      body.innerHTML = denied
        ? emptyState("没有访问权限", `<div><button type="button" class="btn-normal" onclick="go('knowledge')">回研报中心</button></div>`)
        : emptyState(`加载失败：${err.message}`, `<div><button type="button" class="btn-normal" onclick="refreshImaDocuments()">重试</button></div>`);
    }
  }

  function applyImaListFacets(data, itemCount, hasMore) {
    const params = routeQuery();
    const query = imaUsableSearchQuery(params.get("q") || "");
    const tag = params.get("tag") || "";
    const day = params.get("day") || "";
    const searchMode = !!(query || tag);
    const days = Array.isArray(data.days)
      ? data.days.filter(Boolean)
      : [...new Set(_imaItems.map((item) => item.day).filter(Boolean))];
    state.imaDocumentsDays = days;
    _imaTagCounts = imaTagCountsFromData(data);
    _imaDocumentCount = Number(data.document_count) || imaTagCoverageBase(_imaTagCounts, 0);
    const uniqueTags = Array.isArray(data.tags) ? data.tags.filter(Boolean) : Object.keys(_imaTagCounts);
    if (tag && !uniqueTags.includes(tag)) uniqueTags.unshift(tag);
    const tagTrigger = $("#ima-doc-tag");
    if (tagTrigger) {
      const tagLabel = tagTrigger.querySelector(".ima-tag-label");
      if (tagLabel) tagLabel.textContent = tag || "标签";
      if (uniqueTags.length || tag) tagTrigger.removeAttribute("hidden");
      else tagTrigger.hidden = true;
    }
    const navSlot = $("#ima-doc-day-nav-slot");
    if (navSlot) {
      closeImaDayPicker();
      navSlot.innerHTML = imaDocumentsDayNavHtml(searchMode ? "" : day, days);
    }
    const meta = $("#ima-doc-meta");
    if (meta) {
      meta.textContent = imaDocumentsCountLabel(searchMode, data.document_count, itemCount, hasMore);
    }
    syncImaDocumentsFilterStatus();
  }

  // 列表已画完才去补分面（计数/日期/标签）；失败仅影响这几个显示，列表照常可用
  async function refreshImaListFacets(seq) {
    if (!imaFacetsDeferred()) return;
    try {
      const data = await api(imaDocumentsRequestPath({ facetsOnly: true }));
      if (!routeStillActive(seq)) return;
      applyImaListFacets(data, _imaItems.length, state.imaDocumentsHasMore);
    } catch (err) {
      console.warn("ima facets refresh failed", err);
    }
  }

  function imaDocumentsFilterChipsHtml() {
    const chips = [];
    if (state.imaDocumentsQuery) {
      chips.push(`<span class="ima-doc-filter-chip">搜索 ${escapeHtml(state.imaDocumentsQuery)}<button type="button" onclick="clearImaDocumentsFilter('q')" aria-label="清除搜索">${X_ICON}</button></span>`);
    }
    if (state.imaDocumentsTag) {
      chips.push(`<span class="ima-doc-filter-chip">${escapeHtml(state.imaDocumentsTag)}<button type="button" onclick="clearImaDocumentsFilter('tag')" aria-label="清除标签">${X_ICON}</button></span>`);
    }
    if (!chips.length) return "";
    chips.push(`<button type="button" class="btn-ghost" onclick="clearImaDocumentsFilters()">清除筛选</button>`);
    return chips.join("");
  }

  function syncImaDocumentsFilterStatus() {
    const chips = $("#ima-doc-filter-chips");
    if (chips) chips.innerHTML = imaDocumentsFilterChipsHtml();
  }

  function clearImaDocumentsFilter(kind) {
    if (kind === "q") {
      state.imaDocumentsQuery = "";
      const input = $("#ima-doc-q");
      if (input) input.value = "";
    }
    if (kind === "day") state.imaDocumentsDay = "";
    if (kind === "tag") state.imaDocumentsTag = "";
    if (kind !== "day") state.imaDocumentsDay = "";
    replaceImaDocumentsRoute(imaDocumentsRoute(state.imaDocumentsGroup, state.imaDocumentsQuery, state.imaDocumentsDay, state.imaDocumentsTag));
    renderImaDocuments(bumpRouteSeq());
  }

  function clearImaDocumentsFilters() {
    state.imaDocumentsQuery = "";
    state.imaDocumentsTag = "";
    state.imaDocumentsDay = "";
    const input = $("#ima-doc-q");
    if (input) input.value = "";
    replaceImaDocumentsRoute(imaDocumentsRoute(state.imaDocumentsGroup, "", state.imaDocumentsDay, ""));
    renderImaDocuments(bumpRouteSeq());
  }

  function stopImaDocumentsAutoLoad() {
    _imaDocsLoadObserver?.disconnect();
    _imaDocsLoadObserver = null;
    if (_imaDocsLoadFallback) {
      const body = $("#ima-docs-body");
      body?.removeEventListener("scroll", _imaDocsLoadFallback);
      _imaDocsLoadFallback = null;
    }
    clearTimeout(_imaSearchTimer);
    _imaSearchTimer = null;
    _imaLoadingMore = false;
  }

  function startImaDocumentsAutoLoad() {
    stopImaDocumentsAutoLoad();
    const body = $("#ima-docs-body");
    const sentinel = $("#ima-docs-more");
    if (!body || !sentinel || !state.imaDocumentsHasMore) return;
    const load = () => loadImaDocumentsMore();
    if ("IntersectionObserver" in window) {
      _imaDocsLoadObserver = new IntersectionObserver((entries) => {
        if (entries.some((entry) => entry.isIntersecting)) load();
      }, { root: body, rootMargin: "400px 0px" });
      _imaDocsLoadObserver.observe(sentinel);
      return;
    }
    _imaDocsLoadFallback = () => {
      if (body.scrollTop + body.clientHeight >= body.scrollHeight - 400) load();
    };
    body.addEventListener("scroll", _imaDocsLoadFallback, { passive: true });
    _imaDocsLoadFallback();
  }

  async function loadImaDocumentsMore() {
    if (_imaLoadingMore || !state.imaDocumentsHasMore) return;
    if (state.imaDocumentsDay && !state.imaDocumentsQuery && !state.imaDocumentsTag) return;
    _imaLoadingMore = true;
    const moreBtn = $("#ima-docs-more");
    if (moreBtn) {
      moreBtn.textContent = "正在加载更多…";
      moreBtn.setAttribute("aria-busy", "true");
    }
    const seq = currentRouteSeq();
    const listSeq = _imaListSeq;
    try {
      const params = new URLSearchParams();
      if (state.imaDocumentsQuery) params.set("q", state.imaDocumentsQuery);
      if (state.imaDocumentsTag) params.set("tag", state.imaDocumentsTag);
      if (state.imaDocumentsGroup) params.set("group", state.imaDocumentsGroup);
      params.set("limit", "50");
      params.set("offset", String(_imaItems.length));
      const data = await api(`/api/ima-documents?${params.toString()}`);
      if (listSeq !== _imaListSeq || !routeStillActive(seq)) return;
      const incoming = Array.isArray(data.items) ? data.items : [];
      _imaItems.push(...incoming);
      state.imaDocumentsHasMore = !!data.has_more && incoming.length > 0;
      const body = $("#ima-docs-body");
      const list = body?.querySelector(".ima-doc-list");
      if (list && incoming.length) {
        list.insertAdjacentHTML("beforeend", incoming.map((item) => imaDocumentRow(item)).join(""));
      }
      const btn = $("#ima-docs-more");
      if (!state.imaDocumentsHasMore) {
        btn?.remove();
      } else if (btn) {
        btn.textContent = "下滑加载更多";
        btn.removeAttribute("aria-busy");
      }
      startImaDocumentsAutoLoad();
      const meta = $("#ima-doc-meta");
      if (meta) {
        meta.textContent = imaDocumentsCountLabel(
          !!(state.imaDocumentsQuery || state.imaDocumentsTag),
          _imaDocumentCount,
          _imaItems.length,
          state.imaDocumentsHasMore,
        );
      }
    } catch (err) {
      if (listSeq !== _imaListSeq || !routeStillActive(seq)) return;
      const failed = $("#ima-docs-more");
      if (failed) {
        failed.removeAttribute("aria-busy");
        failed.innerHTML = `<button type="button" class="btn-ghost" onclick="loadImaDocumentsMore()">加载失败，重试</button>`;
      }
    } finally {
      if (listSeq === _imaListSeq) _imaLoadingMore = false;
    }
  }

  function backFromImaReader(fallbackRoute, focusSearch = false) {
    clearImaPdfUrl();
    const snapshot = _imaListSnapshot;
    // ponytail: history.back 只在「列表 push 进阅读器」时安全；replace 进入（飞书组直开/j-k/上下篇）时
    // 列表条目已被顶掉，back 会跑偏到广场，改走 go() 由快照恢复列表
    if (snapshot && snapshot.selectedKey && !snapshot.replaced && snapshot.route === normalizeRoute(fallbackRoute)) {
      if (focusSearch) snapshot.focusSearch = true;
      history.back();
      return;
    }
    go(fallbackRoute);
  }

  function imaReaderNavHtml(mediaId, groupId = "", snapshot = null) {
    if (!snapshot || snapshot.items.length < 2) return "";
    const current = imaDocumentKey(mediaId, groupId);
    const index = snapshot.items.findIndex((item) => imaDocumentKey(item.media_id, item.group_id) === current);
    if (index < 0) return "";
    const prev = snapshot.items[index - 1];
    const next = snapshot.items[index + 1];
    const button = (item, className, label) => item
      ? `<button type="button" class="${className}" data-media-id="${escapeHtml(item.media_id)}" data-group-id="${escapeHtml(item.group_id || "")}" aria-label="${label}" title="${label}" onclick="openImaDocument(this.dataset.mediaId, this.dataset.groupId, true)">${className === "ima-reader-prev" ? CHEVRON_LEFT_ICON : ""}${label} <span>${escapeHtml(imaListTitle(item.name))}</span>${className === "ima-reader-next" ? CHEVRON_RIGHT_ICON : ""}</button>`
      : "";
    return `<nav class="ima-reader-nav" aria-label="同一结果集">${button(prev, "ima-reader-prev", "上一份")}${button(next, "ima-reader-next", "下一份")}</nav>`;
  }

  async function renderImaDocument(seq, mediaId) {
    if (!$("#kb-reader")) {
      await renderKnowledge(seq, encodeURIComponent(mediaId));
      return;
    }
    const readerSeq = bumpImaReaderSeq();
    clearImaPdfUrl();
    const currentQuery = routeQuery();
    const listGroup = currentQuery.get("group") || state.imaDocumentsGroup || "";
    const documentGroup = currentQuery.get("doc_group") || listGroup;
    const query = currentQuery.get("q") || state.imaDocumentsQuery || "";
    const day = currentQuery.get("day") || state.imaDocumentsDay || "";
    const tag = currentQuery.get("tag") || state.imaDocumentsTag || "";
    state.imaDocumentsTag = tag;
    const groupQuery = documentGroup ? `?group=${encodeURIComponent(documentGroup)}` : "";
    let backRoute = imaDocumentsRoute(listGroup, query, day, tag);
    setPageTitle("研报中心");
    $("#kb-reader").innerHTML = `<div class="admin-skeleton" aria-hidden="true"></div>`;
    try {
      const item = await api(`/api/ima-documents/${encodeURIComponent(mediaId)}${groupQuery}`);
      if (!currentQuery.has("group") && !currentQuery.has("doc_group") && item.group_id) {
        backRoute = imaDocumentsRoute(item.group_id, query, day, tag);
      }
      if (!routeStillActive(seq) || readerSeq !== currentImaReaderSeq()) return;
      const isFeishuTimeline = item.type === "feishu_timeline";
      setPageTitle(
        isFeishuTimeline
          ? feishuSourceDisplay(item.group_name || item.name).label
          : (item.group_name || $("#ima-doc-title")?.textContent || "研报库")
      );
      const ticker = isFeishuTimeline ? "" : imaDocTicker(item.name);
      const tickerMeta = ticker ? `<span class="ima-reader-meta-item">${escapeHtml(ticker)}</span>` : "";
      const dayContext = !isFeishuTimeline && (item.sort_date || item.day)
        ? `<span class="ima-reader-day ima-reader-meta-item">${escapeHtml(fmtImaDay(item.sort_date || item.day))}</span>`
        : "";
      const abstractText = isFeishuTimeline ? "" : (item.abstract_zh || item.abstract || "");
      const abstractLong = abstractText.length > IMA_ABSTRACT_CLAMP_CHARS;
      const abstractMore = abstractLong
        ? `<button type="button" class="ima-abstract-more" aria-expanded="false" onclick="toggleImaAbstract(this)">展开</button>`
        : "";
      const abstractHtml = abstractText
        ? `<details open class="ima-reader-abstract${abstractLong ? " is-clamped" : ""}"><summary><span>摘要</span></summary><p id="ima-reader-abstract">${escapeHtml(abstractText)}</p>${abstractMore}</details>`
        : "";
      // 快照路由校验（与 currentImaListSnapshot 同思路）：与本次应返回的列表路由不匹配的旧快照不用于导航/计数
      const listSnapshot = _imaListSnapshot && _imaListSnapshot.route === normalizeRoute(backRoute) ? _imaListSnapshot : null;
      const standalonePwa = isStandalonePwa();
      const openLabel = standalonePwa ? "打开 PDF" : "新标签打开 PDF";
      const openNewTab = isFeishuTimeline
        ? `<a class="icon-btn" data-feishu-canonical="1" href="${escapeHtml(item.source_url || "")}" ${item.source_url ? "" : "hidden"} target="_blank" rel="noopener" aria-label="打开飞书原文" title="打开飞书原文">${EXTERNAL_LINK_ICON}</a>`
        : item.has_pdf
          ? `<button type="button" class="icon-btn" aria-label="${openLabel}" title="${openLabel}" onclick="openImaPdfNewTab()">${EXTERNAL_LINK_ICON}</button>`
          : "";
      const documentPanel = isFeishuTimeline
        ? `<div id="ima-document-panel" class="feishu-timeline-panel" aria-busy="true"><p class="ima-reader-status" role="status">正在载入时间线…</p></div>`
        : item.has_pdf
          ? `<div id="ima-pdf-panel" class="ima-pdf-panel" aria-busy="true"><p class="ima-reader-status" role="status">正在打开预览…</p>${standalonePwa
              ? `<button id="ima-pdf-pwa-open" type="button" class="btn-normal" onclick="openImaPdfNewTab()" hidden>打开 PDF</button>`
              : `<iframe id="ima-pdf-frame" title="PDF 预览" hidden style="position:absolute;inset:0;width:100%;height:100%;border:0"></iframe>`}</div>`
          : `<div class="ima-pdf-panel"><div class="ima-reader-empty" role="status"><p>还没有预览文件</p></div></div>`;
      const sizeLine = isFeishuTimeline ? "" : fmtDocSize(item.size);
      const sizeMeta = sizeLine ? `<span class="ima-reader-meta-item">${escapeHtml(sizeLine)}</span>` : "";
      const fileMetaHtml = (tickerMeta || dayContext || sizeMeta)
        ? `<div class="ima-reader-filemeta">${tickerMeta}${dayContext}${sizeMeta}</div>`
        : "";
      const readerTitle = escapeHtml(isFeishuTimeline ? feishuSourceDisplay(item.name).label : imaDisplayTitle(item.name));
      const fromSearch = !!(query || tag);
      const searchBack = fromSearch
        ? `<button type="button" class="icon-btn" aria-label="返回搜索" data-back="${escapeHtml(backRoute)}" onclick="backFromImaReader(this.dataset.back, true)">${SEARCH_ICON}</button>`
        : "";
      $("#kb-reader").innerHTML = isFeishuTimeline
        ? `
        <article class="ima-reader ima-reader--feishu">
          <header class="ima-reader-toolbar">
            <button type="button" class="ima-reader-back" data-back="${escapeHtml(backRoute)}" onclick="backFromImaReader(this.dataset.back)" aria-label="返回" title="返回"><span class="ima-back-icon" aria-hidden="true">${CHEVRON_LEFT_ICON}</span>返回</button>
            <h2 class="ima-reader-title">${readerTitle}</h2>
            <div class="ima-reader-actions">${searchBack}${openNewTab}</div>
          </header>
          <div id="feishu-timeline-toolbar" class="feishu-timeline-toolbar"></div>
          ${documentPanel}
        </article>`
        : `
        <article class="ima-reader">
          <header class="ima-reader-toolbar">
            <button type="button" class="ima-reader-back" data-back="${escapeHtml(backRoute)}" onclick="backFromImaReader(this.dataset.back)" aria-label="返回" title="返回"><span class="ima-back-icon" aria-hidden="true">${CHEVRON_LEFT_ICON}</span>返回</button>
            <div class="ima-reader-actions">${searchBack}${openNewTab}</div>
          </header>
          <section class="ima-reader-info">
            <h2 class="ima-reader-title">${readerTitle}</h2>
            ${fileMetaHtml}
            ${imaReaderWatchHtml(item.tags)}
            ${abstractHtml}
          </section>
          ${documentPanel}
          ${imaReaderNavHtml(mediaId, item.group_id || documentGroup, listSnapshot)}
        </article>`;
      if (isFeishuTimeline) await loadFeishuTimeline(item, seq, readerSeq);
      else if (item.has_pdf) loadImaPdf(mediaId, readerSeq);
      if (item.needs_translation) {
        try {
          const translated = await api(`/api/ima-documents/${encodeURIComponent(mediaId)}/translate${groupQuery}`, { method: "POST" });
          if (!routeStillActive(seq) || readerSeq !== currentImaReaderSeq()) return;
          const zh = translated && translated.abstract_zh;
          const el = $("#ima-reader-abstract");
          if (el && zh) el.textContent = zh;
        } catch {
          /* keep original abstract */
        }
      }
    } catch (err) {
      if (!routeStillActive(seq) || readerSeq !== currentImaReaderSeq()) return;
      const denied = String(err.message || "").includes("知识库不存在");
      $("#kb-reader").innerHTML = denied
        ? emptyState("没有访问权限", `<div><button type="button" class="btn-normal" data-ima-back="${escapeHtml(backRoute)}">回研报中心</button></div>`)
        : emptyState(`文档加载失败：${err.message}`, `<div><button type="button" class="btn-normal" data-ima-back="${escapeHtml(backRoute)}">返回文档列表</button></div>`);
    }
  }

  function showImaPdfFail(mediaId, seq, readerSeq) {
    if (!routeStillActive(seq) || readerSeq !== currentImaReaderSeq()) return;
    const panel = $("#ima-pdf-panel");
    if (!panel) return;
    clearImaPdfUrl();
    panel.hidden = false;
    panel.removeAttribute("aria-busy");
    panel.innerHTML = `<div class="ima-reader-empty" role="status"><p>预览打不开</p></div>`;
  }

  async function loadImaPdf(mediaId, readerSeq) {
    const seq = currentRouteSeq();
    const group = imaReaderDocumentGroup();
    const groupQuery = group ? `?group=${encodeURIComponent(group)}` : "";
    if (_imaPdfAbort) _imaPdfAbort.abort();
    const abort = new AbortController();
    _imaPdfAbort = abort;
    try {
      const blob = await apiBlob(`/api/ima-documents/${encodeURIComponent(mediaId)}/pdf${groupQuery}`, { signal: abort.signal });
      if (abort.signal.aborted) return;
      if (!routeStillActive(seq) || readerSeq !== currentImaReaderSeq()) return;
      const head = blob.size ? await blob.slice(0, 5).text() : "";
      if (abort.signal.aborted) return;
      if (!routeStillActive(seq) || readerSeq !== currentImaReaderSeq()) return;
      if (blob.size < 64 || head !== "%PDF-") {
        showImaPdfFail(mediaId, seq, readerSeq);
        return;
      }
      if (window._imaPdfUrl) URL.revokeObjectURL(window._imaPdfUrl);
      window._imaPdfUrl = URL.createObjectURL(blob);
      const frame = $("#ima-pdf-frame");
      const panel = $("#ima-pdf-panel");
      const pwaOpen = $("#ima-pdf-pwa-open");
      if (panel && (frame || pwaOpen)) {
        const status = panel.querySelector(".ima-reader-status");
        if (status) status.remove();
        panel.hidden = false;
        panel.removeAttribute("aria-busy");
        if (pwaOpen) {
          pwaOpen.hidden = false;
        } else if (frame) {
          frame.src = `${window._imaPdfUrl}#view=FitH&zoom=page-width`;
          frame.hidden = false;
          frame.addEventListener("error", () => showImaPdfFail(mediaId, seq, readerSeq), { once: true });
        }
      }
    } catch (err) {
      if (err && err.name === "AbortError") return;
      if (routeStillActive(seq) && readerSeq === currentImaReaderSeq()) {
        const message = String(err.message || "");
        if (message.includes("频繁") || message.includes("上限")) flash(message, "error");
        showImaPdfFail(mediaId, seq, readerSeq);
      }
    }
  }

  function openImaPdfNewTab() {
    if (!window._imaPdfUrl) {
      flash("PDF 还没加载好，稍后再试", "error");
      return;
    }
    if (isStandalonePwa()) {
      window.location.assign(window._imaPdfUrl);
      return;
    }
    window.open(window._imaPdfUrl, "_blank", "noopener");
  }

  async function downloadImaPdf(mediaId) {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const group = imaReaderDocumentGroup();
    const groupQuery = group ? `&group=${encodeURIComponent(group)}` : "";
    const detailQuery = group ? `?group=${encodeURIComponent(group)}` : "";
    try {
      const [blob, item] = await Promise.all([
        apiBlob(`/api/ima-documents/${encodeURIComponent(mediaId)}/pdf?download=1${groupQuery}`),
        api(`/api/ima-documents/${encodeURIComponent(mediaId)}${detailQuery}`),
      ]);
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = item.name || "ima-document.pdf";
      document.body.appendChild(link);
      link.click();
      link.remove();
      setTimeout(() => {
        if (sessionOwnerStillActive(routeSeq, token, sessionGeneration)) URL.revokeObjectURL(url);
      }, 1000);
    } catch (err) {
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash(`PDF 下载失败：${err.message}`, "error");
    }
  }

  return {
    clearImaPdfUrl,
    renderTickerPage,
    _imaDocumentRoute,
    imaDocumentReaderRoute,
    imaReaderDocumentGroup,
    openImaDocument,
    knowledgeMediaIdFromPath,
    knowledgeTypingTarget,
    onKnowledgeListKey,
    ensureKnowledgeKeys,
    mountKnowledgeListShell,
    mountKnowledgeReaderShell,
    imaCountText,
    imaResolvedCount,
    imaDocumentsCountLabel,
    imaSnapshotIsFiltered,
    toggleImaAbstract,
    fmtImaDay,
    fmtImaDayShort,
    imaDisplayTitle,
    imaDocTicker,
    imaListTitle,
    fmtDocSize,
    imaDocKindLabel,
    imaDocumentTagsHtml,
    imaReaderWatchHtml,
    imaTagCountsFromData,
    imaTagCoverageBase,
    imaDistinctiveTags,
    imaReportMetaHtml,
    imaDocumentRow,
    imaReportSkeletonHtml,
    imaDocumentsEmptyHtml,
    imaDocumentGroups,
    imaDocumentsGroupFromRoute,
    imaDocumentsRequestPath,
    imaDocumentsRoute,
    imaDocumentKey,
    cloneImaListSnapshotFields,
    captureImaListSnapshot,
    stashImaStreamSnapshot,
    adoptImaStreamSnapshot,
    currentImaListSnapshot,
    restoreImaListSnapshot,
    replaceImaDocumentsRoute,
    selectImaDocumentGroup,
    imaUsableSearchQuery,
    queueImaDocumentsSearch,
    submitImaDocumentsSearch,
    selectImaDocumentsDay,
    selectImaDocumentsTag,
    imaDayMenuDays,
    imaDayMenuHtml,
    placeImaDayMenu,
    closeImaDayPicker,
    renderImaDayMenu,
    toggleImaDayPicker,
    pickImaDay,
    closeImaTagMenu,
    imaTagMenuHtml,
    toggleImaTagMenu,
    pickImaTag,
    onImaDayPickerDocDown,
    imaDocumentsDayNavHtml,
    refreshImaDocuments,
    imaReportRefreshErrorHtml,
    knowledgeCardSummary,
    knowledgeLibRowHtml,
    knowledgeSourceControlsHtml,
    refreshKnowledge,
    subscribeKnowledge,
    unsubscribeKnowledge,
    prefetchKnowledge,
    renderKnowledge,
    renderImaDocuments,
    imaDocumentsFilterChipsHtml,
    syncImaDocumentsFilterStatus,
    clearImaDocumentsFilter,
    clearImaDocumentsFilters,
    stopImaDocumentsAutoLoad,
    startImaDocumentsAutoLoad,
    loadImaDocumentsMore,
    backFromImaReader,
    imaReaderNavHtml,
    renderImaDocument,
    showImaPdfFail,
    loadImaPdf,
    openImaPdfNewTab,
    downloadImaPdf,
  };
}
