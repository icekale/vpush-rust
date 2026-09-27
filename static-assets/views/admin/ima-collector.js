export function createAdminImaCollectorView(dependencies) {
  const {
    $,
    state,
    api,
    flash,
    escapeHtml,
    emptyState,
    routeStillActive,
    sessionOwnerStillActive,
    currentRouteSeq,
    bumpRouteSeq,
    currentAdminSeq,
    FOLDER_ICON,
    CHEVRON_RIGHT_ICON,
    CHEVRON_DOWN_ICON,
    X_ICON,
    imaMountState,
    imaCollectorPureCache,
    reloadAdminSettingsPage,
    isAdminSettingsPath,
    currentLocalLibraries,
    focusCookieField,
  } = dependencies;

  let imaProgressTimer = null;
  let imaProgressPollSeq = 0;

  function imaMountCacheKey(groupId, parentId) {
    return `${groupId}\0${parentId}`;
  }

  function imaMountGroup(groupId) {
    return imaMountState.groups.find((group) => String(group.id || "") === String(groupId || "")) || null;
  }

  function imaMountDraft(groupId) {
    let draft = imaMountState.drafts.get(groupId);
    if (!draft) {
      draft = new Set();
      imaMountState.drafts.set(groupId, draft);
    }
    return draft;
  }

  function imaGroupIntervalSeconds(group) {
    const sec = Number(group?.interval_seconds || 3600);
    if (sec < 10800) return 3600;
    if (sec < 43200) return 21600;
    return 86400;
  }

  function initImaMountState(groups, preserve = false) {
    const oldDrafts = imaMountState.drafts;
    const oldSelected = imaMountState.selectedGroupId;
    const oldDirty = imaMountState.dirty;
    const oldGroupsById = new Map(
      imaMountState.groups.map((group) => [String(group.id), group]),
    );
    imaMountState.groups = (Array.isArray(groups) ? groups.filter((group) => group && group.id) : []).map((group) => {
      const previous = oldGroupsById.get(String(group.id));
      const incoming = Number(group.interval_seconds);
      const fallback = previous ? Number(previous.interval_seconds) || 3600 : 3600;
      return {
        ...group,
        interval_seconds: Number.isFinite(incoming) && incoming > 0 ? incoming : fallback,
      };
    });
    imaMountState.selectedGroupId = oldSelected;
    imaMountState.drafts = new Map();
    imaMountState.folders = new Map();
    imaMountState.parents = new Map();
    imaMountState.expanded = new Set();
    imaMountState.loading = new Set();
    imaMountState.errors = new Map();
    imaMountState.folderRequests = new Map();
    imaMountState.discoveryBusy = false;
    imaMountState.discoveryOwner = null;
    imaMountState.generation += 1;
    if (!preserve && !imaMountState.saveOwner) imaMountState.revision += 1;
    imaMountState.dirty = preserve ? oldDirty : false;
    if (!preserve) imaMountState.discoveryEntered = false;
    if (!preserve) imaMountState.folderPanelGroupId = "";
    if (!preserve) imaMountState.folderPanelTouched = false;
    const available = new Set(imaMountState.groups.map((group) => String(group.id)));
    for (const group of imaMountState.groups) {
      const previous = preserve ? oldDrafts.get(String(group.id)) : null;
      const values = previous
        ? [...previous]
        : (Array.isArray(group.folder_ids) ? group.folder_ids : []);
      imaMountState.drafts.set(
        String(group.id),
        new Set(values.filter((folderId) => typeof folderId === "string" && folderId.trim())),
      );
    }
    if (!available.has(String(imaMountState.selectedGroupId || ""))) {
      imaMountState.selectedGroupId = imaMountState.groups[0] ? String(imaMountState.groups[0].id) : "";
    }
  }

  function imaIntervalSegHtml(group) {
    const groupId = String(group?.id || "");
    const current = imaGroupIntervalSeconds(group);
    return `<span class="ima-interval-seg" data-group-id="${escapeHtml(groupId)}">${
      [[3600, "1h"], [21600, "6h"], [86400, "3次"]].map(([sec, label]) =>
        `<button type="button" id="ima-interval-${escapeHtml(groupId)}-${sec}" data-sec="${sec}" aria-pressed="${current === sec}" class="${current === sec ? "is-on" : ""}" onclick="setImaGroupInterval(event, this)">${label}</button>`
      ).join("")
    }</span>`;
  }

  function imaMountGroupRowHtml(group) {
    const groupId = String(group?.id || "");
    const draft = imaMountState.drafts.get(groupId) || new Set();
    const selected = groupId === String(imaMountState.selectedGroupId || "");
    const source = group?.source === "discovered" ? "自动发现" : "旧配置";
    const count = draft.size;
    const mountText = count ? `已选择 ${count} 个文件夹` : "未挂载";
    return `<button type="button" class="ima-mount-kb-row${selected ? " is-selected" : ""}"
      id="ima-kb-row-${escapeHtml(groupId)}" role="option" aria-selected="${selected}"
      data-group-id="${escapeHtml(groupId)}" onclick="selectImaMountGroup(this.dataset.groupId)">
      <span class="ima-mount-kb-copy">
        <span class="ima-mount-kb-name" title="${escapeHtml(group?.name || groupId)}">${escapeHtml(group?.name || groupId)}</span>
        <span class="ima-mount-kb-meta">${escapeHtml(source)} · ${escapeHtml(mountText)}</span>
      </span>
      <span class="ima-mount-kb-count" aria-hidden="true">${count}</span>
    </button>`;
  }

  function renderImaSelectedGroup() {
    const group = imaMountGroup(imaMountState.selectedGroupId);
    const title = $("#ima-selected-group-name");
    const interval = $("#ima-selected-interval");
    const select = $("#ima-kb-select");
    if (select) {
      select.innerHTML = imaMountState.groups.map((item) => {
        const id = String(item.id || "");
        return `<option value="${escapeHtml(id)}"${id === String(imaMountState.selectedGroupId) ? " selected" : ""}>${escapeHtml(item.name || id)}</option>`;
      }).join("");
    }
    if (!group) {
      if (title) title.textContent = "选择知识库";
      if (interval) interval.innerHTML = "";
      return;
    }
    if (title) title.textContent = group.name || String(group.id);
    if (interval) {
      const hours = Math.round(imaGroupIntervalSeconds(group) / 3600);
      const note = hours >= 24 ? "每日 01:00 / 09:00 / 17:00 自动同步（上海）" : `每 ${hours} 小时检查`;
      interval.innerHTML = `${imaIntervalSegHtml(group)}<span class="muted">${note}</span>`;
    }
  }

  function setImaGroupInterval(event, button) {
    event?.stopPropagation?.();
    const seg = button?.closest?.(".ima-interval-seg");
    const groupId = seg?.dataset.groupId || "";
    const seconds = Number(button?.dataset.sec || 0);
    const group = imaMountGroup(groupId);
    if (!group || ![3600, 21600, 86400].includes(seconds)) return;
    group.interval_seconds = seconds;
    imaMountState.revision += 1;
    imaMountState.collectorRevision += 1;
    imaMountState.dirty = true;
    renderImaCollectorDirtyState();
    const draft = rememberImaCollectorDraft();
    if (imaMountState.saveOwner) imaMountState.saveOwner.liveSnapshot = draft;
    const focus = imaFocusSnapshot(button);
    renderImaMountGroups();
    imaRestoreFocus(focus);
  }

  function renderImaMountGroups() {
    const list = $("#ima-kb-list");
    if (!list) return;
    const groups = imaMountState.groups;
    list.innerHTML = groups.length
      ? groups.map((group) => imaMountGroupRowHtml(group)).join("")
      : '<div class="empty ima-mount-empty">尚未发现共享知识库</div>';
    const count = $("#ima-kb-count");
    if (count) count.textContent = `${groups.length} 个`;
    renderImaSelectedGroup();
    renderImaFolderTree(imaMountState.selectedGroupId);
    renderImaCollectorDirtyState();
  }

  function renderImaCollectorDirtyState() {
    const bar = $("#ima-collector-savebar");
    if (!bar) return;
    bar.hidden = !(imaMountState.dirty || imaMountState.collectorDirty);
  }

  function discardImaCollectorChanges() {
    if (!confirm("有未保存的采集配置修改，确定放弃？")) return;
    imaMountState.saveOwner = null;
    imaMountState.collectorDraft = null;
    imaMountState.collectorDraftRevision = "";
    imaMountState.collectorDirty = false;
    imaMountState.dirty = false;
    imaMountState.collectorConfirmedRevision = "";
    imaMountState.collectorConfirmedLiveRevision = -1;
    imaMountState.collectorConfirmedMountRevision = -1;
    renderImaCollectorDirtyState();
    reloadAdminSettingsPage(currentRouteSeq());
  }

  function imaFolderAncestorSelected(groupId, folderId) {
    const selected = imaMountDraft(groupId);
    const seen = new Set();
    let parentId = imaMountState.parents.get(imaMountCacheKey(groupId, folderId)) || "";
    while (parentId && !seen.has(parentId)) {
      if (selected.has(parentId)) return true;
      seen.add(parentId);
      parentId = imaMountState.parents.get(imaMountCacheKey(groupId, parentId)) || "";
    }
    return false;
  }

  function normalizeImaMountDraft(groupId) {
    const selected = imaMountDraft(groupId);
    for (const folderId of [...selected]) {
      if (imaFolderAncestorSelected(groupId, folderId)) selected.delete(folderId);
    }
  }

  function imaFolderDescendantSelected(groupId, folderId) {
    const selected = imaMountDraft(groupId);
    for (const selectedId of selected) {
      if (selectedId === folderId) continue;
      const seen = new Set();
      let parentId = imaMountState.parents.get(imaMountCacheKey(groupId, selectedId)) || "";
      while (parentId && !seen.has(parentId)) {
        if (parentId === folderId) return true;
        seen.add(parentId);
        parentId = imaMountState.parents.get(imaMountCacheKey(groupId, parentId)) || "";
      }
    }
    return false;
  }

  function imaFolderSelectionState(groupId, folderId) {
    const inherited = imaFolderAncestorSelected(groupId, folderId);
    const selected = imaMountDraft(groupId).has(folderId);
    return {
      checked: selected || inherited,
      disabled: inherited,
      indeterminate: !selected && !inherited && imaFolderDescendantSelected(groupId, folderId),
    };
  }

  function imaFolderRowHtml(groupId, item, depth) {
    const folderId = String(item?.id || "");
    const name = String(item?.name || folderId);
    if (!folderId) return "";
    imaMountState.parents.set(imaMountCacheKey(groupId, folderId), String(item?.parent_id || ""));
    const childKey = imaMountCacheKey(groupId, folderId);
    const knownEmpty = item?.has_children === false || Number(item?.folder_count) === 0;
    const hasChildren = !knownEmpty || imaMountState.folders.has(childKey);
    const expanded = imaMountState.expanded.has(childKey);
    const selection = imaFolderSelectionState(groupId, folderId);
    const inputId = `ima-folder-${groupId}-${folderId}`;
    const expand = hasChildren
      ? `<button type="button" class="ima-folder-expand" id="ima-folder-expand-${escapeHtml(groupId)}-${escapeHtml(folderId)}" data-group-id="${escapeHtml(groupId)}" data-folder-id="${escapeHtml(folderId)}" aria-expanded="${expanded}" aria-label="${expanded ? "收起" : "展开"} ${escapeHtml(name)}" title="${expanded ? "收起" : "展开"}" onclick="toggleImaFolderExpand(this)">${expanded ? CHEVRON_DOWN_ICON : CHEVRON_RIGHT_ICON}</button>`
      : '<span class="ima-folder-expand-placeholder" aria-hidden="true"></span>';
    const nested = expanded ? imaRenderFolderBranch(groupId, folderId, depth + 1) : "";
    return `
      <div class="ima-folder-node" style="--ima-folder-indent:${8 + Math.min(depth, 12) * 18}px">
        <div class="ima-folder-row">
          ${expand}
          <label class="ima-folder-choice" for="${escapeHtml(inputId)}">
            <input id="${escapeHtml(inputId)}" type="checkbox" data-group-id="${escapeHtml(groupId)}" data-folder-id="${escapeHtml(folderId)}"
              ${selection.checked ? "checked" : ""}${selection.disabled ? " disabled" : ""}${selection.indeterminate ? ' data-indeterminate="true"' : ""}
              onchange="toggleImaFolder(this)">
            <span class="ima-folder-icon" aria-hidden="true">${FOLDER_ICON}</span>
            <span class="ima-folder-name" title="${escapeHtml(name)}">${escapeHtml(name)}</span>
          </label>
        </div>
        ${nested}
      </div>`;
  }

  function imaFolderErrorHtml(groupId, parentId, error) {
    return `<div class="ima-folder-state ima-folder-error" role="alert"><span>${escapeHtml(error || "文件夹加载失败")}</span><button type="button" class="btn-ghost btn-sm" id="ima-folder-retry-${escapeHtml(groupId)}-${escapeHtml(parentId)}" data-group-id="${escapeHtml(groupId)}" data-parent-id="${escapeHtml(parentId)}" onclick="retryImaFolderLoad(this)">重试</button></div>`;
  }

  function imaRenderFolderBranch(groupId, parentId, depth) {
    const key = imaMountCacheKey(groupId, parentId);
    if (!imaMountState.folders.has(key)) {
      if (imaMountState.loading.has(key)) return '<div class="ima-folder-state">正在加载文件夹…</div>';
      const error = imaMountState.errors.get(key);
      if (error) return imaFolderErrorHtml(groupId, parentId, error);
      return '<div class="ima-folder-state">展开后加载文件夹</div>';
    }
    const items = imaMountState.folders.get(key) || [];
    if (!items.length) return '<div class="ima-folder-state">没有可挂载的子文件夹</div>';
    return items.map((item) => imaFolderRowHtml(groupId, item, depth)).join("");
  }

  function imaFolderOrphansHtml(groupId, rootId) {
    const rootKey = imaMountCacheKey(groupId, rootId);
    if (!imaMountState.folders.has(rootKey)) return "";
    const known = new Set([rootId]);
    for (const [key, items] of imaMountState.folders) {
      if (!key.startsWith(`${groupId}\0`)) continue;
      for (const item of items || []) known.add(String(item.id || ""));
    }
    const orphanIds = [...imaMountDraft(groupId)].filter((folderId) => !known.has(folderId));
    if (!orphanIds.length) return "";
    return `<div class="ima-folder-orphans"><p class="ima-folder-state-title">已选择但当前目录中不可见</p>${orphanIds.map((folderId) => `
      <label class="ima-folder-orphan" for="ima-orphan-${escapeHtml(groupId)}-${escapeHtml(folderId)}">
        <input id="ima-orphan-${escapeHtml(groupId)}-${escapeHtml(folderId)}" type="checkbox" checked data-group-id="${escapeHtml(groupId)}" data-folder-id="${escapeHtml(folderId)}" onchange="toggleImaFolder(this)">
        <span class="ima-folder-name" title="${escapeHtml(folderId)}">${escapeHtml(folderId)}</span>
      </label>`).join("")}</div>`;
  }

  function toggleImaFolderPanel(button) {
    const groupId = String(imaMountState.selectedGroupId || "");
    imaMountState.folderPanelTouched = true;
    imaMountState.folderPanelGroupId = button?.getAttribute("aria-expanded") === "true" ? "" : groupId;
    renderImaFolderTree(groupId);
  }

  function renderImaFolderTree(groupId) {
    const tree = $("#ima-folder-tree");
    if (!tree) return;
    // ponytail: 空配置默认展开（主操作优先），选过文件夹后记住用户收起状态
    if (groupId && !imaMountDraft(String(groupId)).size && !imaMountState.folderPanelTouched) {
      imaMountState.folderPanelGroupId = String(groupId);
    }
    const open = !!groupId && imaMountState.folderPanelGroupId === String(groupId);
    const panel = $("#ima-folder-panel");
    const toggle = $("#ima-folder-panel-toggle");
    const summary = $("#ima-folder-summary");
    const selectedCount = groupId ? imaMountDraft(String(groupId)).size : 0;
    if (toggle) toggle.setAttribute("aria-expanded", String(open));
    if (panel) panel.hidden = !open;
    if (summary) summary.textContent = selectedCount ? `已选 ${selectedCount} 个 · 父目录包含新子目录` : "未选择文件夹";
    if (!open) return;
    const group = imaMountGroup(groupId);
    const title = $("#ima-folder-title");
    const count = $("#ima-folder-count");
    if (!group) {
      if (title) title.textContent = "选择知识库";
      if (count) count.textContent = "";
      tree.innerHTML = '<div class="ima-folder-state">先选择一个知识库</div>';
      return;
    }
    const groupKey = String(group.id);
    const rootId = String(group.root_folder_id || "");
    if (title) title.textContent = group.name || groupKey;
    if (count) count.textContent = `${selectedCount} 个文件夹`;
    const rootKey = imaMountCacheKey(groupKey, rootId);
    const scrollTop = tree.scrollTop;
    tree.setAttribute("aria-busy", String(imaMountState.loading.has(rootKey)));
    tree.innerHTML = imaFolderRowHtml(groupKey, {
      id: rootId,
      name: "整个知识库",
      parent_id: "",
      has_children: true,
    }, 0) + imaFolderOrphansHtml(groupKey, rootId);
    tree.querySelectorAll('input[data-indeterminate="true"]').forEach((input) => {
      input.indeterminate = true;
    });
    tree.scrollTop = scrollTop;
  }

  function imaRestoreFocus(focus) {
    if (!focus || (document.activeElement !== focus.element && document.activeElement !== document.body)) return;
    const target = focus.id ? document.getElementById(focus.id) : null;
    target?.focus({ preventScroll: true });
  }

  function imaFocusSnapshot(element = document.activeElement) {
    return { element, id: element?.id || "" };
  }

  function selectImaMountGroup(groupId) {
    const group = imaMountGroup(groupId);
    if (!group) return;
    const focus = imaFocusSnapshot();
    imaMountState.selectedGroupId = String(group.id);
    renderImaMountGroups();
    renderImaGroupAcl();
    imaRestoreFocus(focus);
  }

  let _aclCandidateUsers = null;
  let _imaAclRenderSeq = 0;

  async function fetchAclCandidateUsers(force = false) {
    if (!force && _aclCandidateUsers) return _aclCandidateUsers;
    _aclCandidateUsers = (await api("/api/users")).filter((u) => !u.is_admin);
    return _aclCandidateUsers;
  }

  function aclGrantedNames(picker) {
    return [...(picker?.querySelectorAll("[data-acl-remove]") || [])]
      .map((el) => el.getAttribute("data-acl-remove"))
      .filter(Boolean);
  }

  function aclChipHtml(name) {
    return `<button type="button" class="ima-acl-chip" data-acl-remove="${escapeHtml(name)}" aria-label="移除 ${escapeHtml(name)}" title="移除 ${escapeHtml(name)}">${escapeHtml(name)}${X_ICON}</button>`;
  }

  function aclPickerHtml(usernames, listId, compact = false) {
    const granted = [...new Set(usernames || [])];
    const chips = granted.length
      ? granted.map(aclChipHtml).join("")
      : `<span class="muted ima-acl-none">仅管理员</span>`;
    return `<div class="ima-acl-picker${compact ? " is-compact" : ""}" data-count="${granted.length}">
      <input type="search" class="form-control ima-acl-search" placeholder="搜索并添加用户" role="combobox" aria-expanded="false" aria-autocomplete="list" aria-label="搜索并添加用户" aria-controls="${listId}" autocomplete="off" oninput="filterAclSuggest(this)" onkeydown="onAclSearchKey(event)" onfocus="filterAclSuggest(this)">
      <div id="${listId}" class="ima-acl-suggest" hidden role="listbox"></div>
      <p class="muted ima-acl-empty" hidden>没有匹配的用户</p>
      <div class="ima-acl-chips">${chips}</div>
      <button type="button" class="ima-acl-more" onclick="toggleImaAclExpanded(this)" hidden></button>
      <p class="muted ima-acl-status" aria-live="polite">${granted.length ? `${granted.length} 人可看` : "仅管理员"}</p>
    </div>`;
  }

  function aclSuggestItems(list) {
    return [...(list?.querySelectorAll("[data-acl-add]") || [])];
  }

  function setAclActive(list, index) {
    const items = aclSuggestItems(list);
    const input = list?.closest(".ima-acl-picker")?.querySelector(".ima-acl-search");
    items.forEach((el, i) => {
      const on = i === index;
      el.classList.toggle("is-on", on);
      el.setAttribute("aria-selected", String(on));
    });
    const active = items[index];
    if (input) input.setAttribute("aria-activedescendant", active?.id || "");
    active?.scrollIntoView({ block: "nearest" });
  }

  function filterAclSuggest(input) {
    const picker = input.closest(".ima-acl-picker");
    const list = picker?.querySelector(".ima-acl-suggest");
    const empty = picker?.querySelector(".ima-acl-empty");
    if (!picker || !list) return;
    const needle = input.value.trim().toLowerCase();
    const granted = new Set(aclGrantedNames(picker));
    const close = () => {
      list.hidden = true;
      list.innerHTML = "";
      input.setAttribute("aria-expanded", "false");
      input.removeAttribute("aria-activedescendant");
    };
    // ponytail: 空查询展示前 8 个候选（可浏览），有输入再按子串过滤
    const hits = (_aclCandidateUsers || [])
      .map((u) => String(u.username || ""))
      .filter((name) => name && !granted.has(name) && (!needle || name.toLowerCase().includes(needle)))
      .slice(0, needle ? 50 : 8);
    if (!hits.length) {
      close();
      if (empty) empty.hidden = false;
      return;
    }
    if (empty) empty.hidden = true;
    const listId = list.id || "ima-acl-list";
    list.hidden = false;
    input.setAttribute("aria-expanded", "true");
    list.innerHTML = hits.map((name, i) =>
      `<button type="button" class="ima-acl-suggest-item" role="option" id="${listId}-opt-${i}" data-acl-add="${escapeHtml(name)}" aria-selected="false">${escapeHtml(name)}</button>`
    ).join("");
    setAclActive(list, 0);
  }

  function onAclSearchKey(event) {
    const input = event.target;
    const picker = input.closest(".ima-acl-picker");
    const list = picker?.querySelector(".ima-acl-suggest");
    if (event.key === "Escape") {
      event.preventDefault();
      input.value = "";
      filterAclSuggest(input);
      return;
    }
    const items = aclSuggestItems(list);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (!items.length) return;
      event.preventDefault();
      const cur = items.findIndex((el) => el.classList.contains("is-on"));
      const next = event.key === "ArrowDown"
        ? Math.min(items.length - 1, (cur < 0 ? -1 : cur) + 1)
        : Math.max(0, (cur < 0 ? items.length : cur) - 1);
      setAclActive(list, next);
      return;
    }
    if (event.key !== "Enter") return;
    event.preventDefault();
    const active = items.find((el) => el.classList.contains("is-on")) || items[0];
    if (active) addAclUser(active.getAttribute("data-acl-add"), picker);
  }

  async function saveImaGroupAcl(groupId, usernames) {
    const r = await api(`/api/admin/ima-collector/groups/${encodeURIComponent(groupId)}/acl`, {
      method: "PUT",
      body: JSON.stringify({ usernames }),
    });
    return r.acl_usernames || usernames;
  }

  function applyAclNamesToPicker(picker, names) {
    const chips = picker.querySelector(".ima-acl-chips");
    const status = picker.querySelector(".ima-acl-status");
    if (chips) {
      chips.innerHTML = names.length
        ? names.map(aclChipHtml).join("")
        : `<span class="muted ima-acl-none">仅管理员</span>`;
    }
    if (status) status.textContent = names.length ? `${names.length} 人可看` : "仅管理员";
    syncImaAclMoreButton(picker, names.length);
  }

  function syncImaAclMoreButton(picker, count) {
    const button = picker?.querySelector(".ima-acl-more");
    if (!button) return;
    picker.dataset.count = String(count);
    button.hidden = count <= 6;
    const expanded = picker.classList.contains("is-expanded");
    button.textContent = expanded ? "收起" : `展开全部 ${count} 人`;
    button.setAttribute("aria-expanded", String(expanded));
  }

  function toggleImaAclExpanded(button) {
    const picker = button?.closest(".ima-acl-picker");
    if (!picker) return;
    picker.classList.toggle("is-expanded");
    syncImaAclMoreButton(picker, Number(picker.dataset.count || 0));
  }

  function rememberAclOnModel(groupId, names) {
    const group = imaMountGroup(groupId);
    if (group) group.acl_usernames = names;
    const lib = ((currentLocalLibraries()?.libraries) || [])
      .find((item) => `local-${item.slug}` === groupId);
    if (!lib) return;
    lib.acl_usernames = names;
    const el = document.querySelector(`.ima-source-block[data-slug="${lib.slug}"]`);
    const line = el?.querySelector(".section-meta");
    if (!line) return;
    const n = names.length;
    line.innerHTML = line.innerHTML.replace(
      /· (权限 \d+ 人|仅管理员)\s*$/,
      `· ${n ? `权限 ${n} 人` : "仅管理员"}`,
    );
  }

  async function addAclUser(name, picker) {
    if (!name || !picker || picker.dataset.busy) return;
    const groupId = picker.dataset.groupId;
    if (!groupId) return;
    const current = aclGrantedNames(picker);
    if (current.includes(name)) return;
    picker.dataset.busy = "1";
    try {
      const saved = await saveImaGroupAcl(groupId, [...current, name]);
      rememberAclOnModel(groupId, saved);
      applyAclNamesToPicker(picker, saved);
      const search = picker.querySelector(".ima-acl-search");
      if (search) {
        search.value = "";
        filterAclSuggest(search);
        search.focus();
      }
      flash("权限已保存");
    } catch (err) {
      flash("保存失败: " + err.message, "error");
    } finally {
      delete picker.dataset.busy;
    }
  }

  async function removeAclUser(name, picker) {
    if (!name || !picker || picker.dataset.busy) return;
    const groupId = picker.dataset.groupId;
    if (!groupId) return;
    const current = aclGrantedNames(picker);
    if (!current.includes(name)) return;
    if (current.length === 1 && !confirm("将只剩管理员可看，确定？")) return;
    picker.dataset.busy = "1";
    try {
      const saved = await saveImaGroupAcl(groupId, current.filter((item) => item !== name));
      rememberAclOnModel(groupId, saved);
      applyAclNamesToPicker(picker, saved);
      const search = picker.querySelector(".ima-acl-search");
      if (search?.value) filterAclSuggest(search);
      flash("权限已保存");
    } catch (err) {
      flash("保存失败: " + err.message, "error");
    } finally {
      delete picker.dataset.busy;
    }
  }

  async function renderImaGroupAcl() {
    const slot = $("#ima-group-acl");
    if (!slot) return;
    const group = imaMountGroup(imaMountState.selectedGroupId);
    if (!group) {
      slot.innerHTML = '<p class="muted">先选择一个知识库</p>';
      return;
    }
    const seq = ++_imaAclRenderSeq;
    const groupId = String(group.id);
    slot.innerHTML = '<p class="muted">加载用户…</p>';
    try {
      await fetchAclCandidateUsers();
      if (seq !== _imaAclRenderSeq || String(imaMountState.selectedGroupId) !== groupId) return;
      slot.innerHTML = aclPickerHtml(group.acl_usernames || [], "ima-acl-list", /* compact */ true);
      const picker = slot.querySelector(".ima-acl-picker");
      if (picker) {
        picker.dataset.groupId = groupId;
        syncImaAclMoreButton(picker, (group.acl_usernames || []).length);
      }
    } catch (err) {
      if (seq !== _imaAclRenderSeq) return;
      slot.innerHTML = `<p class="muted">用户列表加载失败：${escapeHtml(err.message)}</p>
        <button type="button" class="btn-ghost" onclick="retryImaGroupAcl()">重试</button>`;
    }
  }

  function toggleImaFolderExpand(button) {
    const groupId = button?.dataset.groupId || "";
    const folderId = button?.dataset.folderId || "";
    if (!groupId || !folderId) return;
    const focus = imaFocusSnapshot(button);
    const key = imaMountCacheKey(groupId, folderId);
    const opening = !imaMountState.expanded.has(key);
    if (opening) imaMountState.expanded.add(key);
    else imaMountState.expanded.delete(key);
    renderImaFolderTree(imaMountState.selectedGroupId);
    imaRestoreFocus(focus);
    if (opening && !imaMountState.folders.has(key)) loadImaFolderChildren(groupId, folderId);
  }

  function toggleImaFolder(input) {
    const groupId = input?.dataset.groupId || "";
    const folderId = input?.dataset.folderId || "";
    if (!groupId || !folderId || input.disabled) return;
    const focus = imaFocusSnapshot(input);
    const selected = imaMountDraft(groupId);
    const group = imaMountGroup(groupId);
    if (input.checked) {
      if (folderId === String(group?.root_folder_id || "")) {
        selected.clear();
      } else {
        selected.delete(folderId);
        for (const selectedId of [...selected]) {
          const seen = new Set();
          let parentId = imaMountState.parents.get(imaMountCacheKey(groupId, selectedId)) || "";
          while (parentId && !seen.has(parentId)) {
            if (parentId === folderId) selected.delete(selectedId);
            seen.add(parentId);
            parentId = imaMountState.parents.get(imaMountCacheKey(groupId, parentId)) || "";
          }
        }
      }
      selected.add(folderId);
    } else {
      selected.delete(folderId);
    }
    imaMountState.revision += 1;
    imaMountState.collectorRevision += 1;
    imaMountState.dirty = true;
    renderImaCollectorDirtyState();
    const draft = rememberImaCollectorDraft();
    if (imaMountState.saveOwner) imaMountState.saveOwner.liveSnapshot = draft;
    renderImaMountGroups();
    imaRestoreFocus(focus);
  }

  async function loadImaFolderChildren(groupId, parentId, force = false) {
    const key = imaMountCacheKey(groupId, parentId);
    if (!force && imaMountState.folders.has(key)) return;
    if (!force && imaMountState.loading.has(key)) return;
    const focus = imaFocusSnapshot();
    const request = { generation: imaMountState.generation, id: ++imaMountState.requestSeq };
    if (force) imaMountState.folders.delete(key);
    imaMountState.errors.delete(key);
    imaMountState.loading.add(key);
    imaMountState.folderRequests.set(key, request);
    if (String(imaMountState.selectedGroupId) === String(groupId)) {
      renderImaFolderTree(groupId);
      imaRestoreFocus(focus);
    }
    try {
      const path = `/api/admin/ima-collector/groups/${encodeURIComponent(groupId)}/folders?parent_id=${encodeURIComponent(parentId)}`;
      const data = await api(path);
      const current = request.generation === imaMountState.generation
        && imaMountState.folderRequests.get(key) === request;
      if (!current) return;
      const items = Array.isArray(data.items) ? data.items : [];
      imaMountState.folders.set(key, items);
      items.forEach((item) => {
        if (item?.id) imaMountState.parents.set(imaMountCacheKey(groupId, String(item.id)), String(item.parent_id || parentId));
      });
      normalizeImaMountDraft(groupId);
    } catch (err) {
      const current = request.generation === imaMountState.generation
        && imaMountState.folderRequests.get(key) === request;
      if (current) imaMountState.errors.set(key, imaSafeError(err.message || "文件夹加载失败"));
    } finally {
      const current = request.generation === imaMountState.generation
        && imaMountState.folderRequests.get(key) === request;
      if (!current) return;
      imaMountState.folderRequests.delete(key);
      imaMountState.loading.delete(key);
      if (String(imaMountState.selectedGroupId) === String(groupId)) {
        renderImaFolderTree(groupId);
        imaRestoreFocus(focus);
      }
    }
  }

  function retryImaFolderLoad(button) {
    loadImaFolderChildren(button?.dataset.groupId || "", button?.dataset.parentId || "", true);
  }

  async function discoverImaGroups() {
    if (imaMountState.discoveryBusy) return;
    const routeSeq = currentRouteSeq();
    const generation = imaMountState.generation;
    const discoverySeq = ++imaMountState.discoverySeq;
    const request = { generation, routeSeq, seq: discoverySeq };
    imaMountState.discoveryOwner = request;
    imaMountState.discoveryBusy = true;
    const button = $("#ima-discover-btn");
    const status = $("#ima-group-discovery-status");
    if (button) button.disabled = true;
    if (status) status.textContent = "正在发现共享知识库…";
    try {
      const result = await api("/api/admin/ima-collector/discover", { method: "POST" });
      if (generation !== imaMountState.generation
        || imaMountState.discoverySeq !== discoverySeq
        || imaMountState.discoveryOwner !== request
        || !routeStillActive(routeSeq)) return;
      if (result.ok && result.config) {
        initImaMountState(result.config.groups, true);
        imaMountState.discoveryOwner = request;
        imaMountState.discoveryBusy = true;
        renderImaMountGroups();
        renderImaGroupAcl();
        const currentStatus = $("#ima-group-discovery-status");
        if (currentStatus) currentStatus.innerHTML = imaGroupDiscoveryStatusText(result);
      } else {
        const error = result.discovery?.error || "自动发现失败";
        const currentStatus = $("#ima-group-discovery-status");
        if (currentStatus) currentStatus.innerHTML = `自动发现失败：${escapeHtml(imaSafeError(error))}（已保留上次结果）`;
      }
    } catch (err) {
      if (generation === imaMountState.generation
        && imaMountState.discoverySeq === discoverySeq
        && imaMountState.discoveryOwner === request
        && routeStillActive(routeSeq)) {
        const currentStatus = $("#ima-group-discovery-status");
        if (currentStatus) currentStatus.innerHTML = `自动发现失败：${escapeHtml(imaSafeError(err.message || "请求失败"))}（已保留上次结果）`;
      }
    } finally {
      if (imaMountState.discoveryOwner !== request || !routeStillActive(routeSeq)) return;
      imaMountState.discoveryBusy = false;
      imaMountState.discoveryOwner = null;
      const currentButton = $("#ima-discover-btn");
      if (currentButton && document.body.contains(currentButton)) currentButton.disabled = false;
    }
  }

  function readImaMountGroups() {
    return imaMountState.groups.map((group) => {
      const folderIds = [...imaMountDraft(String(group.id))];
      return {
        id: String(group.id || "") || null,
        name: String(group.name || "").trim(),
        knowledge_base_id: String(group.knowledge_base_id || "").trim(),
        root_folder_id: String(group.root_folder_id || "").trim(),
        folder_ids: folderIds,
        enabled: folderIds.length > 0,
        interval_seconds: imaGroupIntervalSeconds(group),
      };
    });
  }

  function imaCollectorFormSnapshot() {
    return {
      uid: imaCollectorPureCache.uid || "",
      knowledge_base_id: imaCollectorPureCache.knowledge_base_id || "",
      root_folder_id: imaCollectorPureCache.root_folder_id || "",
      interval_seconds: Number(imaCollectorPureCache.interval_seconds || 3600),
      groups: readImaMountGroups(),
      refresh_token: String(imaMountState.collectorDraft?.refresh_token || ""),
    };
  }

  function imaCollectorFormRevision(snapshot) {
    return JSON.stringify(snapshot);
  }

  function rememberImaCollectorDraft(snapshot = imaCollectorFormSnapshot()) {
    imaMountState.collectorDraft = snapshot;
    imaMountState.collectorDraftRevision = imaCollectorFormRevision(snapshot);
    imaMountState.collectorDirty = true;
    return snapshot;
  }

  function clearImaCollectorDraft(revision) {
    if (imaMountState.collectorDraftRevision !== revision) return;
    imaMountState.collectorDraft = null;
    imaMountState.collectorDraftRevision = "";
    imaMountState.collectorDirty = false;
    if (imaMountState.collectorConfirmedRevision === revision) {
      imaMountState.collectorConfirmedRevision = "";
      imaMountState.collectorConfirmedLiveRevision = -1;
      imaMountState.collectorConfirmedMountRevision = -1;
    }
  }

  function imaCollectorDraftChanged(event) {
    const target = event.target;
    if (!target?.closest?.(".ima-interval-seg")) return;
    if (event.type === "click") return;
    const snapshot = rememberImaCollectorDraft();
    imaMountState.collectorRevision += 1;
    if (imaMountState.saveOwner) imaMountState.saveOwner.liveSnapshot = snapshot;
  }

  document.addEventListener("input", imaCollectorDraftChanged);
  document.addEventListener("change", imaCollectorDraftChanged);

  function restoreImaCollectorOwnerToken(owner, seq, draft = null) {
    if (owner && owner !== imaMountState.saveOwner) return;
    const draftRevision = imaMountState.collectorDraftRevision;
    if (draftRevision === imaMountState.collectorConfirmedRevision
      && imaMountState.collectorRevision === imaMountState.collectorConfirmedLiveRevision) return;
    const pendingToken = owner?.liveSnapshot?.refresh_token || owner?.snapshot?.refresh_token || draft?.refresh_token;
    if (!pendingToken) return;
    const tokenInput = $("#ima-pure-token");
    if (tokenInput && !tokenInput.value) tokenInput.value = pendingToken;
  }

  function imaSafeError(value) {
    let text = String(value ?? "").split(/\r?\n/, 1)[0].slice(0, 240);
    text = text.replace(/https?:\/\/\S+/gi, "<url>");
    text = text.replace(/\bBearer\s+\S+/gi, "Bearer <redacted>");
    text = text.replace(/(^|[?&\s])((?:token|refresh_token|authorization|sign|q-sign)\b(?:\s*[=:]\s*|\s+))[^\s&]+/gi, "$1$2<redacted>");
    return text;
  }

  function imaGroupDiscoveryStatusText(status) {
    const result = status?.last_result || {};
    const discovery = status?.discovery || {};
    const discoveryError = String(discovery.error || result.discovery_error || "").trim();
    if (discoveryError) {
      const safeError = imaSafeError(discoveryError);
      return `自动发现失败：${escapeHtml(safeError)}（已保留上次结果）`;
    }
    const groups = Array.isArray(status?.config?.groups) ? status.config.groups : [];
    if (!groups.length) {
      return discovery.status === "ok" ? "未发现可用共享知识库" : "尚未发现共享知识库";
    }
    const mounted = groups.filter((group) => Array.isArray(group.folder_ids) && group.folder_ids.length).length;
    const synced = Number.isFinite(Number(result.succeeded_groups))
      ? ` · 最近同步 ${Number(result.succeeded_groups)} 个知识库`
      : " · 等待同步";
    return `已发现 ${groups.length} 个知识库 · 已挂载 ${mounted} 个${synced}`;
  }

  function imaCollectorHasUnsaved() {
    return !!(imaMountState.dirty || imaMountState.collectorDirty);
  }

  function imaCollectorStatusText(status) {
    const result = status.last_result || {};
    const config = status.config || {};
    const storage = status.storage;
    const storageMessages = {
      unavailable: "研报库存储暂不可用",
      stale: "研报库存储状态过期",
      readonly: "研报库存储当前只读",
      capacity_blocked: "研报库存储空间已达限制",
    };
    if (storageMessages[storage?.status]) return storageMessages[storage.status];
    let text;
    if (!config.refresh_token?.set) text = "未配置 Refresh Token";
    else if (status.running) text = "同步中…";
    else {
      const groups = Array.isArray(config.groups) ? config.groups : [];
      const mounted = groups.filter((group) => Array.isArray(group.folder_ids) && group.folder_ids.length).length;
      if (!mounted) text = "已连接 · 尚未挂载研报库";
      else if (status.last_finished_at) {
        const ok = Number(result.downloaded || 0);
        const failed = Number(result.failed || 0);
        text = `已归档 ${Number(status.documents || 0).toLocaleString()} 份 · 上次新增 ${ok} 份${failed ? ` · 失败 ${failed} 份` : ""}`;
      } else {
        text = `已配置 · 每 ${Math.round(Number(config.interval_seconds || 3600) / 60)} 分钟检查`;
      }
    }
    if (storage?.status === "available") {
      const used = Math.max(0, Math.min(100, Number(storage.used_percent) || 0));
      text += ` · 存储 ${used}%`;
    }
    const indexMessages = {
      rebuilding: "索引重建中",
      fallback: "索引回退",
      failed: "索引异常",
    };
    const indexStatus = status.index?.status || "ready";
    if (indexStatus !== "ready" && indexMessages[indexStatus]) {
      text += ` · ${indexMessages[indexStatus]}`;
    }
    return text;
  }

  async function saveImaCollector() {
    const routeSeq = currentRouteSeq();
    const saveButton = $("#ima-collector-save");
    if (imaMountState.saveOwner) return;
    if (saveButton?.disabled) return;
    const sessionGeneration = imaMountState.sessionGeneration;
    const mountRevision = imaMountState.revision;
    const collectorRevision = imaMountState.collectorRevision;
    const focusElement = document.activeElement;
    const focusId = focusElement?.id || "";
    let focusMoved = false;
    const onFocusIn = (event) => {
      if (event.target !== focusElement && event.target !== document.body) focusMoved = true;
    };
    const snapshot = imaCollectorFormSnapshot();
    rememberImaCollectorDraft(snapshot);
    const body = {
      uid: snapshot.uid,
      knowledge_base_id: snapshot.knowledge_base_id,
      root_folder_id: snapshot.root_folder_id,
      interval_seconds: snapshot.interval_seconds,
      groups: readImaMountGroups(),
    };
    const token = snapshot.refresh_token;
    if (token) body.refresh_token = token;
    const saveOwner = {
      routeSeq,
      sessionGeneration,
      mountRevision,
      collectorRevision,
      formRevision: imaCollectorFormRevision(snapshot),
      snapshot,
    };
    const onDraftChange = (event) => {
      if (imaMountState.saveOwner !== saveOwner) return;
      const target = event.target;
      if (!target?.closest?.(".ima-interval-seg") && !target.closest?.("#ima-mount-layout")) return;
      const liveSnapshot = rememberImaCollectorDraft();
      if (imaMountState.saveOwner === saveOwner) saveOwner.liveSnapshot = liveSnapshot;
    };
    imaMountState.saveOwner = saveOwner;
    if (saveButton) saveButton.disabled = true;
    document.addEventListener("input", onDraftChange);
    document.addEventListener("change", onDraftChange);
    document.addEventListener("click", onDraftChange);
    document.addEventListener("focusin", onFocusIn);
    try {
      const savedImaStatus = await api("/api/admin/ima-collector", { method: "PUT", body: JSON.stringify(body) });
      if (sessionGeneration !== imaMountState.sessionGeneration || imaMountState.saveOwner !== saveOwner) return;
      saveOwner.savedImaStatus = savedImaStatus;
      saveOwner.putCompleted = true;
      const submittedLiveRevision = saveOwner.liveSnapshot
        ? imaCollectorFormRevision(saveOwner.liveSnapshot) : saveOwner.formRevision;
      const formStillCurrent = submittedLiveRevision === saveOwner.formRevision;
      const collectorStillCurrent = imaMountState.collectorRevision === saveOwner.collectorRevision;
      const mountStillCurrent = imaMountState.revision === saveOwner.mountRevision;
      if (formStillCurrent && collectorStillCurrent && mountStillCurrent) {
        imaMountState.collectorConfirmedRevision = saveOwner.formRevision;
        imaMountState.collectorConfirmedLiveRevision = saveOwner.collectorRevision;
        imaMountState.collectorConfirmedMountRevision = saveOwner.mountRevision;
      }
      const currentSnapshot = routeStillActive(routeSeq) && isAdminSettingsPath()
        && $("#ima-mount-layout") ? imaCollectorFormSnapshot() : null;
      const currentFormRevision = currentSnapshot
        ? imaCollectorFormRevision(currentSnapshot) : submittedLiveRevision;
      if (currentSnapshot && currentFormRevision !== saveOwner.formRevision) {
        saveOwner.liveSnapshot = rememberImaCollectorDraft(currentSnapshot);
      }
      if (sessionGeneration !== imaMountState.sessionGeneration || imaMountState.saveOwner !== saveOwner) return;
      if (!routeStillActive(routeSeq) && !isAdminSettingsPath()) return;
      const statsReloadSeq = routeStillActive(routeSeq) ? routeSeq : currentRouteSeq();
      let statsReloadAccepted;
      if (statsReloadSeq === routeSeq) {
        statsReloadAccepted = await reloadAdminSettingsPage(routeSeq, savedImaStatus);
      } else {
        statsReloadAccepted = await reloadAdminSettingsPage(currentRouteSeq(), savedImaStatus);
      }
      if (!statsReloadAccepted || sessionGeneration !== imaMountState.sessionGeneration
        || imaMountState.saveOwner !== saveOwner) return;
      if (!routeStillActive(statsReloadSeq) || !isAdminSettingsPath()) return;
      const reloadedSnapshot = imaCollectorFormSnapshot();
      const reloadedRevision = imaCollectorFormRevision(reloadedSnapshot);
      const liveRevision = saveOwner.liveSnapshot
        ? imaCollectorFormRevision(saveOwner.liveSnapshot) : saveOwner.formRevision;
      const formStillCurrentAfterReload = !saveOwner.liveSnapshot || reloadedRevision === liveRevision;
      const mountStillCurrentAfterReload = imaMountState.revision === saveOwner.mountRevision;
      const noNewerEditsAfterReload = formStillCurrentAfterReload && mountStillCurrentAfterReload && liveRevision === saveOwner.formRevision;
      const collectorStillCurrentAfterReload = imaMountState.collectorRevision === saveOwner.collectorRevision;
      if (noNewerEditsAfterReload) {
        if (collectorStillCurrentAfterReload) {
          clearImaCollectorDraft(saveOwner.formRevision);
          imaMountState.dirty = false;
          const tokenInput = $("#ima-pure-token");
          if (tokenInput) tokenInput.value = "";
        }
      }
      renderImaCollectorDirtyState();
      const restoreFocus = document.activeElement === focusElement || document.activeElement === document.body;
      if (!focusMoved && restoreFocus) {
        const focusTarget = document.getElementById(focusId) || document.getElementById("ima-collector-save");
        focusTarget?.focus({ preventScroll: true });
      }
      flash("IMA 文档采集配置已保存");
    } catch (err) {
      if (sessionGeneration !== imaMountState.sessionGeneration || imaMountState.saveOwner !== saveOwner) return;
      if (routeStillActive(routeSeq) && isAdminSettingsPath()) {
        flash(err.message || "保存失败", "error");
      }
    } finally {
      document.removeEventListener("input", onDraftChange);
      document.removeEventListener("change", onDraftChange);
      document.removeEventListener("click", onDraftChange);
      document.removeEventListener("focusin", onFocusIn);
      if (imaMountState.saveOwner === saveOwner) {
        imaMountState.saveOwner = null;
        if (saveButton && document.body.contains(saveButton)) saveButton.disabled = false;
        const currentSaveButton = $("#ima-collector-save");
        if (currentSaveButton) currentSaveButton.disabled = false;
      }
    }
  }

  function imaCollectorProgressHtml(status) {
    const p = status.progress;
    if (!status.running || !p) return "";
    if (p.phase === "listing") {
      return `<div class="ima-progress"><div class="ima-progress-label">${escapeHtml(p.group_name || "")} · 列目录 ${Number(p.listed||0)}</div><div class="ima-progress-bar"><span style="width:15%"></span></div></div>`;
    }
    const total = Math.max(1, Number(p.pending || 0));
    const done = Number(p.downloaded || 0);
    const pct = Math.max(0, Math.min(100, Math.round(done * 100 / total)));
    return `<div class="ima-progress"><div class="ima-progress-label">${escapeHtml(p.group_name || "")} · 下载 ${done} / ${Number(p.pending||0)}</div><div class="ima-progress-bar"><span style="width:${pct}%"></span></div></div>`;
  }

  function stopImaProgressPoll() {
    imaProgressPollSeq += 1;
    if (imaProgressTimer) {
      clearInterval(imaProgressTimer);
      imaProgressTimer = null;
    }
  }

  function applyImaCollectorProgress(status) {
    const progress = $("#ima-sync-progress");
    if (progress) progress.innerHTML = imaCollectorProgressHtml(status);
    const btn = $("#ima-sync-btn");
    if (btn && document.body.contains(btn)) btn.disabled = !!status?.running;
    const text = imaCollectorStatusText(status);
    const target = $("#ima-collector-status");
    if (target) target.textContent = text;
    const selected = $("#ima-selected-group-state");
    if (selected) selected.textContent = text;
  }

  function startImaProgressPoll() {
    stopImaProgressPoll();
    const routeSeq = currentRouteSeq();
    const pollSeq = imaProgressPollSeq;
    const tick = async () => {
      try {
        const status = await api("/api/admin/ima-collector");
        if (pollSeq !== imaProgressPollSeq || !routeStillActive(routeSeq)) return;
        applyImaCollectorProgress(status);
        if (!status.running) stopImaProgressPoll();
      } catch {
        if (pollSeq !== imaProgressPollSeq || !routeStillActive(routeSeq)) return;
      }
    };
    imaProgressTimer = setInterval(tick, 2000);
    tick();
  }

  async function triggerImaCollector() {
    const routeSeq = currentRouteSeq();
    const btn = $("#ima-sync-btn");
    if (btn?.disabled) return;
    const groupId = imaMountState.selectedGroupId;
    if (!groupId) {
      flash("请先选择知识库", "error");
      return;
    }
    const group = imaMountGroup(groupId);
    const mounted = Array.isArray(group?.folder_ids) && group.folder_ids.length;
    if (!mounted) {
      flash("请先挂载该知识库并保存", "error");
      return;
    }
    if (btn) btn.disabled = true;
    try {
      const result = await api("/api/admin/ima-collector/sync", { method: "POST", body: JSON.stringify({ group_id: groupId }) });
      if (!routeStillActive(routeSeq)) return;
      flash(result.status === "already_running" ? "IMA 文档同步正在进行中" : `已启动同步「${group?.name || groupId}」`);
      const status = await api("/api/admin/ima-collector");
      if (!routeStillActive(routeSeq)) return;
      const target = $("#ima-collector-status");
      if (target) target.textContent = imaCollectorStatusText(status);
      const discovery = $("#ima-group-discovery-status");
      if (discovery) discovery.innerHTML = imaGroupDiscoveryStatusText(status);
      applyImaCollectorProgress(status);
      startImaProgressPoll();
    } catch (err) {
      if (routeStillActive(routeSeq)) {
        flash(err.message || "同步启动失败", "error");
        if (btn && document.body.contains(btn)) btn.disabled = false;
      }
    } finally {
      if (routeStillActive(routeSeq)) return;
      stopImaProgressPoll();
    }
  }

  async function saveImaCredentials() {
    const routeSeq = currentRouteSeq();
    const token = state.token;
    const sessionGeneration = imaMountState.sessionGeneration;
    const cookie = $("#ima-cookie")?.value?.trim() || "";
    const cid = $("#ima-cid")?.value?.trim() || "";
    const key = $("#ima-key")?.value?.trim() || "";
    if (!cookie && !(cid && key)) {
      flash("需至少填 Cookie 或 OpenAPI 凭证（clientid + apikey）", "error");
      return;
    }
    try {
      await api("/api/admin/ima-credentials", {
        method: "POST",
        body: JSON.stringify({ cookie, openapi_clientid: cid, openapi_apikey: key }),
      });
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash("IMA 凭证已保存");
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      await reloadAdminSettingsPage(routeSeq);
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      focusCookieField("ima");
    } catch (e) {
      if (!sessionOwnerStillActive(routeSeq, token, sessionGeneration)) return;
      flash(e.message || "保存失败", "error");
    }
  }

  function retryImaGroupAcl() {
    return fetchAclCandidateUsers(true).then(renderImaGroupAcl);
  }

  return {
    imaMountCacheKey,
    imaMountGroup,
    imaMountDraft,
    imaGroupIntervalSeconds,
    initImaMountState,
    imaIntervalSegHtml,
    imaMountGroupRowHtml,
    renderImaSelectedGroup,
    setImaGroupInterval,
    renderImaMountGroups,
    renderImaCollectorDirtyState,
    discardImaCollectorChanges,
    imaFolderAncestorSelected,
    normalizeImaMountDraft,
    imaFolderDescendantSelected,
    imaFolderSelectionState,
    imaFolderRowHtml,
    imaFolderErrorHtml,
    imaRenderFolderBranch,
    imaFolderOrphansHtml,
    toggleImaFolderPanel,
    renderImaFolderTree,
    imaRestoreFocus,
    imaFocusSnapshot,
    selectImaMountGroup,
    fetchAclCandidateUsers,
    aclGrantedNames,
    aclChipHtml,
    aclPickerHtml,
    aclSuggestItems,
    setAclActive,
    filterAclSuggest,
    onAclSearchKey,
    saveImaGroupAcl,
    applyAclNamesToPicker,
    syncImaAclMoreButton,
    toggleImaAclExpanded,
    rememberAclOnModel,
    addAclUser,
    removeAclUser,
    renderImaGroupAcl,
    toggleImaFolderExpand,
    toggleImaFolder,
    loadImaFolderChildren,
    retryImaFolderLoad,
    discoverImaGroups,
    readImaMountGroups,
    imaCollectorFormSnapshot,
    imaCollectorFormRevision,
    rememberImaCollectorDraft,
    clearImaCollectorDraft,
    imaCollectorDraftChanged,
    restoreImaCollectorOwnerToken,
    imaSafeError,
    imaGroupDiscoveryStatusText,
    imaCollectorHasUnsaved,
    imaCollectorStatusText,
    saveImaCollector,
    imaCollectorProgressHtml,
    stopImaProgressPoll,
    applyImaCollectorProgress,
    startImaProgressPoll,
    triggerImaCollector,
    saveImaCredentials,
    retryImaGroupAcl,
  };
}
