(() => {
  const list = document.querySelector("[data-agent-messages]");
  const status = document.querySelector("[data-agent-status]");
  const count = document.querySelector("[data-agent-count]");
  const form = document.querySelector(".agent-composer");
  const input = document.getElementById("agentBody");
  const editMessageId = document.getElementById("editMessageId");
  const editStrip = document.getElementById("editStrip");
  const sendButton = document.querySelector("[data-send-button]");
  const cancelButton = document.querySelector("[data-cancel-button]");
  const suggestionBox = document.getElementById("commandSuggestions");
  const activeCwd = document.querySelector("[data-active-cwd]");
  const composerSuggestions = {composer_suggestions_json};
  const modelPicker = document.querySelector("[data-agent-model]");
  const reasoningPicker = document.querySelector("[data-agent-reasoning]");
  const initialModelCatalog = {model_catalog_json};
  let modelCatalog = initialModelCatalog;
  const modelStorageKey = "mobailmux.agent.model";
  const reasoningStorageKey = "mobailmux.agent.reasoning";
  let selectedSuggestion = 0;
  const viewingTranscript = {viewing_transcript};
  const activeSlotId = "{active_slot_id}";
  const slotRows = new Map();
  function storedValue(key) {
    try { return window.localStorage.getItem(key) || ""; } catch (_) { return ""; }
  }
  function storeValue(key, value) {
    try { window.localStorage.setItem(key, value); } catch (_) {}
  }
  function catalogModel(models, name) {
    return (models || []).find((model) => model.model === name) || null;
  }
  function defaultCatalogModel(models) {
    return (models || []).find((model) => model.is_default) || (models || [])[0] || null;
  }
  function setOption(select, value, label, title) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    if (title) option.title = title;
    select.append(option);
  }
  function syncReasoningPicker(model, preferredEffort) {
    if (!reasoningPicker) return;
    reasoningPicker.replaceChildren();
    const efforts = model?.supported_reasoning_efforts || [];
    if (!model || !efforts.length) {
      setOption(reasoningPicker, "", "Unavailable");
      reasoningPicker.disabled = true;
      return;
    }
    const selected = efforts.some((item) => item.effort === preferredEffort)
      ? preferredEffort
      : model.default_reasoning_effort || efforts[0].effort;
    efforts.forEach((item) => setOption(reasoningPicker, item.effort, item.effort, item.description));
    reasoningPicker.value = selected;
    reasoningPicker.disabled = false;
    storeValue(reasoningStorageKey, selected);
  }
  function syncModelPickers(models) {
    if (!modelPicker) return;
    modelCatalog = models || [];
    if (!models.length) {
      modelPicker.replaceChildren();
      setOption(modelPicker, "", "Models unavailable");
      modelPicker.disabled = true;
      syncReasoningPicker(null, "");
      return;
    }
    const currentModel = catalogModel(models, modelPicker.value);
    const storedModel = catalogModel(models, storedValue(modelStorageKey));
    const selectedModel = currentModel || storedModel || defaultCatalogModel(models);
    const supportedEfforts = selectedModel?.supported_reasoning_efforts || [];
    const currentEffort = reasoningPicker?.value || "";
    const storedEffort = storedValue(reasoningStorageKey);
    const previousEffort = supportedEfforts.some((item) => item.effort === currentEffort)
      ? currentEffort
      : storedEffort;
    modelPicker.replaceChildren();
    models.forEach((model) => setOption(modelPicker, model.model, model.display_name || model.model, model.description));
    modelPicker.value = selectedModel.model;
    modelPicker.disabled = false;
    storeValue(modelStorageKey, selectedModel.model);
    syncReasoningPicker(selectedModel, previousEffort);
  }
  let modelCatalogPollTimer = 0;
  function scheduleModelCatalogPoll(delay) {
    window.clearTimeout(modelCatalogPollTimer);
    modelCatalogPollTimer = window.setTimeout(loadModelCatalog, delay);
  }
  async function loadModelCatalog() {
    try {
      const response = await fetch("/agents/models", {cache:"no-store"});
      if (response.ok) {
        const data = await response.json();
        if (Array.isArray(data.models) && data.models.length) {
          syncModelPickers(data.models);
          return;
        }
      }
    } catch (_) {}
    scheduleModelCatalogPoll(2500);
  }
  if (modelPicker) {
    modelPicker.addEventListener("change", () => {
      const model = catalogModel(modelCatalog, modelPicker.value);
      storeValue(modelStorageKey, modelPicker.value);
      syncReasoningPicker(model, storedValue(reasoningStorageKey));
    });
  }
  reasoningPicker?.addEventListener("change", () => storeValue(reasoningStorageKey, reasoningPicker.value));
  if (initialModelCatalog.length) syncModelPickers(initialModelCatalog);
  else scheduleModelCatalogPoll(0);
  document.querySelectorAll("[data-slot-row]").forEach((row) => {
    const id = row.getAttribute("data-slot-id");
    if (!id) return;
    const entry = {
      row,
      status: row.querySelector("[data-slot-status]"),
      badge: row.querySelector("[data-slot-badge]"),
      wasRunning: row.getAttribute("data-slot-running") === "true"
    };
    row.addEventListener("click", () => {
      row.classList.remove("done");
      if (entry.badge) entry.badge.hidden = true;
    });
    slotRows.set(id, entry);
  });
  function syncDialogLock() {
    const locked = Array.from(document.querySelectorAll("dialog")).some((dialog) => dialog.open);
    document.documentElement.classList.toggle("drawer-open", locked);
    document.body.classList.toggle("drawer-open", locked);
  }
  function openDialog(dialog) {
    if (!dialog) return;
    if (!dialog.open) {
      if (typeof dialog.showModal === "function") dialog.showModal();
      else dialog.setAttribute("open", "");
    }
    syncDialogLock();
  }
  function closeDialog(dialog) {
    if (!dialog) return;
    if (typeof dialog.close === "function") dialog.close();
    else dialog.removeAttribute("open");
    syncDialogLock();
  }
  document.querySelectorAll("dialog").forEach((dialog) => {
    dialog.addEventListener("close", syncDialogLock);
    dialog.addEventListener("cancel", () => setTimeout(syncDialogLock, 0));
  });
  const projectPanel = document.getElementById("projectPanel");
  const projectForm = document.querySelector("[data-project-form]");
  document.querySelector("[data-project-open]")?.addEventListener("click", () => {
    openDialog(projectPanel);
    window.setTimeout(() => projectForm?.elements.workdir?.focus(), 0);
  });
  document.querySelector("[data-project-close]")?.addEventListener("click", () => closeDialog(projectPanel));
  projectForm?.addEventListener("submit", () => {
    const projectModel = projectForm.querySelector("[data-project-model]");
    const projectReasoning = projectForm.querySelector("[data-project-reasoning]");
    if (projectModel && modelPicker) projectModel.value = modelPicker.value;
    if (projectReasoning && reasoningPicker) projectReasoning.value = reasoningPicker.value;
  });
  if ({reopen_project}) openDialog(projectPanel);
  const terminalPanel = document.getElementById("terminalPanel");
  const terminalForm = document.querySelector("[data-terminal-form]");
  const terminalOutput = document.querySelector("[data-terminal-output]");
  const terminalCwd = document.querySelector("[data-terminal-cwd]");
  document.querySelector("[data-terminal-open]")?.addEventListener("click", () => openDialog(terminalPanel));
  document.querySelector("[data-terminal-close]")?.addEventListener("click", () => closeDialog(terminalPanel));
  const closestElement = (target, selector) => target instanceof Element ? target.closest(selector) : null;
  const lockPageScroll = () => {
    if (document.body.classList.contains("modal-scroll-locked")) return;
    const scrollY = window.scrollY || document.documentElement.scrollTop || 0;
    document.body.dataset.scrollLockY = String(scrollY);
    document.body.style.top = "-" + scrollY + "px";
    document.body.classList.add("modal-scroll-locked");
  };
  const unlockPageScroll = () => {
    if (!document.body.classList.contains("modal-scroll-locked")) return;
    const scrollY = Number(document.body.dataset.scrollLockY || "0");
    document.body.classList.remove("modal-scroll-locked");
    document.body.style.top = "";
    delete document.body.dataset.scrollLockY;
    window.scrollTo(0, scrollY);
  };
  const openLockedDialog = (dialog) => {
    if (!dialog) return;
    if (!dialog.open) {
      if (typeof dialog.showModal === "function") dialog.showModal();
      else dialog.setAttribute("open", "");
    }
    lockPageScroll();
    syncDialogLock();
  };
  document.querySelectorAll("[data-refresh-form]").forEach((form) => {
    form.addEventListener("submit", () => {
      const button = form.querySelector("[data-refresh-button]");
      if (!button) return;
      button.classList.add("is-busy");
      button.setAttribute("aria-busy", "true");
      button.setAttribute("aria-label", "Refreshing");
      button.setAttribute("title", "Refreshing");
    });
  });
  let dirty = false;
  let agentPollTimer = 0;
  let slotPollTimer = 0;
  let selectionHoldUntil = 0;
  let lastMessagesHtml = list ? list.innerHTML : "";
  function scheduleAgentPoll(delay) {
    window.clearTimeout(agentPollTimer);
    agentPollTimer = window.setTimeout(poll, delay);
  }
  function scheduleSlotPoll(delay) {
    window.clearTimeout(slotPollTimer);
    slotPollTimer = window.setTimeout(pollSlots, delay);
  }
  function captureOpenFolds() {
    const keys = new Set();
    list?.querySelectorAll("details[data-fold-key]").forEach((details) => {
      if (details.open) {
        const key = details.getAttribute("data-fold-key");
        if (key) keys.add(key);
      }
    });
    return keys;
  }
  function nodeInsideMessageList(node) {
    if (!list || !node) return false;
    const element = node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
    return !!element && list.contains(element);
  }
  function messageSelectionActive() {
    const selection = window.getSelection?.();
    if (!selection || selection.isCollapsed || !selection.toString().trim()) return false;
    return nodeInsideMessageList(selection.anchorNode) || nodeInsideMessageList(selection.focusNode);
  }
  function holdMessageUpdates(ms = 9000) {
    selectionHoldUntil = Math.max(selectionHoldUntil, Date.now() + ms);
  }
  function canReplaceMessages() {
    if (messageSelectionActive()) {
      holdMessageUpdates();
      return false;
    }
    return Date.now() >= selectionHoldUntil;
  }
  document.addEventListener("selectionchange", () => {
    if (messageSelectionActive()) holdMessageUpdates();
  });
  list?.addEventListener("contextmenu", () => holdMessageUpdates(12000));
  list?.addEventListener("touchstart", () => {
    if (messageSelectionActive()) holdMessageUpdates(5000);
  }, {passive:true});
  function replaceMessages(html) {
    if (!list || html === lastMessagesHtml) return true;
    if (!canReplaceMessages()) return false;
    const openFolds = captureOpenFolds();
    const nearBottom = list.scrollTop + list.clientHeight >= list.scrollHeight - 90;
    list.innerHTML = html;
    lastMessagesHtml = html;
    list.querySelectorAll("details[data-fold-key]").forEach((details) => {
      const key = details.getAttribute("data-fold-key");
      if (key && openFolds.has(key)) details.open = true;
    });
    if (nearBottom) list.scrollTop = list.scrollHeight;
    return true;
  }
  terminalForm?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const commandInput = terminalForm.elements.command;
    const command = commandInput?.value.trim() || "";
    if (!command) return;
    if (commandInput) commandInput.value = "";
    terminalOutput.textContent += `\n$ ${command}\n`;
    terminalOutput.scrollTop = terminalOutput.scrollHeight;
    const body = new URLSearchParams(new FormData(terminalForm));
    body.set("command", command);
    try {
      const response = await fetch("/agents/terminal/run", {method:"POST", body, headers:{"Accept":"application/json"}});
      const data = response.ok ? await response.json() : {ok:false,status:`http ${response.status}`,output:""};
      if (terminalCwd && data.cwd) terminalCwd.textContent = data.cwd;
      const output = data.output || "";
      terminalOutput.textContent += output;
      if (output && !output.endsWith("\n")) terminalOutput.textContent += "\n";
      terminalOutput.textContent += `[${data.status || (data.ok ? "ok" : "failed")}]\n`;
    } catch (_) { terminalOutput.textContent += "[request failed]\n"; }
    terminalOutput.scrollTop = terminalOutput.scrollHeight;
    commandInput?.focus();
  });
  function activeCompletionToken() {
    if (!input) return null;
    const value = input.value;
    const cursor = input.selectionStart ?? value.length;
    const before = value.slice(0, cursor);
    const match = before.match(/(^|[\s([{])([/!#$])([A-Za-z0-9:_-]*)$/);
    if (!match) return null;
    const symbol = match[2];
    const kind = symbol === "$" ? "skill" : symbol === "#" ? "plugin" : "command";
    return {
      start: before.length - match[2].length - match[3].length,
      end: cursor,
      symbol,
      kind,
      typed: match[3].toLowerCase()
    };
  }
  function matchingSuggestions() {
    const token = activeCompletionToken();
    if (!token) return [];
    return composerSuggestions
      .filter((item) => item.kind === token.kind)
      .filter((item) => {
        const typed = token.typed;
        if (!typed) return true;
        const haystack = `${item.name} ${item.description} ${item.insert}`.toLowerCase();
        return haystack.includes(typed);
      })
      .slice(0, 24)
      .map((item) => ({...item, token}));
  }
  function suggestionInsert(item) {
    if (item.kind === "command") return `${item.token.symbol}${item.name}`;
    return item.insert;
  }
  function applyCommandSuggestion(item) {
    if (!input || !item || !item.token) return;
    const insert = suggestionInsert(item);
    const before = input.value.slice(0, item.token.start);
    const after = input.value.slice(item.token.end).replace(/^\s+/, "");
    const spacer = item.takes_arg ? " " : "";
    input.value = `${before}${insert}${spacer}${after}`;
    const caret = before.length + insert.length + spacer.length;
    input.focus();
    input.setSelectionRange(caret, caret);
    dirty = input.value.length > 0;
    renderCommandSuggestions();
  }
  function renderCommandSuggestions() {
    if (!suggestionBox) return;
    const matches = matchingSuggestions();
    if (!matches.length) {
      suggestionBox.hidden = true;
      suggestionBox.innerHTML = "";
      return;
    }
    selectedSuggestion = Math.min(selectedSuggestion, matches.length - 1);
    suggestionBox.hidden = false;
    suggestionBox.innerHTML = "";
    matches.forEach((command, index) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "command-suggestion" + (index === selectedSuggestion ? " active" : "");
      button.setAttribute("role", "option");
      const label = command.kind === "command" ? `${command.token.symbol}${command.name}` : command.insert;
      button.innerHTML = `<strong>${label}</strong><span>${command.kind} | ${command.description}</span>`;
      button.addEventListener("click", (event) => {
        event.preventDefault();
        selectedSuggestion = index;
        applyCommandSuggestion(command);
      });
      suggestionBox.appendChild(button);
      if (index === selectedSuggestion) button.scrollIntoView({block:"nearest"});
    });
  }
  function setEditMode(id, body) {
    if (!input || !editMessageId) return;
    editMessageId.value = id;
    input.value = body || "";
    dirty = input.value.length > 0;
    if (editStrip) editStrip.hidden = false;
    if (sendButton) sendButton.textContent = "Save";
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
    input.scrollIntoView({block:"nearest"});
    renderCommandSuggestions();
  }
  function clearEditMode() {
    if (editMessageId) editMessageId.value = "";
    if (editStrip) editStrip.hidden = true;
    if (sendButton) sendButton.textContent = "Send";
    if (input) {
      input.value = "";
      dirty = false;
      renderCommandSuggestions();
      input.focus();
    }
  }
  async function copyText(value) {
    if (!value) return false;
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(value);
      return true;
    }
    const area = document.createElement("textarea");
    area.value = value;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.left = "-9999px";
    area.style.top = "0";
    document.body.appendChild(area);
    area.focus();
    area.select();
    area.setSelectionRange(0, value.length);
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  }
  document.querySelector("[data-edit-clear]")?.addEventListener("click", clearEditMode);
  list?.addEventListener("click", (event) => {
    const copyButton = event.target.closest("[data-copy-code]");
    if (copyButton) {
      event.preventDefault();
      const code = copyButton.closest(".message-code")?.querySelector("code")?.textContent || "";
      copyText(code).then((ok) => {
        const original = copyButton.textContent || "Copy";
        copyButton.textContent = ok ? "Copied" : "Copy failed";
        window.setTimeout(() => copyButton.textContent = original, 1400);
      }).catch(() => {
        copyButton.textContent = "Copy failed";
        window.setTimeout(() => copyButton.textContent = "Copy", 1400);
      });
      return;
    }
    const button = event.target.closest("[data-edit-message]");
    if (!button) return;
    event.preventDefault();
    setEditMode(button.getAttribute("data-edit-message"), button.getAttribute("data-edit-body") || "");
  });
  form?.addEventListener("submit", (event) => {
    if (event.submitter?.getAttribute("name") === "control") {
      dirty = false;
      return;
    }
    if (input?.value.trim().toLowerCase() === "/model") {
      event.preventDefault();
      modelPicker?.focus();
    }
  });
  input.addEventListener("input", () => {
    dirty = input.value.length > 0;
    selectedSuggestion = 0;
    renderCommandSuggestions();
  });
  input.addEventListener("focus", () => setTimeout(() => input.scrollIntoView({block:"nearest"}), 80));
  input.addEventListener("keydown", (event) => {
    if (!suggestionBox || suggestionBox.hidden) return;
    const matches = matchingSuggestions();
    if (!matches.length) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      selectedSuggestion = (selectedSuggestion + 1) % matches.length;
      renderCommandSuggestions();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      selectedSuggestion = (selectedSuggestion + matches.length - 1) % matches.length;
      renderCommandSuggestions();
    } else if (event.key === "Tab" || event.key === "ArrowRight") {
      event.preventDefault();
      applyCommandSuggestion(matches[selectedSuggestion]);
    } else if (event.key === "Escape") {
      suggestionBox.hidden = true;
    }
  });
  async function poll() {
    if (!list || dirty) {
      scheduleAgentPoll(1800);
      return;
    }
    try {
      const response = await fetch("/agents/slots/{active_slot_id}/state", {cache:"no-store"});
      if (response.ok) {
        const data = await response.json();
        const replaced = replaceMessages(data.messages_html);
        status.textContent = data.active_status || (data.running ? (data.current || "running") : "idle");
        count.textContent = data.message_count + " messages";
        if (cancelButton) cancelButton.disabled = !data.running;
        scheduleAgentPoll(!replaced ? 1200 : data.running ? 1200 : 4000);
        return;
      }
    } catch (_) {}
    scheduleAgentPoll(4000);
  }
  function renderSlotStates(slots) {
    let anyRunning = false;
    for (const slot of slots || []) {
      const id = String(slot.id);
      const entry = slotRows.get(id);
      if (!entry) continue;
      const label = slot.running ? (slot.current || "running") : (slot.status || "idle");
      anyRunning = anyRunning || !!slot.running;
      if (entry.status) entry.status.textContent = label;
      if (id === activeSlotId && cancelButton) cancelButton.disabled = !slot.running;
      entry.row.setAttribute("data-slot-running", slot.running ? "true" : "false");
      entry.row.classList.toggle("running", !!slot.running);
      if (slot.running) {
        entry.row.classList.remove("done");
        if (entry.badge) entry.badge.hidden = true;
      } else if (entry.wasRunning) {
        entry.row.classList.add("done");
        if (entry.badge) {
          entry.badge.textContent = "done";
          entry.badge.hidden = false;
        }
        if (id === activeSlotId && status) status.textContent = "done";
      }
      entry.wasRunning = !!slot.running;
    }
    return anyRunning;
  }
  async function pollSlots() {
    try {
      const response = await fetch("/agents/slots/state", {cache:"no-store"});
      if (response.ok) {
        const data = await response.json();
        const anyRunning = renderSlotStates(data.slots || []);
        scheduleSlotPoll(anyRunning ? 1200 : 4000);
        return;
      }
    } catch (_) {}
    scheduleSlotPoll(5000);
  }
  function refreshVisibleState() {
    if (document.visibilityState === "hidden") return;
    scheduleSlotPoll(0);
    if (!viewingTranscript) scheduleAgentPoll(0);
  }
  if (list) list.scrollTop = list.scrollHeight;
  window.addEventListener("pageshow", refreshVisibleState);
  window.addEventListener("focus", refreshVisibleState);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") refreshVisibleState();
  });
  scheduleSlotPoll(1000);
  if (!viewingTranscript) scheduleAgentPoll(1200);
})();
