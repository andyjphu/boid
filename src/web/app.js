"use strict";

const workspace = document.querySelector("#workspace");
const empty = document.querySelector("#empty");
const workspaceError = document.querySelector("#workspace-error");
const login = document.querySelector("#login");
const sessions = new Map();
const communications = new Map();
const communicationEntries = new Map();
const composerPadding = 8;
let renderSelection;

function rememberCommunication(message) {
  communications.set(message.id, message);
  communicationEntries.set(`${message.sender_id}:${message.sender_entry}`, message);
  communicationEntries.set(`${message.recipient_id}:${message.id}`, message);
}
const terminals = new Map();
const terminalCatalog = new Map();
let selected = null;
let lastSequence = -1;
// Local revisions remain ordered when a restarted server resets its sequence.
let catalogRevision = 0;
const sessionLifecycle = new Map();
let events;
const drafts = new Map();
let paneOrder = [];
let orderKey = "";
let catalogReady = false;
let reconcilingCatalog = false;
let launchCwd = "";

function loadPaneOrder(cwd) {
  const key = `boid:pane-order:v1:${location.origin}:${cwd || ""}`;
  if (key === orderKey) return;
  orderKey = key;
  try {
    const saved = JSON.parse(sessionStorage.getItem(key) || "null");
    const valid = key => typeof key === "string" && /^(agent|terminal|draft):.+/.test(key);
    paneOrder = [...new Set((Array.isArray(saved) ? saved : saved?.order || []).filter(valid))];
    for (const draft of saved?.drafts || []) {
      if (draft && typeof draft.id === "string" && ["name", "cwd"].every(key => typeof draft[key] === "string")) {
        const restored = { ...draft };
        if (saved.version !== 2) {
          for (const [key, value] of Object.entries(agentDefaults)) if (restored[key] === value) restored[key] = "";
        }
        addSetup(restored);
      }
    }
  } catch { paneOrder = []; }
}

function rememberPaneOrder() {
  if (!catalogReady || reconcilingCatalog) return;
  const panes = new Map([...workspace.querySelectorAll(".pane")].map(pane => [pane.dataset.paneKey, pane]));
  paneOrder = paneOrder.filter(key => panes.has(key));
  for (const key of panes.keys()) if (!paneOrder.includes(key)) paneOrder.push(key);
  for (let i = 0; i < paneOrder.length; i++) {
    const pane = panes.get(paneOrder[i]);
    const current = workspace.querySelectorAll(".pane")[i];
    if (pane !== current) workspace.insertBefore(pane, current || null);
  }
  try {
    sessionStorage.setItem(orderKey, JSON.stringify({
      version: 2, order: paneOrder,
      drafts: [...drafts.values()].map(ui => ({ id: ui.id, ...Object.fromEntries(new FormData(ui.form)) })),
    }));
  } catch { /* Storage availability must not prevent session access. */ }
}

function disableTextAssistance(input) {
  for (const [name, value] of Object.entries({
    spellcheck: "false", autocorrect: "off", autocapitalize: "off",
    writingsuggestions: "false", autocomplete: "off",
  })) input.setAttribute(name, value);
}

document.querySelectorAll("input, textarea").forEach(disableTextAssistance);

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (tag === "input" || tag === "textarea") disableTextAssistance(node);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function showError(node, error) {
  node.textContent = error instanceof Error ? error.message : String(error);
  node.hidden = false;
}

async function request(path, method = "GET", body, headers = {}) {
  const options = { method, credentials: "same-origin", headers };
  if (body !== undefined) {
    options.headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  const text = await response.text();
  let result;
  try { result = text ? JSON.parse(text) : {}; } catch { result = { error: text }; }
  if (!response.ok) {
    const error = new Error(result.error || result.message || `Request failed (${response.status}).`);
    error.status = response.status;
    throw error;
  }
  return result;
}

function layout(event) {
  rememberPaneOrder();
  if (event?.type !== "resize") {
    for (const ui of sessions.values()) rememberPosition(ui);
  }
  for (const ui of sessions.values()) ui.reflow = true;
  const panes = [...workspace.querySelectorAll(".pane")];
  empty.hidden = panes.length > 0;
  const columns = Math.max(1, Math.min(panes.length, Math.floor(workspace.clientWidth / 420)));
  workspace.style.setProperty("--columns", columns);
  workspace.style.setProperty("--rows", Math.max(1, Math.ceil(panes.length / columns)));
  requestAnimationFrame(() => {
    for (const ui of sessions.values()) restorePosition(ui);
    for (const ui of sessions.values()) ui.reflow = false;
  });
}

// Microsoft Codicons 0.0.36, CC-BY-4.0; attribution in vendor/codicons-NOTICE.
function paneIcon(kind) {
  const icon = element("span", "pane-icon");
  icon.setAttribute("aria-hidden", "true");
  icon.innerHTML = kind === "terminal" ? "<svg width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" xmlns=\"http://www.w3.org/2000/svg\" fill=\"currentColor\"><path fill-rule=\"evenodd\" clip-rule=\"evenodd\" d=\"M1.5 3L3 1.5H21L22.5 3V21L21 22.5H3L1.5 21V3ZM3 3V21H21V3H3Z\"/><path d=\"M7.06078 7.49988L6.00012 8.56054L10.2427 12.8032L6 17.0459L7.06066 18.1066L12 13.1673V12.4391L7.06078 7.49988Z\"/><rect x=\"12\" y=\"16.5\" width=\"6\" height=\"1.5\"/></svg>" : "<svg width=\"16\" height=\"16\" viewBox=\"0 0 16 16\" xmlns=\"http://www.w3.org/2000/svg\" fill=\"currentColor\"><path fill-rule=\"evenodd\" clip-rule=\"evenodd\" d=\"M8.48 4h4l.5.5v2.03h.52l.5.5V8l-.5.5h-.52v3l-.5.5H9.36l-2.5 2.76L6 14.4V12H3.5l-.5-.64V8.5h-.5L2 8v-.97l.5-.5H3V4.36L3.53 4h4V2.86A1 1 0 0 1 7 2a1 1 0 0 1 2 0 1 1 0 0 1-.52.83V4zM12 8V5H4v5.86l2.5.14H7v2.19l1.8-2.04.35-.15H12V8zm-2.12.51a2.71 2.71 0 0 1-1.37.74v-.01a2.71 2.71 0 0 1-2.42-.74l-.7.71c.34.34.745.608 1.19.79.45.188.932.286 1.42.29a3.7 3.7 0 0 0 2.58-1.07l-.7-.71zM6.49 6.5h-1v1h1v-1zm3 0h1v1h-1v-1z\"/></svg>";
  return icon;
}

function makePane(name, cwd, kind, id) {
  const pane = element("section", `pane ${kind}-pane`);
  pane.setAttribute("aria-label", name);
  pane.dataset.paneKey = `${kind}:${id}`;
  const header = element("header", "pane-header");
  const tab = element("div", "pane-tab");
  const title = element("span", "pane-title", name);
  const status = element("span", "pane-status", "idle");
  const directory = element("p", "pane-cwd", cwd || "Server working directory");
  directory.title = cwd || "Server working directory";
  const error = element("p", "error-banner");
  error.setAttribute("role", "alert");
  error.hidden = true;
  const close = element("button", "pane-close");
  close.innerHTML = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.3"/></svg>';
  close.type = "button";
  close.setAttribute("aria-label", `Close ${name}`);
  close.title = kind === "agent" ? "End and close this agent" : "End and close this terminal";
  tab.append(paneIcon(kind), title, directory, close);
  header.append(tab, status);
  pane.append(header);
  workspace.append(pane);
  return { pane, header, tab, title, status, directory, error, close };
}

function enablePaneRename(ui, kind) {
  ui.title.title = "Double-click or press F2 to rename";
  ui.title.tabIndex = 0;
  ui.setName = name => {
    ui.nameRevision = (ui.nameRevision || 0) + 1;
    ui.name = name || ui.id;
    ui.title.textContent = ui.name;
    ui.pane.setAttribute("aria-label", ui.name);
    ui.close.setAttribute("aria-label", `Close ${ui.name}`);
    if (ui.view) ui.view.name = ui.name;
    if (ui.input) ui.input.setAttribute("aria-label", `Message ${ui.name}`);
  };
  ui.setName(ui.title.textContent);
  let editing = false;
  function rename() {
    if (editing) return;
    editing = true;
    const input = element("input", "pane-name-input");
    input.type = "text";
    input.value = ui.name;
    input.maxLength = 128;
    input.size = Math.max(4, Math.min(24, ui.name.length + 2));
    input.setAttribute("aria-label", kind === "agent" ? "Agent name" : "Terminal name");
    ui.title.hidden = true;
    ui.title.after(input);
    let saving = false;
    function finish() {
      if (!editing) return;
      editing = false;
      input.remove();
      ui.title.hidden = false;
    }
    input.addEventListener("keydown", async event => {
      if (event.isComposing || !["Enter", "Escape"].includes(event.key)) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") { finish(); ui.title.focus({ preventScroll: true }); return; }
      if (saving) return;
      const name = input.value.trim();
      if (!name) { input.select(); return; }
      saving = true;
      input.disabled = true;
      const revision = ui.nameRevision;
      try {
        const summary = await request(`/api/${kind === "agent" ? "sessions" : "terminals"}/${encodeURIComponent(ui.id)}`, "PATCH", { name });
        // A newer catalog event takes precedence over a delayed PATCH response.
        if (ui.nameRevision === revision) ui.setName(summary.name);
        ui.error.hidden = true;
        finish();
        ui.title.focus({ preventScroll: true });
      } catch (error) {
        saving = false;
        input.disabled = false;
        showError(ui.error, error);
        input.focus({ preventScroll: true });
      }
    });
    input.addEventListener("blur", () => { if (!saving) finish(); });
    input.focus({ preventScroll: true });
    input.select();
  }
  ui.title.addEventListener("dblclick", rename);
  ui.title.addEventListener("keydown", event => {
    if (event.key === "F2") { event.preventDefault(); rename(); }
  });
}

function reportUnsavedEnd(result) {
  if (result.persisted === false) {
    showError(workspaceError, "The agent ended, but storage failed. Its final state could not be saved.");
  }
}

function removeSession(ui) {
  if (loginOwner === ui) closeLogin();
  ui.skillRequest = (ui.skillRequest || 0) + 1;
  ui.traceResize.disconnect();
  ui.overlayResize.disconnect();
  if (selected === ui.pane) selected = null;
  ui.pane.remove();
  sessions.delete(ui.id);
  layout();
}

function messageBottom(ui) {
  // Keep the usual trace padding, but never follow into the composer reserve.
  return Math.max(0, ui.content.offsetHeight + 20 - ui.trace.clientHeight);
}

function atBottom(ui) {
  return ui.trace.scrollTop >= messageBottom(ui) - 1;
}

function promptVisible(ui) {
  return !ui.composer.hidden && (ui.pane.matches(":hover")
    || ui.composer.contains(document.activeElement) || ui.input.value.length > 0);
}

function followTarget(ui) {
  const clearance = ui.autoClearance && promptVisible(ui)
    ? ui.overlay.getBoundingClientRect().height : 0;
  const bottom = messageBottom(ui);
  const target = Math.max(bottom + Math.max(ui.followClearance, clearance),
    ui.retainedPromptEnd - ui.trace.clientHeight);
  if (bottom + ui.trace.clientHeight >= ui.retainedPromptEnd) ui.retainedPromptEnd = 0;
  // Keep enough scrollable space while new content fills the vacated prompt area.
  const reserve = Math.max(ui.overlay.getBoundingClientRect().height,
    target - bottom - composerPadding);
  ui.trace.style.setProperty("--composer-space", `${Math.ceil(reserve) + composerPadding}px`);
  return target;
}

function retainPromptSpace(ui) {
  if (!ui.follow || selectedWithin(ui.trace) || ui.trace.scrollTop <= messageBottom(ui)) return;
  ui.retainedPromptEnd = Math.max(ui.retainedPromptEnd,
    ui.trace.scrollTop + ui.trace.clientHeight);
}

function clearPromptSpaceWhenHidden(ui) {
  if (promptVisible(ui)) return;
  const hadClearance = ui.autoClearance || ui.followClearance > 0;
  retainPromptSpace(ui);
  ui.autoClearance = false;
  ui.followClearance = 0;
  if (hadClearance && ui.follow && !selectedWithin(ui.trace)) restorePosition(ui);
}

function activatePromptSpace(ui) {
  if (!ui.follow || selectedWithin(ui.trace) || !promptVisible(ui)) return;
  ui.autoClearance = true;
  restorePosition(ui);
}

function selectionRange() {
  const selection = window.getSelection();
  return selection && !selection.isCollapsed && selection.rangeCount > 0
    ? selection.getRangeAt(0).cloneRange() : null;
}

function selectedWithin(node) {
  if (!node.isConnected) return false;
  const range = renderSelection === undefined ? selectionRange() : renderSelection;
  return Boolean(range?.intersectsNode(node));
}

function withRenderSelection(render) {
  const previous = renderSelection;
  renderSelection = selectionRange();
  try { render(); } finally { renderSelection = previous; }
}

function rememberPosition(ui) {
  if (ui.follow) { ui.anchor = null; return; }
  const top = ui.trace.getBoundingClientRect().top;
  const anchor = ui.anchor?.node;
  if (anchor?.isConnected && !anchor.hidden) {
    const bounds = anchor.getBoundingClientRect();
    if (bounds.top <= top && bounds.bottom > top) {
      ui.anchor.offset = bounds.top - top;
      return;
    }
  }
  const node = [...ui.trace.querySelectorAll(".entry")].find(node =>
    !node.hidden && node.getBoundingClientRect().bottom > top);
  ui.anchor = node ? { node, offset: node.getBoundingClientRect().top - top } : null;
}

function scrollTrace(ui, top) {
  if (Math.abs(ui.trace.scrollTop - top) > 0.5) ui.trace.scrollTop = top;
  // Programmatic movement must not be interpreted as a new reading gesture.
  ui.lastScrollTop = ui.trace.scrollTop;
}

function atTraceEnd(ui) {
  return ui.trace.scrollTop >= ui.trace.scrollHeight - ui.trace.clientHeight - 1;
}

function restorePosition(ui) {
  if (ui.follow && !selectedWithin(ui.trace)) {
    scrollTrace(ui, followTarget(ui));
  } else if (ui.anchor?.node.isConnected) {
    scrollTrace(ui, ui.trace.scrollTop + ui.anchor.node.getBoundingClientRect().top
      - ui.trace.getBoundingClientRect().top - ui.anchor.offset);
  }
}

document.addEventListener("selectionchange", () => {
  for (const ui of sessions.values()) {
    if (selectedWithin(ui.trace)) {
      ui.follow = false;
      rememberPosition(ui);
    }
    for (const item of ui.entries.values()) {
      if (item.superseded && !selectedWithin(item.node)) item.node.hidden = true;
      if (item.deferred && !selectedWithin(item.node)) {
        item.deferred = false;
        renderEntry(ui, item);
        item.rendered = true;
      }
    }
    groupUserEntries(ui);
    restorePosition(ui);
    if (ui.deferredNavigation && !selectedWithin(ui.trace)) {
      const { direction, entryId } = ui.deferredNavigation; ui.deferredNavigation = null;
      loadHistory(ui, direction, entryId);
    }
  }
});

function followLatest(ui) {
  if (ui.followScheduled) return;
  ui.followScheduled = true;
  requestAnimationFrame(() => {
    ui.followScheduled = false;
    if (ui.follow && !selectedWithin(ui.trace)) {
      scrollTrace(ui, followTarget(ui));
    }
  });
}

function closeModelPicker(ui) {
  ui.modelPicker.hidden = true;
  ui.composer.hidden = false;
  updateActivity(ui);
  ui.input.focus({ preventScroll: true });
}

function updateProfile(ui, profile, contextWindow = ui.view.context_window) {
  ui.view.profile = profile; ui.view.context_window = contextWindow;
  updateContextUsage(ui);
}
async function openModelPicker(ui) {
  ui.slashSuggestion.hidden = true; ui.composer.hidden = true; ui.modelPicker.hidden = false;
  clearPromptSpaceWhenHidden(ui); updateActivity(ui);
  const generation = ui.modelGeneration = (ui.modelGeneration || 0) + 1;
  ui.modelPicker.replaceChildren(element("div", "model-heading", "Select model"), element("div", "model-hint", "Loading…"));
  ui.modelPicker.focus({ preventScroll: true });
  try {
    const [view, catalog] = await Promise.all([
      request(`/api/sessions/${encodeURIComponent(ui.id)}/history`),
      request(`/api/sessions/${encodeURIComponent(ui.id)}/models`),
    ]);
    if (ui.modelPicker.hidden || generation !== ui.modelGeneration) return;
    updateProfile(ui, view.profile, view.context_window);
    const choices = element("div", "model-options"); choices.setAttribute("role", "listbox"); choices.setAttribute("aria-label", "Model");
    for (const model of catalog.models) {
      const current = model.slug === view.profile.model;
      const option = element("button", "model-option", model.display_name + (current ? " (current)" : ""));
      option.type = "button"; option.setAttribute("role", "option"); option.setAttribute("aria-selected", String(current));
      option.addEventListener("click", async () => {
        if (ui.modelSaving) return;
        ui.modelSaving = true; option.disabled = true;
        try {
          const result = await request(`/api/sessions/${encodeURIComponent(ui.id)}/model`, "POST", { model: model.slug });
          updateProfile(ui, result.profile, result.context_window);
          if (generation === ui.modelGeneration) closeModelPicker(ui);
        } catch (error) { showError(ui.error, error); }
        finally { ui.modelSaving = false; option.disabled = false; }
      });
      choices.append(option);
    }
    ui.modelPicker.replaceChildren(element("div", "model-heading", "Select model"), choices);
    (choices.querySelector('[aria-selected="true"]') || choices.firstElementChild)?.focus({ preventScroll: true });
  } catch (error) {
    if (!ui.modelPicker.hidden && generation === ui.modelGeneration) { closeModelPicker(ui); showError(ui.error, error); }
  }
}

function pickerOpen(ui) {
  return loginOwner === ui || !ui.modelPicker.hidden || !ui.skillPicker.hidden;
}

function closeSkillPicker(ui) {
  ui.skillRequest = (ui.skillRequest || 0) + 1;
  ui.skillPicker.hidden = true;
  ui.composer.hidden = false;
  updateActivity(ui);
  ui.input.focus({ preventScroll: true });
}

async function openSkillPicker(ui) {
  const generation = ui.skillRequest = (ui.skillRequest || 0) + 1;
  const current = () => sessions.get(ui.id) === ui && ui.skillRequest === generation && !ui.skillPicker.hidden;
  ui.slashSuggestion.hidden = true;
  ui.composer.hidden = true;
  ui.modelPicker.hidden = true;
  ui.skillPicker.hidden = false;
  clearPromptSpaceWhenHidden(ui);
  updateActivity(ui);
  ui.skillPicker.replaceChildren(element("div", "model-hint", "Loading skills…"));
  ui.skillPicker.focus({ preventScroll: true });
  try {
    const catalog = await request(`/api/sessions/${encodeURIComponent(ui.id)}/skills`);
    if (!current()) return;
    ui.skillPicker.replaceChildren();
    const filter = element("input", "skill-filter");
    filter.placeholder = "Find a skill…";
    filter.setAttribute("aria-label", "Find a skill");
    const choices = element("div", "skill-options");
    const render = () => {
      choices.replaceChildren();
      const query = filter.value.toLowerCase();
      for (const skill of catalog.skills.filter(s => `${s.name} ${s.description} ${s.source}`.toLowerCase().includes(query))) {
        const option = element("button", "skill-option");
        option.type = "button";
        option.disabled = !skill.enabled || !catalog.refresh_on_message;
        option.append(element("span", "skill-name", skill.name + (!skill.enabled ? " · disabled" : !skill.allow_implicit_invocation ? " · explicit only" : "")),
          element("span", "skill-source", skill.source), element("span", "skill-description", skill.description));
        for (const warning of skill.warnings) option.append(element("span", "skill-warning", warning));
        option.addEventListener("click", () => {
          const mention = `$[${skill.id}] `;
          ui.input.setRangeText(mention, ui.input.selectionStart, ui.input.selectionEnd, "end");
          closeSkillPicker(ui);
          ui.input.dispatchEvent(new Event("input", { bubbles: true }));
        });
        choices.append(option);
      }
      if (!choices.children.length) choices.append(element("div", "model-hint", "No matching skills."));
    };
    filter.addEventListener("input", render);
    ui.skillPicker.append(filter, choices);
    for (const warning of catalog.warnings) ui.skillPicker.append(element("div", "skill-warning", warning));
    if (!catalog.refresh_on_message) ui.skillPicker.append(element("div", "model-hint", "This older session retains its original skill environment. Create a new session to select local skills."));
    ui.skillPicker.append(element("div", "model-hint", "Enter select · Esc back"));
    render();
    filter.focus({ preventScroll: true });
  } catch (error) { if (current()) { closeSkillPicker(ui); showError(ui.error, error); } }
}

const tokenFormat = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 2 });
const usageCanvas = document.createElement("canvas").getContext("2d");

function updateContextUsage(ui) {
  const usage = ui.view.context_usage;
  const used = usage && Number.isSafeInteger(usage.input_tokens) && Number.isSafeInteger(usage.output_tokens)
    ? usage.input_tokens + usage.output_tokens : null;
  const limit = ui.view.context_window;
  const knownLimit = Number.isSafeInteger(limit) && limit > 0;
  const percent = used !== null && knownLimit ? 100 * used / limit : null;
  ui.tab.style.setProperty("--context-fill", `${Math.min(100, Math.max(0, percent || 0))}%`);
  const amount = used === null ? "Context —" : knownLimit
    ? `${tokenFormat.format(used)} / ${tokenFormat.format(limit)} · ${percent.toFixed(1)}%`
    : `${tokenFormat.format(used)} tokens`;
  ui.contextLabel.textContent = amount;
  const detail = used === null ? "Context usage available after the next model response."
    : `Last response: ${usage.input_tokens.toLocaleString("en")} input + ${usage.output_tokens.toLocaleString("en")} output tokens. Drafts and subsequent input are not included.`;
  ui.contextLabel.title = detail;
  ui.tab.title = detail;
  updateContextVisibility(ui);
}

function updateContextVisibility(ui) {
  if (!ui.contextLabel || !usageCanvas) return;
  const style = getComputedStyle(ui.input);
  usageCanvas.font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
  const firstLine = (ui.input.value || ui.input.placeholder).split("\n", 1)[0]
    .replace(/\t/g, " ".repeat(parseInt(style.tabSize, 10) || 8));
  const width = usageCanvas.measureText(firstLine).width
    + Math.max(0, firstLine.length - 1) * (parseFloat(style.letterSpacing) || 0);
  const textStart = ui.input.getBoundingClientRect().left + parseFloat(style.paddingLeft);
  const available = ui.contextLabel.getBoundingClientRect().left - textStart - 12;
  ui.contextLabel.style.visibility = ui.input.scrollTop > 0 || width >= available ? "hidden" : "visible";
}

function addSession(view) {
  const id = String(view.id);
  if (sessions.has(id)) return sessions.get(id);
  const ui = makePane(view.name || id, view.cwd, "agent", id);
  ui.view = view;
  ui.history = view.history ? { ...view.history } : null;
  ui.historyGeneration = 0;
  for (const message of view.communications || []) rememberCommunication(message);
  ui.id = id;
  enablePaneRename(ui, "agent");
  ui.entries = new Map();
  ui.toolPairs = new Map();
  ui.messageResults = new Map();
  ui.mounted = new Set();
  ui.dirty = new Set();
  ui.pending = new Set(view.pending || []);
  ui.follow = true;
  ui.followClearance = 0;
  ui.autoClearance = false;
  ui.retainedPromptEnd = 0;
  ui.overlayHeight = 0;
  ui.trace = element("div", "trace");
  ui.trace.tabIndex = 0;
  ui.lastScrollTop = 0;
  ui.trace.setAttribute("aria-label", "Live agent trace");
  ui.content = element("div", "trace-content");
  ui.historyBefore = element("button", "history-more", "Load earlier messages");
  ui.historyAfter = element("button", "history-more", "Load newer messages");
  ui.historyLatest = element("button", "history-latest", "Latest messages");
  for (const button of [ui.historyBefore, ui.historyAfter, ui.historyLatest]) { button.type = "button"; button.hidden = true; }
  ui.spacerBefore = element("div", "history-spacer");
  ui.spacerAfter = element("div", "history-spacer");
  ui.content.append(ui.historyBefore, ui.spacerBefore, ui.spacerAfter, ui.historyAfter);
  ui.trace.append(ui.content);
  ui.historyBefore.onclick = () => loadHistory(ui, "before");
  ui.historyAfter.onclick = () => loadHistory(ui, "after");
  ui.historyLatest.onclick = () => { if (selectedWithin(ui.trace)) getSelection().removeAllRanges(); loadHistory(ui, "latest"); };
  ui.activity = element("div", "working-status");
  ui.activity.setAttribute("role", "status");
  ui.activity.append(element("span", "working-label", "Working"), " · Esc to interrupt");
  ui.modelPicker = element("div", "model-picker");
  ui.modelPicker.hidden = true;
  ui.modelPicker.tabIndex = -1;
  ui.modelPicker.setAttribute("role", "dialog");
  ui.modelPicker.setAttribute("aria-label", "Select model and effort");
  ui.modelPicker.addEventListener("keydown", event => {
    if (event.isComposing) return;
    if (["Escape", "Enter", "ArrowUp", "ArrowDown", "Tab"].includes(event.key)) {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") closeModelPicker(ui);
      else {
        const options = [...ui.modelPicker.querySelectorAll(".model-option")];
        const current = options.indexOf(document.activeElement);
        if (event.key === "Enter") options[current]?.click();
        else options[(current + (event.key === "ArrowUp" || event.shiftKey ? -1 : 1) + options.length) % options.length]?.focus({ preventScroll: true });
      }
    }
  });
  ui.slashSuggestion = element("div", "slash-suggestion");
  ui.slashSuggestion.id = `slash-${id}`;
  ui.slashSuggestion.setAttribute("role", "listbox");
  ui.slashSuggestion.setAttribute("aria-label", "Slash commands");
  ui.slashSuggestion.hidden = true;
  ui.slashSuggestion.addEventListener("mousedown", event => event.preventDefault());
  ui.slashSuggestion.addEventListener("click", event => {
    const option = event.target.closest(".slash-option");
    if (!option) return;
    ui.input.value = option.dataset.command;
    resizeInput();
    ui.composer.requestSubmit();
  });
  ui.skillPicker = element("div", "skill-picker");
  ui.skillPicker.hidden = true;
  ui.skillPicker.tabIndex = -1;
  ui.skillPicker.setAttribute("role", "dialog");
  ui.skillPicker.setAttribute("aria-label", "Select skill");
  ui.skillPicker.addEventListener("keydown", event => {
    if (event.isComposing || !["Escape", "Enter", "ArrowUp", "ArrowDown"].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") { closeSkillPicker(ui); return; }
    const options = [...ui.skillPicker.querySelectorAll(".skill-option:not(:disabled)")];
    const index = options.indexOf(document.activeElement);
    if (event.key === "Enter") (options[index < 0 ? 0 : index])?.click();
    else if (options.length) {
      const next = index < 0 ? (event.key === "ArrowDown" ? 0 : options.length - 1) : (index + (event.key === "ArrowDown" ? 1 : -1) + options.length) % options.length;
      options[next].focus();
    }
  });
  ui.composer = element("form", "composer");
  ui.input = element("textarea");
  ui.input.rows = 1;
  ui.input.placeholder = "Message…";
  ui.input.setAttribute("aria-label", `Message ${view.name || id}`);
  ui.sending = false;
  ui.trace.addEventListener("scroll", () => {
    const top = ui.trace.scrollTop;
    const movedDown = top > ui.lastScrollTop;
    ui.lastScrollTop = top;
    if (ui.reflow || ui.trace.clientHeight !== ui.traceHeight) return;
    const wasFollowing = ui.follow;
    ui.follow = (wasFollowing ? atBottom(ui) : movedDown && atTraceEnd(ui))
      && !selectedWithin(ui.trace) && (!ui.history || ui.history.end >= ui.history.total);
    ui.followClearance = ui.follow && promptVisible(ui)
      ? Math.min(ui.overlay.getBoundingClientRect().height + composerPadding,
        Math.max(0, ui.trace.scrollTop - messageBottom(ui))) : 0;
    rememberPosition(ui);
    if (ui.follow && !wasFollowing) scheduleTranscript(ui);
    if (ui.history) {
      scheduleViewport(ui);
      if (top < 240 && ui.history.start > 0) loadHistory(ui, "before");
      else if (movedDown && atTraceEnd(ui) && ui.history.end < ui.history.total) loadHistory(ui, "after");
    }
  }, { passive: true });
  ui.trace.addEventListener("wheel", event => {
    if (event.deltaY < 0) {
      ui.follow = false;
      ui.lastScrollTop = ui.trace.scrollTop;
      rememberPosition(ui);
    } else if (event.deltaY > 0 && !ui.follow && atTraceEnd(ui) && !selectedWithin(ui.trace) && (!ui.history || ui.history.end >= ui.history.total)) {
      ui.follow = true;
      scheduleTranscript(ui);
    }
  }, { passive: true });
  ui.trace.addEventListener("keydown", event => {
    if (["ArrowUp", "PageUp", "Home"].includes(event.key)) {
      ui.follow = false;
      ui.lastScrollTop = ui.trace.scrollTop;
      rememberPosition(ui);
    }
  });
  ui.contextLabel = element("span", "context-usage");
  ui.composer.append(ui.input, ui.contextLabel);
  ui.input.addEventListener("scroll", () => updateContextVisibility(ui), { passive: true });
  const stream = element("div", "agent-stream");
  const overlay = element("div", "composer-overlay");
  ui.overlay = overlay;
  overlay.append(ui.slashSuggestion, ui.composer);
  stream.append(ui.trace, overlay, ui.historyLatest);
  ui.pane.append(stream, ui.error, ui.activity, ui.modelPicker, ui.skillPicker);
  const overlayResize = new ResizeObserver(() => {
    const height = Math.ceil(overlay.getBoundingClientRect().height);
    if (height < ui.overlayHeight) {
      retainPromptSpace(ui);
      ui.followClearance = Math.min(ui.followClearance, height + composerPadding);
    }
    ui.overlayHeight = height;
    followTarget(ui);
    if (ui.autoClearance || ui.retainedPromptEnd) restorePosition(ui);
    updateContextVisibility(ui);
  });
  overlayResize.observe(overlay);
  ui.overlayResize = overlayResize;
  ui.pane.addEventListener("pointerleave", () => clearPromptSpaceWhenHidden(ui));
  ui.composer.addEventListener("focusout", () => {
    requestAnimationFrame(() => clearPromptSpaceWhenHidden(ui));
  });
  function resizeInput() {
    ui.input.style.height = "auto";
    ui.input.style.height = `${Math.min(ui.input.scrollHeight, 140)}px`;
    const text = ui.input.value.trim();
    const commands = {
      "/login": "/login  Choose API or subscription",
      "/model": "/model  Select model and effort",
      "/skills": "/skills  Select a skill",
      "/oai-compact": "/oai-compact  Compact OpenAI context",
      "/clear": "/clear  Start fresh model context",
    };
    ui.slashMatches = Object.keys(commands).filter(command => command.startsWith(text));
    if (!ui.slashMatches.includes(ui.slashCommand)) ui.slashCommand = ui.slashMatches[0];
    ui.slashSuggestion.replaceChildren(...ui.slashMatches.map(command => {
      const option = element("div", "slash-option", commands[command]);
      option.id = `slash-${id}-${command.slice(1)}`;
      option.dataset.command = command;
      option.setAttribute("role", "option");
      option.setAttribute("aria-selected", String(command === ui.slashCommand));
      return option;
    }));
    ui.slashSuggestion.hidden = ui.slashDismissed || !text || !ui.slashCommand || ui.input.value.includes("\n");
    ui.input.setAttribute("aria-controls", ui.slashSuggestion.id);
    ui.input.setAttribute("aria-expanded", String(!ui.slashSuggestion.hidden));
    if (!ui.slashSuggestion.hidden) ui.input.setAttribute("aria-activedescendant", `slash-${id}-${ui.slashCommand.slice(1)}`);
    else ui.input.removeAttribute("aria-activedescendant");
    clearPromptSpaceWhenHidden(ui);
    updateContextVisibility(ui);
  }
  let historyIndex = null;
  let savedDraft = "";
  ui.input.addEventListener("input", () => { historyIndex = null; ui.slashDismissed = false; ui.slashCommand = null; resizeInput(); activatePromptSpace(ui); });
  ui.traceHeight = ui.trace.clientHeight;
  const traceResize = new ResizeObserver(() => {
    ui.traceHeight = ui.trace.clientHeight;
    restorePosition(ui);
    updateContextVisibility(ui);
  });
  traceResize.observe(ui.trace);
  ui.traceResize = traceResize;
  ui.close.addEventListener("click", async () => {
    ui.close.disabled = true;
    try {
      const result = await request(`/api/sessions/${encodeURIComponent(id)}`, "DELETE");
      reportUnsavedEnd(result);
      if (sessions.has(id)) removeSession(ui);
    } catch (error) { showError(ui.error, error); ui.close.disabled = false; }
  });
  ui.input.addEventListener("keydown", event => {
    if (!event.isComposing && !ui.slashSuggestion.hidden && !event.ctrlKey && !event.metaKey && !event.altKey) {
      if (["Escape", "Tab", "ArrowUp", "ArrowDown"].includes(event.key)) {
        event.preventDefault();
        if (event.key === "Escape") { ui.slashDismissed = true; resizeInput(); }
        if (event.key === "Tab") { ui.input.value = ui.slashCommand; resizeInput(); }
        if (event.key === "ArrowUp" || event.key === "ArrowDown") {
          const index = ui.slashMatches.indexOf(ui.slashCommand);
          ui.slashCommand = ui.slashMatches[(index + (event.key === "ArrowDown" ? 1 : -1) + ui.slashMatches.length) % ui.slashMatches.length];
          resizeInput();
        }
        return;
      }
    }
    if (!event.isComposing && !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey
        && ui.input.selectionStart === ui.input.selectionEnd) {
      const prompts = [...ui.entries.values()].filter(item => item.kind === "user").map(item => item.text);
      const up = event.key === "ArrowUp" && ui.input.selectionStart === 0;
      const down = event.key === "ArrowDown" && ui.input.selectionEnd === ui.input.value.length;
      if ((up && prompts.length) || (down && historyIndex !== null)) {
        event.preventDefault();
        if (historyIndex === null) { savedDraft = ui.input.value; historyIndex = prompts.length; }
        historyIndex = Math.max(0, Math.min(prompts.length, historyIndex + (up ? -1 : 1)));
        ui.input.value = historyIndex === prompts.length ? savedDraft : prompts[historyIndex];
        ui.input.setSelectionRange(up ? 0 : ui.input.value.length, up ? 0 : ui.input.value.length);
        resizeInput();
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      ui.composer.requestSubmit();
    }
  });
  ui.composer.addEventListener("submit", async event => {
    event.preventDefault();
    if (ui.sending || !ui.input.value.trim()) return;
    const text = ui.input.value;
    activatePromptSpace(ui);
    if (text.trim() === "/login" || (!ui.slashSuggestion.hidden && ui.slashCommand === "/login")) {
      ui.input.value = "";
      historyIndex = null;
      resizeInput();
      openLogin(ui);
      return;
    }
    if (text.trim() === "/skills" || (!ui.slashSuggestion.hidden && ui.slashCommand === "/skills")) {
      ui.input.value = "";
      historyIndex = null;
      resizeInput();
      await openSkillPicker(ui);
      return;
    }
    if (text.trim() === "/model" || (!ui.slashSuggestion.hidden && ui.slashCommand === "/model")) {
      ui.input.value = "";
      historyIndex = null;
      resizeInput();
      await openModelPicker(ui);
      return;
    }
    ui.sending = true;
    ui.error.hidden = true;
    try {
      const submitted = !ui.slashSuggestion.hidden ? ui.slashCommand : text;
      ui.submission = request(`/api/sessions/${encodeURIComponent(id)}/messages`, "POST", { text: submitted });
      await ui.submission;
      if (ui.input.value === text) { ui.input.value = ""; historyIndex = null; resizeInput(); }
    } catch (error) { showError(ui.error, error); }
    finally { ui.sending = false; ui.submission = null; }
  });
  sessions.set(id, ui);
  updateContextUsage(ui);
  setStatus(ui, view.status);
  for (const [index, entry] of (view.entries || []).entries()) putEntry(ui, { ...entry, history_index: (view.history?.start || 0) + index });
  setPending(ui, view.pending || []);
  followLatest(ui);
  layout();
  return ui;
}

function updateActivity(ui) {
  const since = ui.view?.working_since_ms;
  const seconds = Number.isFinite(since) ? Math.max(0, Math.floor((Date.now() - since) / 1000)) : null;
  const elapsed = seconds === null ? "" : seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  const hint = ui.interruptRequested ? "Interrupt requested" : pickerOpen(ui) ? "" : "Esc to interrupt";
  ui.activity.lastChild.textContent = elapsed ? ` (${elapsed}${hint ? ` · ${hint}` : ""})` : (hint ? ` · ${hint}` : "");
}

function setStatus(ui, status, workingSince = ui.view?.working_since_ms) {
  if (status !== "working") {
    ui.interruptRequested = false;
  }
  if (ui.view) {
    ui.view.status = status;
    ui.view.working_since_ms = status === "working" ? workingSince : null;
  }
  ui.pane.dataset.status = status || "idle";
  updateActivity(ui);
  ui.status.textContent = "";
}

function toolEntry(text) {
  const split = text.indexOf("\n");
  const heading = split < 0 ? text : text.slice(0, split);
  const payload = split < 0 ? "" : text.slice(split + 1);
  const identity = /^(.+) \(([^()\r\n]+)\)$/.exec(heading);
  let value;
  try { value = JSON.parse(payload); } catch { /* Partial output stays visible. */ }
  return { heading, payload, value, name: identity?.[1] || heading, callId: identity?.[2] };
}

function toolOutcome(result) {
  if (!result) return { label: "…", failed: false };
  const value = result.value;
  if (value?.timed_out) return { label: "Timed out", failed: true };
  if (value?.signal) return { label: `Signal ${value.signal}`, failed: true };
  if (value?.error) return { label: "Failed", failed: true };
  if (typeof value?.exit_code === "number" && value.exit_code !== 0) {
    return { label: `Exit ${value.exit_code}`, failed: true };
  }
  return { label: "", failed: false };
}

function compactOutput(ui, item, content, text, label = "Output") {
  const lines = text.trimEnd().split("\n");
  if (label !== "Arguments" && lines.length <= 3 && text.length <= 240) {
    if (label === "Reasoning") {
      const wrapper = element("div");
      wrapper.append(element("div", "activity-label", label), content);
      return wrapper;
    }
    return content;
  }
  const details = element("details", "activity-details");
  const openKey = `expanded${label}`;
  details.open = Boolean(item[openKey]);
  const summary = element("summary");
  summary.title = `Expand or collapse ${label.toLowerCase()}`;
  summary.append(element("span", "activity-label", label === "Arguments" ? "Tool arguments"
    : label === "Command" ? lines[0] : `${label} · ${lines.length} ${lines.length === 1 ? "line" : "lines"}`));
  const preview = element("pre", "activity-preview", lines.slice(0, 3).join("\n"));
  content.classList.add("activity-full");
  details.append(summary, content);
  const block = element("div", "activity-block");
  block.append(details);
  if (label !== "Command" && label !== "Arguments") block.append(preview);
  summary.addEventListener("click", event => {
    event.preventDefault();
    rememberPosition(ui);
    details.open = !details.open;
    item[openKey] = details.open;
    restorePosition(ui);
  });
  details.addEventListener("toggle", () => {
    if (!details.isConnected) return;
    if (Boolean(item[openKey]) === details.open) return;
    item[openKey] = details.open;
    restorePosition(ui);
  });
  return block;
}

function toolCommand(tool) {
  const value = tool.value;
  if (!value || typeof value !== "object") return tool.payload;
  if (typeof value.command === "string") return value.command;
  if (typeof value.path === "string") {
    if (typeof value.content === "string") return `${value.path}\n${value.content}`;
    if (typeof value.old === "string" && typeof value.new === "string") {
      return `${value.path}\n- ${value.old}\n+ ${value.new}`;
    }
    return value.path + (value.pattern ? `\n${value.pattern}` : "");
  }
  if (typeof value.to === "string" && typeof value.text === "string") return `${value.to}\n${value.text}`;
  if (typeof value.pattern === "string") return value.pattern + (value.glob ? `\n${value.glob}` : "");
  return JSON.stringify(value, null, 2);
}

function toolOutput(tool) {
  const output = element("div", "tool-output");
  const value = tool.value;
  if (value?.ok === true && Object.keys(value).length === 1) return output;
  if (value && typeof value === "object" && !Array.isArray(value) && ("stdout" in value || "stderr" in value)) {
    for (const channel of ["stdout", "stderr"]) {
      if (value[channel]?.text) output.append(element("pre", `tool-text ${channel}`, value[channel].text));
      if (value[channel]?.truncated) output.append(element("div", "tool-note", `${channel} truncated`));
    }
    if (value.error) output.append(element("pre", "tool-text stderr", String(value.error)));
    if (!output.childNodes.length) output.append(element("div", "tool-note", "No output"));
  } else if (value && typeof value.error === "string") {
    output.append(element("pre", "tool-text stderr", value.error));
  } else if (value?.queued === true && typeof value.to === "string") {
    output.append(element("pre", "tool-text", value.accepted_by_agent === false
      ? "Queued; recipient has not accepted the message yet." : "Queued for recipient."));
  } else if (value && typeof value.text === "string") {
    output.append(element("pre", "tool-text", value.text));
    if (value.truncated) output.append(element("div", "tool-note", "Output truncated"));
  } else {
    output.append(element("pre", "tool-text", value !== undefined ? JSON.stringify(value, null, 2) : tool.payload));
  }
  return output;
}

function renderToolEntry(ui, item) {
  const tool = toolEntry(item.text);
  const counterpartKind = item.kind === "tool_call" ? "tool_result" : "tool_call";
  const related = tool.callId && ui.toolPairs.get(tool.heading)?.[counterpartKind];
  const counterpart = related && !related.partial ? related : null;
  const call = item.kind === "tool_call" ? item : counterpart;
  const result = item.kind === "tool_result" ? item : counterpart;
  const owner = call && ui.mounted.has(call) ? call : result;
  const expandedTool = Boolean(call?.expandedTool || result?.expandedTool);
  if (!owner) return;
  const command = toolEntry((call || owner).text);
  const outcome = toolOutcome(result ? toolEntry(result.text) : null);
  const text = call ? toolCommand(command) : "";
  const newline = text.indexOf("\n");
  const firstLine = newline < 0 ? text : text.slice(0, newline);
  const rest = newline < 0 ? "" : text.slice(newline + 1);
  const output = result ? toolOutput(toolEntry(result.text)) : null;
  const outputText = output ? [...output.children].map(child => child.textContent).join("\n") : "";
  const expandable = Boolean(call || rest || firstLine.length > 120 || outputText.length > 240 || outputText.trimEnd().split("\n").length > 3);
  let previous = result?.node.previousElementSibling;
  while (previous?.hidden) previous = previous.previousElementSibling;
  const joined = Boolean(call && result && previous === call.node);
  if (call) call.node.classList.toggle("tool-joined-call", joined);
  if (result) result.node.classList.toggle("tool-joined-result", joined);
  for (const entry of [call, result].filter(Boolean)) {
    entry.node.title = tool.heading;
    entry.node.dataset.toolFailed = String(outcome.failed);
  }
  if ([call, result].some(entry => entry && selectedWithin(entry.node))) {
    owner.deferred = true;
    return;
  }
  owner.body.replaceChildren();
  if (result && result !== owner && !result.partial) result.body.replaceChildren();
  const header = element(expandable ? "summary" : "div", "tool-header");
  header.append(element("span", "tool-name", command.name === "exec_command" ? "$" : command.name));
  if (firstLine) header.append(element("span", "tool-command", firstLine));
  header.append(element("span", "tool-status", outcome.label));
  let details;
  if (expandable) {
    details = element("details", "activity-details tool-operation");
    details.open = expandedTool;
    header.title = "Expand or collapse tool call and result";
    details.append(header);
    if (rest) details.append(element("pre", "tool-text activity-full", rest));
    owner.body.append(details);
  } else {
    owner.body.append(header);
  }
  let preview;
  if (result && outputText) {
    // Intervening peer or assistant text keeps its place between call and result.
    if (call && !joined) result.body.append(element("div", "tool-note", `${command.name} result`));
    result.body.append(output);
    if (expandable) {
      preview = element("pre", "activity-preview", outputText.split("\n").slice(0, 3).join("\n"));
      result.body.append(preview);
    }
  }
  function showResult(open) {
    if (!preview) return;
    output.hidden = !open;
    preview.hidden = open;
  }
  showResult(expandedTool);
  if (details) {
    header.addEventListener("click", event => {
      event.preventDefault();
      rememberPosition(ui);
      details.open = !details.open;
      for (const entry of [call, result].filter(Boolean)) entry.expandedTool = details.open;
      showResult(owner.expandedTool);
      restorePosition(ui);
    });
    details.addEventListener("toggle", () => {
      if (!details.isConnected || Boolean(owner.expandedTool) === details.open) return;
      for (const entry of [call, result].filter(Boolean)) entry.expandedTool = details.open;
      showResult(details.open);
      restorePosition(ui);
    });
  }
}

function renderSkillEntry(ui, item) {
  const { body, text } = item;
  let event;
  try { event = JSON.parse(text); } catch { body.textContent = text; return; }
  body.replaceChildren();
  if (event.event === "error") {
    body.append(element("div", "skill-warning", `Skill selection failed: ${event.error}`));
    return;
  }
  const skill = event.skill;
  if (event.event !== "catalog" && (event.event !== "loaded" || typeof skill?.text !== "string")) {
    body.textContent = text;
    return;
  }
  const details = element("details", "skill-event");
  details.open = Boolean(item.expandedSkill);
  details.append(element("summary", null, skill ? `Loaded ${skill.name || skill.id}` : "Skills refreshed"));
  if (skill) {
    details.append(element("div", "skill-source", skill.source));
    if (skill.version) details.append(element("div", "skill-source", skill.version));
  }
  details.append(element("pre", "skill-text", skill ? skill.text : event.text));
  body.append(details);
  for (const warning of (skill?.warnings || event.warnings || [])) body.append(element("div", "skill-warning", warning));
  details.querySelector("summary").addEventListener("click", e => {
    e.preventDefault();
    rememberPosition(ui);
    item.expandedSkill = details.open = !details.open;
    restorePosition(ui);
  });
  details.addEventListener("toggle", () => {
    if (!details.isConnected || Boolean(item.expandedSkill) === details.open) return;
    item.expandedSkill = details.open;
    restorePosition(ui);
  });
}

function communicationFor(ui, item) {
  return communicationEntries.get(`${ui.id}:${item.id}`);
}

async function locateCommunication(ui, message, otherId) {
  try {
    const target = sessions.get(String(otherId));
    if (!target) throw new Error("This agent is no longer open.");
    const entryId = String(otherId === message.recipient_id ? message.id : message.sender_entry);
    if (selectedWithin(target.trace)) getSelection().removeAllRanges();
    target.follow = false;
    if (!await loadHistory(target, "around", entryId)) return;
    await new Promise(resolve => requestAnimationFrame(resolve));
    const item = target.entries.get(entryId);
    if (!item?.node.isConnected) throw new Error("Message is unavailable in this session.");
    selected = target.pane;
    target.follow = false; target.autoClearance = false;
    target.pane.scrollIntoView({ block: "nearest", inline: "nearest" });
    scrollTrace(target, target.trace.scrollTop + item.node.getBoundingClientRect().top
      - target.trace.getBoundingClientRect().top);
    item.node.tabIndex = -1; item.node.focus({ preventScroll: true });
    rememberPosition(target);
  } catch (error) { showError(ui.error, error); }
}

function renderCommunication(ui, item, message) {
  const otherId = message.sender_id === ui.id ? message.recipient_id : message.sender_id;
  const ended = sessionLifecycle.get(otherId)?.open === false;
  const signature = JSON.stringify([message, ended]);
  if (item.communicationSignature === signature) return;
  const refocus = item.body.contains(document.activeElement);
  const header = element("div", "peer-header");
  for (const [id, name, arrow] of [
    [message.sender_id, message.sender_name, message.urgency === "emergency" ? " ->> " : " -> "],
    [message.recipient_id, message.recipient_name, ""],
  ]) {
    const link = id === otherId && !ended;
    const node = element(link ? "button" : "span", link ? "peer-link" : "", name);
    if (link) {
      node.type = "button";
      node.setAttribute("aria-label", `Find this message in ${name}`);
      node.addEventListener("click", () => locateCommunication(ui, message, id));
    } else if (id === otherId && ended) node.title = "Session ended";
    header.append(node, document.createTextNode(arrow));
  }
  item.body.replaceChildren(header, element("div", "peer-text", message.text));
  const note = message.state === "cancelled" ? (message.persisted ? "Cancelled when the recipient ended." : "Cancelled in memory; storage unavailable.")
    : message.blocked ? "Recipient paused; waiting for operator input." : "";
  if (note) item.body.append(element("div", "tool-note", note));
  item.node.dataset.communicationId = message.id;
  item.node.dataset.communicationDirection = message.sender_id === ui.id ? "outgoing" : "incoming";
  item.communicationSignature = signature;
  if (refocus) item.body.querySelector("button")?.focus({ preventScroll: true });
}

function refreshPeerEntries(sessionId) {
  for (const peer of communications.values()) {
    if (peer.sender_id === sessionId || peer.recipient_id === sessionId) receiveCommunication(peer);
  }
}

function receiveCommunication(message) {
  rememberCommunication(message);
  for (const id of new Set([message.sender_id, message.recipient_id])) {
    const ui = sessions.get(id);
    if (!ui) continue;
    for (const entryId of [message.id, message.sender_entry]) {
      const item = ui.entries.get(String(entryId));
      if (item && communicationFor(ui, item)?.id === message.id) ui.dirty.add(item);
    }
    for (const item of ui.messageResults.get(message.id) || []) ui.dirty.add(item);
    scheduleTranscript(ui);
  }
}

// Keep the loaded history in memory, but mount only the viewport and overscan.
// Spacer heights preserve geometry when old nodes leave the DOM.
function mountTranscript(ui) {
  const history = [...ui.entries.values()].filter(item =>
    !item.suppressed && (!item.superseded || selectedWithin(item.node)));
  let visible = history;
  let before = 0, after = 0;
  if (ui.history && history.length) {
    const heights = history.map(item => item.node.isConnected ? measuredHeight(item) : (item.height || 60));
    let start = 0, end = history.length;
    if (ui.follow) start = Math.max(0, end - 35);
    else {
      const top = Math.max(0, ui.trace.scrollTop - 700);
      const bottom = ui.trace.scrollTop + ui.trace.clientHeight + 700;
      let position = ui.historyBefore.hidden ? 0 : ui.historyBefore.offsetHeight;
      start = history.length - 1; end = history.length;
      for (let i = 0; i < history.length; i++) {
        if (position + heights[i] >= top) { start = i; break; }
        position += heights[i];
      }
      end = start;
      while (end < history.length && position < bottom && end - start < 100) position += heights[end++];
      end = Math.max(start + 1, end);
    }
    if (ui.historyRebase && ui.anchor?.node) {
      const index = history.findIndex(item => item.node === ui.anchor.node);
      if (index >= 0) { start = Math.max(0, index - 8); end = Math.min(history.length, index + 35); }
    }
    ui.historyRebase = false;
    // Selection and a focused peer-link destination stay mounted until released.
    for (let i = 0; i < history.length; i++) if (selectedWithin(history[i].node) || history[i].node.contains(document.activeElement)) {
      start = Math.min(start, i); end = Math.max(end, i + 1);
    }
    if (ui.jumpEntry) {
      const index = history.findIndex(item => item.id === ui.jumpEntry);
      if (index >= 0) { start = Math.max(0, index - 10); end = Math.min(history.length, index + 15); }
      ui.jumpEntry = null;
    }
    before = heights.slice(0, start).reduce((a,b) => a+b, 0);
    after = heights.slice(end).reduce((a,b) => a+b, 0);
    visible = history.slice(start, end);
  }
  const wanted = new Set(visible);
  for (const item of ui.mounted) {
    if (wanted.has(item)) continue;
    item.height = measuredHeight(item);
    item.node.remove(); item.body.replaceChildren(); item.rendered = false;
    item.communicationSignature = undefined;
  }
  ui.spacerBefore.style.height = `${before}px`;
  ui.spacerAfter.style.height = `${after}px`;
  let next = ui.spacerAfter;
  for (let i = visible.length - 1; i >= 0; i--) {
    const item = visible[i];
    if (item.node.parentNode !== ui.content || item.node.nextSibling !== next) {
      ui.content.insertBefore(item.node, next); ui.dirty.add(item);
    }
    next = item.node;
  }
  ui.mounted = wanted;
  updateHistoryControls(ui);
}
function measuredHeight(item) {
  if (item.node.hidden) return 0;
  const style = getComputedStyle(item.node);
  return item.node.getBoundingClientRect().height + parseFloat(style.marginTop || 0) + parseFloat(style.marginBottom || 0);
}
function updateHistoryControls(ui) {
  ui.historyBefore.hidden = !ui.history || ui.history.start === 0;
  ui.historyAfter.hidden = !ui.history || ui.history.end >= ui.history.total;
  ui.historyLatest.hidden = !ui.history || (ui.follow && ui.history.end >= ui.history.total);
  ui.historyBefore.disabled = ui.historyAfter.disabled = Boolean(ui.historyLoading);
}
function scheduleViewport(ui) {
  if (ui.viewportScheduled) return;
  ui.viewportScheduled = true;
  requestAnimationFrame(() => {
    ui.viewportScheduled = false;
    if (!ui.history || sessions.get(ui.id) !== ui) return;
    ui.structureDirty = true; scheduleTranscript(ui);
  });
}
function mergeHistory(ui, view, replace = false) {
  if (!view.history) return;
  rememberPosition(ui);
  const savedAnchor = !ui.follow && ui.anchor ? { id: ui.anchor.node.dataset.entryId, offset: ui.anchor.offset } : null;
  if (replace) {
    const keep = new Set(view.entries.map(entry => String(entry.id)));
    for (const [id, item] of ui.entries) {
      if (!keep.has(id)) { item.node.remove(); ui.entries.delete(id); ui.mounted.delete(item); ui.dirty.delete(item); }
      else item.rendered = false;
    }
    ui.toolPairs.clear(); ui.messageResults.clear();
    ui.anchor = null;
  }
  const old = ui.history;
  ui.history = { ...view.history, total: replace ? view.history.total : Math.max(old?.total || 0, view.history.total) };
  for (const message of view.communications || []) rememberCommunication(message);
  for (const [index, entry] of view.entries.entries()) {
    const item = ui.entries.get(String(entry.id));
    if (item && item.eventSeq > view.history.seq) continue;
    putEntry(ui, { ...entry, history_index: view.history.start + index });
    ui.entries.get(String(entry.id)).eventSeq = entry.history_seq ?? view.history.seq;
  }
  ui.entries = new Map([...ui.entries].sort((a,b) => a[1].index - b[1].index));
  const indices = [...ui.entries.values()].map(item => item.index).filter(Number.isFinite);
  if (indices.length) { ui.history.start = Math.min(...indices); ui.history.end = Math.max(...indices) + 1; }
  if (savedAnchor && ui.entries.has(savedAnchor.id)) ui.anchor = { node: ui.entries.get(savedAnchor.id).node, offset: savedAnchor.offset };
  ui.historyRebase = true; ui.structureDirty = true; scheduleTranscript(ui); updateHistoryControls(ui);
}
async function loadHistory(ui, direction, entryId = null) {
  if (sessions.get(ui.id) !== ui) return false;
  if (["latest", "around", "restore"].includes(direction) && selectedWithin(ui.trace)) {
    ui.deferredNavigation = { direction, entryId }; return false;
  }
  if (ui.historyLoading) {
    if (!["latest", "around", "restore"].includes(direction)) return false;
    ui.historyGeneration++; ui.historyLoading = false;
  }
  const generation = ui.historyGeneration;
  ui.historyLoading = true; ui.historyUpdates = []; updateHistoryControls(ui);
  try {
    const query = entryId ? `?around=${encodeURIComponent(entryId)}` : direction === "before" ? `?before=${ui.history.start}` : direction === "after" ? `?after=${ui.history.end}` : "";
    const view = await request(`/api/sessions/${encodeURIComponent(ui.id)}/history${query}`);
    if (sessions.get(ui.id) !== ui || ui.historyGeneration !== generation) return;
    if (view.archived) throw new Error("This agent has ended.");
    // Rehydrate an already-expanded tall anchor before restoring its pixel offset.
    if (direction === "restore" && entryId && ui.entries.get(entryId)?.partial === false) {
      const anchor = view.entries.find(entry => String(entry.id) === entryId);
      if (anchor?.text_complete === false) {
        const full = await request(`/api/sessions/${encodeURIComponent(ui.id)}/history?entry=${encodeURIComponent(entryId)}`);
        if (sessions.get(ui.id) !== ui || ui.historyGeneration !== generation) return false;
        Object.assign(anchor, full.entries[0], { text_complete: true, history_seq: full.history.seq });
        view.communications = [...(view.communications || []), ...(full.communications || [])];
      }
    }
    if (["latest", "around", "restore"].includes(direction) && selectedWithin(ui.trace)) {
      ui.deferredNavigation = { direction, entryId }; return false;
    }
    if (direction === "latest") ui.follow = true;
    if (view.history) mergeHistory(ui, view, ["latest", "around", "restore"].includes(direction));
    else { for (const entry of view.entries || []) putEntry(ui, entry); scheduleTranscript(ui); }
    for (const message of ui.historyUpdates) if (message.seq > (view.history?.seq || 0)) applyTranscriptEvent(ui, message);
    if (entryId && direction === "around") ui.jumpEntry = entryId;
    if (ui.follow) followLatest(ui);
    return true;
  } catch (error) { showError(ui.error, error); }
  finally { if (ui.historyGeneration === generation) { ui.historyLoading = false; updateHistoryControls(ui); } }
}

function scheduleTranscript(ui) {
  if (ui.renderScheduled) return;
  ui.renderScheduled = true;
  requestAnimationFrame(() => {
    ui.renderScheduled = false;
    if (sessions.get(ui.id) !== ui) return;
    withRenderSelection(() => {
      rememberPosition(ui);
      if (ui.structureDirty) {
        mountTranscript(ui);
        ui.structureDirty = false;
        ui.groupDirty = true;
      }
      for (const item of ui.dirty) {
        if (!ui.mounted.has(item)) { item.rendered = false; continue; }
        if (selectedWithin(item.node)) { item.deferred = true; continue; }
        const hidden = item.node.hidden;
        renderEntry(ui, item);
        if (hidden !== item.node.hidden) ui.groupDirty = true;
        item.rendered = true;
      }
      for (const item of ui.mounted) item.height = measuredHeight(item);
      ui.dirty.clear();
      if (ui.groupDirty) groupUserEntries(ui);
      activatePromptSpace(ui);
      restorePosition(ui);
      if (ui.history && ui.history.start > 0 && ui.content.offsetHeight < ui.trace.clientHeight) loadHistory(ui, "before");
    });
  });
}

function renderEntry(ui, item) {
  if (!ui.mounted.has(item)) { item.rendered = false; return; }
  const { node, body, kind, text } = item;
  node.dataset.pending = ui.pending.has(item.id) ? "Queued" : "";
  if (kind === "reasoning") return;
  if (item.partial) {
    const load = element("button", "history-more", "Load full message"); load.type = "button";
    body.replaceChildren(element("pre", "tool-text", text), load);
    load.onclick = async () => {
      const generation = ui.historyGeneration;
      load.disabled = true;
      try {
        const view = await request(`/api/sessions/${encodeURIComponent(ui.id)}/history?entry=${encodeURIComponent(item.id)}`);
        if (sessions.get(ui.id) !== ui || generation !== ui.historyGeneration || ui.entries.get(item.id) !== item) return;
        const entry = view.entries[0];
        for (const message of view.communications || []) rememberCommunication(message);
        // A streaming update that raced this read requires another fresh read.
        if ((item.eventSeq || 0) > view.history.seq) { load.disabled = false; return; }
        putEntry(ui, { ...entry, history_index: item.index });
        item.eventSeq = view.history.seq;
      } catch (error) { showError(ui.error, error); load.disabled = false; }
    };
    return;
  }
  const communication = communicationFor(ui, item);
  if (communication) { renderCommunication(ui, item, communication); return; }
  if (kind === "tool_result") {
    const result = toolEntry(text);
    if (result.name === "send_message" && communications.has(result.value?.message_id) && !result.value?.error) {
      node.hidden = true;
      return;
    }
  }
  if (kind === "skill") {
    renderSkillEntry(ui, item);
  } else if (kind === "assistant") {
    body.textContent = text;
  } else if (kind === "tool_call" || kind === "tool_result") {
    renderToolEntry(ui, item);
  } else if (kind === "tool_arguments") {
    body.replaceChildren(compactOutput(ui, item, element("pre", "tool-text", text), text, "Arguments"));
  } else {
    body.textContent = text;
  }
}

function groupUserEntries(ui) {
  ui.groupDirty = false;
  let previous;
  for (const item of ui.mounted) {
    item.node.classList.remove("user-joined-before", "user-joined-after");
  }
  for (const item of ui.mounted) {
    if (item.node.hidden) continue;
    if (item.kind === "user" && previous?.kind === "user"
      && ((ui.pending.has(item.id) && ui.pending.has(previous.id))
        || (item.inputBatch != null && item.inputBatch === previous.inputBatch))) {
      previous.node.classList.add("user-joined-after");
      item.node.classList.add("user-joined-before");
    }
    previous = item;
  }
}

function putEntry(ui, entry, delta = false) {
  const id = String(entry.id);
  let item = ui.entries.get(id);
  if (!item) {
    const kind = entry.kind || "notice";
    const node = element("article", "entry");
    node.dataset.kind = kind;
    node.dataset.entryId = id;
    const body = element("div", "entry-body");
    if (kind === "context" && ui.entries.size === 0) {
      const details = element("details", "session-context");
      details.append(element("summary", null, "Context"), body);
      const raw = element("a", "raw-transcript", "Full session record");
      raw.href = `/api/sessions/${encodeURIComponent(ui.id)}`;
      raw.target = "_blank";
      raw.rel = "noopener";
      details.append(raw);
      node.append(details);
    } else {
      if (kind === "context") node.append(element("div", "entry-kind", "Context"));
      node.append(body);
    }
    // Transport receipts remain in the full record; they are not model output.
    node.hidden = kind === "reasoning" || (kind === "notice" && /^(Input .* applied to the conversation\.|Queued message (?:[^ ]+ )?from .* applied to this model request\.|Reasoning shown here is the provider's exposed summary\.)/.test(entry.text || ""));
    if (entry.replaces) {
      const replaced = ui.entries.get(String(entry.replaces));
      if (replaced) {
        replaced.superseded = true;
        if (!selectedWithin(replaced.node)) replaced.node.hidden = true;
      }
    }
    item = { id, node, body, kind, text: "", suppressed: node.hidden, index: entry.history_index ?? ui.entries.size };
    if (entry.superseded) { item.superseded = true; item.node.hidden = true; }
    ui.entries.set(id, item);
    ui.structureDirty = true;
    if (kind === "user") ui.pending.add(id);
  }
  if (!delta) {
    item.partial = entry.text_complete === false;
    if (entry.superseded !== undefined) { item.superseded = entry.superseded; if (!selectedWithin(item.node)) item.node.hidden = item.suppressed || item.superseded; }
  }
  if (delta && item.partial) { item.textBytes = (item.textBytes || item.text.length) + (entry.text || "").length; return; }
  const batchChanged = !delta && item.inputBatch !== entry.input_batch;
  if (batchChanged && item.kind === "user") ui.groupDirty = true;
  if (!delta) item.inputBatch = entry.input_batch;
  const text = delta ? item.text + (entry.text || "") : (entry.text || "");
  if (text !== item.text || !item.rendered || batchChanged) {
    item.text = text;
    if (item.kind === "tool_call" || item.kind === "tool_result") {
      const tool = toolEntry(item.text);
      if (tool.callId) {
        const pair = ui.toolPairs.get(tool.heading) || {};
        pair[item.kind] = item;
        ui.toolPairs.set(tool.heading, pair);
        for (const entry of Object.values(pair)) ui.dirty.add(entry);
      }
      const messageId = tool.name === "send_message" && tool.value?.message_id;
      if (item.kind === "tool_result" && messageId) {
        if (!ui.messageResults.has(messageId)) ui.messageResults.set(messageId, new Set());
        ui.messageResults.get(messageId).add(item);
      }
    }
    ui.dirty.add(item);
    scheduleTranscript(ui);
  }
}

function setPending(ui, pending) {
  rememberPosition(ui);
  ui.pending = new Set(pending);
  for (const item of ui.entries.values()) {
    if (item.kind === "user") item.node.dataset.pending = ui.pending.has(item.id) ? "Queued" : "";
  }
  groupUserEntries(ui);
  restorePosition(ui);
}

function acceptSessionResponse(view, startedAt) {
  const lifecycle = sessionLifecycle.get(String(view.id));
  if (view.archived || view.status === "ending" || (lifecycle && !lifecycle.open && lifecycle.revision > startedAt)) {
    throw new Error("This agent has ended. Choose it from Inactive sessions to reopen.");
  }
  sessionLifecycle.set(String(view.id), { open: true, revision: ++catalogRevision });
  const ui = addSession(view);
  refreshPeerEntries(String(view.id));
  return ui;
}

function snapshot(views, terminalViews = []) {
  communications.clear();
  communicationEntries.clear();
  for (const view of views) for (const message of view.communications || []) rememberCommunication(message);
  const revision = ++catalogRevision;
  const active = new Set(views.map(view => String(view.id)));
  for (const [id, lifecycle] of sessionLifecycle) {
    if (lifecycle.open && !active.has(id)) sessionLifecycle.set(id, { open: false, revision });
  }
  for (const id of active) sessionLifecycle.set(id, { open: true, revision });
  views = [...views].sort((a, b) => String(a.id).localeCompare(String(b.id)));
  terminalViews = [...terminalViews].sort((a, b) => String(a.id).localeCompare(String(b.id)));
  reconcilingCatalog = true;
  reconcileTerminals(terminalViews);
  const ids = new Set(views.map(view => String(view.id)));
  for (const [id, ui] of sessions) {
    if (!ids.has(id)) {
      removeSession(ui);
    }
  }
  for (const view of views) {
    const ui = sessions.get(String(view.id));
    if (!ui) { addSession(view); continue; }
    // Reconcile after a reconnect even if the interrupt's status event was lost.
    if (!ui.interrupting) ui.interruptRequested = false;
    ui.view = view;
    if (view.history) {
      if (selectedWithin(ui.trace)) ui.follow = false;
      const anchor = !ui.follow && ui.anchor?.node?.dataset.entryId;
      ui.historyGeneration++; ui.historyLoading = false;
      for (const item of ui.entries.values()) item.eventSeq = 0;
      if (ui.follow) mergeHistory(ui, view, true);
      else {
        ui.history.total = view.history.total;
        if (anchor) loadHistory(ui, "restore", anchor);
        else mergeHistory(ui, view);
      }
      updateProfile(ui, view.profile, view.context_window);
      ui.setName(view.name || String(view.id));
      setPending(ui, view.pending || []); setStatus(ui, view.status);
      continue;
    }
    updateProfile(ui, view.profile, view.context_window);
    const follow = ui.follow;
    rememberPosition(ui);
    ui.setName(view.name || String(view.id));
    ui.directory.textContent = view.cwd;
    ui.directory.title = view.cwd;
    const present = new Set();
    for (const entry of view.entries || []) {
      present.add(String(entry.id));
      const item = ui.entries.get(String(entry.id));
      if (item && (communicationFor(ui, item) || item.kind === "tool_result")) item.rendered = false;
      putEntry(ui, entry);
    }
    for (const [id, entry] of ui.entries) {
      if (!present.has(id)) {
        entry.node.remove();
        ui.entries.delete(id);
        ui.mounted.delete(entry);
        ui.dirty.delete(entry);
        ui.structureDirty = true;
      }
    }
    for (const [heading, pair] of ui.toolPairs) {
      for (const kind of Object.keys(pair)) if (!ui.entries.has(pair[kind].id)) delete pair[kind];
      if (!Object.keys(pair).length) ui.toolPairs.delete(heading);
    }
    for (const [id, results] of ui.messageResults) {
      for (const item of results) if (!ui.entries.has(item.id)) results.delete(item);
      if (!results.size) ui.messageResults.delete(id);
    }
    scheduleTranscript(ui);
    setPending(ui, view.pending || []);
    setStatus(ui, view.status);
    if (follow) followLatest(ui);
    else restorePosition(ui);
  }
  reconcilingCatalog = false;
  catalogReady = true;
  layout();
  refreshChoices();
}

function applyTranscriptEvent(ui, message) {
  const id = String(message.type === "entry" ? message.entry.id : message.entry_id);
  const current = ui.entries.get(id);
  if (current && current.eventSeq >= message.seq) return;
  if (message.type === "entry") {
    const index = message.entry_index;
        if (ui.history && Number.isFinite(index)) {
          ui.history.total = Math.max(ui.history.total, index + 1);
          if (index > ui.history.end) { updateHistoryControls(ui); return; }
          ui.history.end = Math.max(ui.history.end, index + 1);
        }
        putEntry(ui, { ...message.entry, history_index: index });
        ui.entries.get(String(message.entry.id)).eventSeq = message.seq;
      }
  if (message.type === "delta") {
        const item = ui.entries.get(String(message.entry_id));
        if (!ui.history || item) {
          putEntry(ui, { id: message.entry_id, text: message.text }, true);
          ui.entries.get(String(message.entry_id)).eventSeq = message.seq;
        }
      }
 }

function startEvents() {
  events?.close();
  events = new EventSource("/api/events?history=recent");
  events.onopen = () => { document.body.classList.remove("disconnected"); };
  events.onerror = () => {
    document.body.classList.add("disconnected");
    request("/api/config").catch(error => {
      if (error.status === 401) { events.close(); requireLogin(); }
    });
  };
  events.onmessage = event => {
    try {
      const message = JSON.parse(event.data);
      if (message.type === "snapshot") {
        lastSequence = message.seq;
        snapshot(message.sessions, message.terminals || []);
        return;
      }
      if (message.seq <= lastSequence) return;
      lastSequence = message.seq;
      const revision = ++catalogRevision;
      if (message.type === "session") sessionLifecycle.set(String(message.session.id), { open: true, revision });
      if (message.type === "closed") sessionLifecycle.set(String(message.session_id), { open: false, revision });
      if (["session", "closed", "status", "name"].includes(message.type)) refreshChoices();
      if (message.type === "session") {
        addSession(message.session);
        refreshPeerEntries(String(message.session.id));
        return;
      }
      if (message.type === "terminal") {
        terminalCatalog.set(String(message.terminal.id), message.terminal);
        reconcileTerminals([...terminalCatalog.values()]);
        return;
      }
      if (message.type === "communication") { receiveCommunication(message.message); return; }
      const ui = sessions.get(String(message.session_id));
      if (message.type === "closed") {
        reportUnsavedEnd(message);
        if (ui) removeSession(ui);
        refreshPeerEntries(String(message.session_id));
        return;
      }
      if (!ui) return;
      if (message.type === "name") { ui.setName(message.name); return; }
      if (message.type === "profile") { updateProfile(ui, message.profile, message.context_window); return; }
      if (message.type === "context_usage") {
        ui.view.context_usage = message.context_usage;
        ui.view.context_window = message.context_window;
        updateContextUsage(ui);
        return;
      }
      if (message.type === "input") {
        const item = ui.entries.get(String(message.id));
        if (item) item.inputBatch = message.input_batch;
        ui.pending.delete(String(message.id));
        setPending(ui, ui.pending);
      }
      if (["entry", "delta"].includes(message.type)) {
        if (ui.historyLoading) ui.historyUpdates.push(message);
        applyTranscriptEvent(ui, message);
      }
      if (message.type === "status") setStatus(ui, message.status, message.working_since_ms ?? null);
    } catch (error) { showError(workspaceError, error); }
  };
}

function reconcileTerminals(infos) {
  terminalCatalog.clear();
  for (const info of infos) terminalCatalog.set(String(info.id), info);
  for (const [id, ui] of terminals) {
    const info = terminalCatalog.get(id);
    if (!info || info.archived || !info.running) ui.dispose();
    else ui.update(info);
  }
  for (const info of infos) {
    if (info.running && !info.archived && !terminals.has(String(info.id))) attachTerminal(info);
  }
}

function attachTerminal(info) {
  if (!info.running || info.archived) return;
  const id = String(info.id);
  if (terminals.has(id)) return terminals.get(id);
  const ui = makePane(info.name || "#?", info.cwd, "terminal", id);
  ui.id = id;
  enablePaneRename(ui, "terminal");
  const close = ui.close;
  const host = element("div", "terminal-host");
  ui.pane.append(host, ui.error);
  const term = new Terminal({
    cursorBlink: true,
    disableStdin: true,
    fontFamily: '"MesloLGS NF", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
    fontSize: 11,
    scrollback: 5000,
    theme: { background: "#1e1e1e", foreground: "#d7d7d7", cursor: "#d7d7d7" },
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(host);
  let socket;
  let closed = false;
  let retry;
  let clientId;
  let controller = false;
  let replaying = true;
  let wantsControl = false;
  let rendering = Promise.resolve();
  function send(message) {
    if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  }
  function resize() {
    if (!controller || replaying) return;
    const size = fit.proposeDimensions();
    if (size) send({ type: "resize", cols: Math.max(1, Math.min(500, size.cols)), rows: Math.max(1, Math.min(500, size.rows)) });
  }
  function connect() {
    if (closed) return;
    clearTimeout(retry);
    if (socket) { socket.onclose = null; socket.close(); }
    ui.status.textContent = "";
    ui.pane.classList.add("disconnected");
    term.options.disableStdin = true;
    const scheme = location.protocol === "https:" ? "wss:" : "ws:";
    socket = new WebSocket(`${scheme}//${location.host}/api/terminals/${encodeURIComponent(id)}/ws`);
    socket.binaryType = "arraybuffer";
    const connectedSocket = socket;
    controller = false;
    replaying = true;
    socket.onopen = () => {
      ui.pane.classList.remove("disconnected");
      ui.error.hidden = true;
    };
    socket.onmessage = event => {
      rendering = rendering.then(async () => {
        if (socket !== connectedSocket || closed) return;
        if (event.data instanceof ArrayBuffer) {
          await new Promise(resolve => term.write(new Uint8Array(event.data), resolve));
          return;
        }
        const message = JSON.parse(event.data);
        if (message.type === "name") ui.setName(message.name);
        if (message.type === "hello") clientId = message.id;
        if (message.type === "replay_start") {
          replaying = true;
          term.options.disableStdin = true;
          term.reset();
        }
        if (message.type === "resize") term.resize(message.cols, message.rows);
        if (message.type === "replay_end") {
          replaying = false;
          if (wantsControl) { wantsControl = false; send({ type: "control" }); }
        }
        if (message.type === "control") {
          controller = message.id === clientId;
          term.options.disableStdin = replaying || !controller;
          resize();
        }
      }).catch(error => showError(ui.error, error));
    };
    socket.onclose = async () => {
      if (closed) return;
      ui.pane.classList.add("disconnected");
      term.options.disableStdin = true;
      try {
        const list = await request("/api/terminals");
        const current = list.find(item => String(item.id) === id);
        if (!current) { ui.dispose(); return; }
        if (!current.running || current.archived) { ui.dispose(); return; }
      } catch (error) {
        if (error.status === 401) { requireLogin(); return; }
      }
      retry = setTimeout(connect, 2000);
    };
  }
  term.onData(data => { if (controller && !replaying) send({ type: "input", data }); });
  function takeControl() {
    if (!info.running) return;
    if (replaying) wantsControl = true;
    else if (!controller) send({ type: "control" });
  }
  host.addEventListener("focusin", takeControl);
  host.addEventListener("pointerdown", takeControl);
  function resumeControl() {
    if (host.contains(document.activeElement)) takeControl();
  }
  window.addEventListener("focus", resumeControl);
  const screen = host.querySelector(".xterm-screen");
  const viewport = host.querySelector(".xterm-viewport");
  let scrollbarIdle;
  viewport.addEventListener("scroll", () => {
    host.classList.add("scrolling");
    clearTimeout(scrollbarIdle);
    scrollbarIdle = setTimeout(() => host.classList.remove("scrolling"), 100);
  }, { passive: true });
  const observer = new ResizeObserver(entries => {
    host.style.setProperty("--terminal-track-edge", `${viewport.getBoundingClientRect().left + viewport.clientWidth - host.getBoundingClientRect().left}px`);
    if (entries.some(entry => entry.target === host) && host.clientWidth > 0 && host.clientHeight > 0) resize();
  });
  observer.observe(host);
  observer.observe(screen);
  ui.dispose = () => {
    if (closed) return;
    closed = true;
    clearTimeout(retry);
    socket?.close();
    observer.disconnect();
    clearTimeout(scrollbarIdle);
    window.removeEventListener("focus", resumeControl);
    term.dispose();
    if (selected === ui.pane) selected = null;
    ui.pane.remove();
    terminals.delete(id);
    layout();
  };
  ui.update = latest => {
    if (!latest.running || latest.archived) { ui.dispose(); return; }
    info = latest;
    ui.setName(info.name);
    ui.directory.textContent = info.cwd || "";
    ui.directory.title = (info.cwd || "") + (info.running && info.cwd_observed === false ? " (last known directory)" : "");
    ui.status.textContent = "";
    if (info.persistence_error) showError(ui.error, `History could not be saved: ${info.persistence_error}`);
    if (!info.running) term.options.disableStdin = true;
  };
  close.addEventListener("click", async () => {
    close.disabled = true;
    try {
      await request(`/api/terminals/${encodeURIComponent(id)}`, "DELETE");
      ui.dispose();
    } catch (error) { showError(ui.error, error); close.disabled = false; }
  });
  ui.reconnect = connect;
  terminals.set(id, ui);
  ui.update(info);
  layout();
  connect();
  return ui;
}

const agentDefaults = {
  name: "", cwd: "",
};
let agentNames = [];
let choicesVersion = 0;
let choicesTimer;

function focusSession(ui) {
  selected = ui.pane;
  ui.pane.scrollIntoView({ block: "nearest", inline: "nearest" });
  if (ui.input) ui.input.focus({ preventScroll: true });
  else ui.title.focus({ preventScroll: true });
}

function discardSetup(ui, replacement) {
  if (loginOwner === ui) closeLogin();
  if (replacement) {
    const key = replacement.pane.dataset.paneKey;
    paneOrder = paneOrder.filter(item => item !== key).map(item => item === `draft:${ui.id}` ? key : item);
  }
  drafts.delete(ui.id);
  if (selected === ui.pane) selected = null;
  ui.pane.remove();
  layout();
  if (replacement) focusSession(replacement);
}

function addSetup(saved) {
  const values = saved || {
    id: Array.from(crypto.getRandomValues(new Uint8Array(16)), byte => byte.toString(16).padStart(2, "0")).join(""),
    name: "", cwd: "",
  };
  if (drafts.has(values.id)) return drafts.get(values.id);
  const ui = makePane("New agent", "", "draft", values.id);
  ui.pane.classList.add("setup-pane");
  ui.id = values.id;
  ui.directory.hidden = true;
  ui.status.textContent = "";
  ui.close.title = "Discard draft";
  ui.close.setAttribute("aria-label", "Discard agent draft");
  const body = element("div", "session-setup-grid");
  ui.form = element("form", "setup-form");
  for (const [key, label] of [["name", "Name"], ["cwd", "Working directory"]]) {
    const wrapper = element("label", null, label);
    const input = element("input");
    input.name = key;
    input.value = values[key];
    input.placeholder = agentDefaults[key];
    if (key === "name") {
      input.maxLength = 128;
      const suggestions = element("datalist");
      suggestions.id = `agent-names-${ui.id}`;
      for (const name of agentNames) {
        const option = element("option");
        option.value = name;
        suggestions.append(option);
      }
      input.setAttribute("list", suggestions.id);
      wrapper.append(suggestions);
    }
    input.addEventListener("input", rememberPaneOrder);
    wrapper.append(input);
    ui.form.append(wrapper);
  }
  const submit = element("button", "setup-submit", "Create agent");
  submit.type = "submit";
  const loginCommand = element("button", "setup-login", "/login");
  loginCommand.type = "button";
  loginCommand.title = "Choose API or subscription";
  loginCommand.addEventListener("click", () => openLogin(ui));
  ui.form.append(submit, loginCommand);
  const choices = element("div", "session-choices");
  ui.active = element("div", "active-agents");
  ui.inactive = element("div", "inactive-agents");
  choices.append(element("h2", null, "Active sessions"), ui.active, element("h2", null, "Inactive sessions"), ui.inactive);
  body.append(ui.form, choices);
  ui.setupBody = body;
  ui.pane.append(body, ui.error);
  ui.close.addEventListener("click", () => discardSetup(ui));
  ui.form.addEventListener("submit", async event => {
    event.preventDefault();
    if (ui.busy) return;
    ui.busy = true;
    submit.disabled = true;
    ui.close.disabled = true;
    ui.error.hidden = true;
    try {
      const startedAt = catalogRevision;
      const fields = Object.fromEntries(new FormData(ui.form));
      fields.cwd ||= agentDefaults.cwd;
      // Empty names are allocated atomically by the server, including concurrent creation.
      fields.name = fields.name.trim();
      const view = await request("/api/sessions", "POST", fields);
      discardSetup(ui, acceptSessionResponse(view, startedAt));
    } catch (error) { showError(ui.error, error); }
    finally { ui.busy = false; submit.disabled = false; ui.close.disabled = false; }
  });
  drafts.set(ui.id, ui);
  layout();
  refreshChoices();
  return ui;
}

function sessionCard(record, action) {
  const card = element("button", "session-card");
  card.type = "button";
  card.dataset.sessionId = String(record.id);
  const heading = element("span", "session-card-heading");
  heading.append(element("span", "session-card-name", record.name || "Agent"), element("span", "session-card-status", record.status || "idle"));
  const cwd = element("span", "session-card-cwd", record.cwd || "");
  cwd.title = record.cwd || "";
  card.append(heading, cwd);
  const task = record.entries?.find(entry => entry.kind === "user")?.text;
  if (task) {
    const excerpt = element("span", "session-card-task", task.replace(/\s+/g, " ").slice(0, 160));
    excerpt.title = task;
    card.append(excerpt);
  }
  card.title = action;
  return card;
}

function focusedChoice(container) {
  const focused = document.activeElement;
  return container.contains(focused) ? focused.closest(".session-card")?.dataset.sessionId : null;
}

function restoreChoiceFocus(container, id) {
  if (!id) return;
  const card = [...container.querySelectorAll(".session-card")].find(card => card.dataset.sessionId === id);
  if (card && !card.disabled) card.focus({ preventScroll: true });
}

function renderAgentChoices(ui, active, inactive) {
  for (const [container, records, archived] of [[ui.active, active, false], [ui.inactive, inactive, true]]) {
    const focused = focusedChoice(container);
    container.replaceChildren();
    if (!records.length) container.append(element("p", "session-empty", "None"));
    for (const record of records) {
      const card = sessionCard(record, archived ? "Reopen paused with history" : "Focus existing agent");
      card.disabled = record.status === "ending" || (archived && !record.archived);
      card.addEventListener("click", async () => {
        if (ui.busy) return;
        ui.busy = true;
        card.disabled = true;
        ui.close.disabled = true;
        ui.error.hidden = true;
        try {
          const startedAt = catalogRevision;
          const view = await request(`/api/sessions/${encodeURIComponent(record.id)}${archived ? "/reopen?history=recent" : "/history"}`, archived ? "POST" : "GET");
          const attached = sessions.has(String(view.id));
          const agent = acceptSessionResponse(view, startedAt);
          if (archived || !attached) discardSetup(ui, agent);
          else { discardSetup(ui); focusSession(agent); }
        } catch (error) { showError(ui.error, error); refreshChoices(); }
        finally { ui.busy = false; card.disabled = false; ui.close.disabled = false; }
      });
      container.append(card);
    }
    restoreChoiceFocus(container, focused);
  }
}

function refreshChoices() {
  const version = ++choicesVersion;
  clearTimeout(choicesTimer);
  if (!drafts.size) return;
  choicesTimer = setTimeout(async () => {
    try {
      const [active, inactive, config] = await Promise.all([
        request("/api/sessions?history=recent"), request("/api/sessions/archived?history=recent"), request("/api/config"),
      ]);
      if (version !== choicesVersion) return;
      agentDefaults.name = config.next_agent_name;
      for (const ui of drafts.values()) {
        ui.form.elements.name.placeholder = agentDefaults.name;
        renderAgentChoices(ui, active, inactive);
      }
    } catch (error) {
      if (version !== choicesVersion) return;
      for (const ui of drafts.values()) showError(ui.error, error);
    }
  }, 40);
}

document.querySelector("#add-agent").addEventListener("click", () => {
  const ui = addSetup();
  ui.form.elements.name.focus();
});
document.querySelector("#add-terminal").addEventListener("click", async event => {
  const button = event.currentTarget;
  button.disabled = true;
  workspaceError.hidden = true;
  try {
    const info = await request("/api/terminals", "POST", { cols: 80, rows: 24, cwd: agentDefaults.cwd });
    attachTerminal(info);
  } catch (error) { showError(workspaceError, error); }
  finally { button.disabled = false; }
});


const diagnosticsButton = document.querySelector("#open-diagnostics");
const diagnosticsDialog = document.querySelector("#diagnostics-dialog");
const diagnosticsScope = document.querySelector("#diagnostics-scope");
const diagnosticsContent = document.querySelector("#diagnostics-content");
const diagnosticsError = document.querySelector("#diagnostics-error");
let diagnosticsAgent = "";
let diagnosticsSelection = "";
let diagnosticsData;
let diagnosticsTimer;
let diagnosticsGeneration = 0;
let diagnosticsOptions = "";
let diagnosticsRendered = "";

for (const kind of ["agent", "terminal"]) {
  const plus = element("span", null, "+");
  plus.setAttribute("aria-hidden", "true");
  document.querySelector(`#add-${kind}`).replaceChildren(plus, paneIcon(kind));
}

function rememberDiagnosticsAgent(target) {
  if (target.closest(".workspace-actions, #diagnostics-dialog")) return;
  const pane = target.closest(".pane");
  diagnosticsAgent = [...sessions.values()].find(ui => ui.pane === pane)?.id || "";
}

function diagnosticNumber(value) {
  return Number.isFinite(value) ? value.toLocaleString("en-US") : "—";
}

function diagnosticCost(value) {
  return Number.isFinite(value) ? `$${value.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 6 })}` : "—";
}

function diagnosticRow(list, label, value, reason) {
  const term = element("dt", null, label);
  const detail = element("dd", null, value);
  if (reason) detail.append(element("span", "diagnostics-reason", reason));
  list.append(term, detail);
}

function renderDiagnostics(resetMissing = true) {
  if (!diagnosticsData) return;
  const records = diagnosticsData.sessions || [];
  const options = JSON.stringify(records.map(record => [record.id, record.name, record.archived]));
  if (options !== diagnosticsOptions) {
    diagnosticsOptions = options;
    const all = element("option", null, "All agents");
    all.value = "";
    diagnosticsScope.replaceChildren(all);
    for (const record of records) {
      const duplicate = records.filter(other => other.name === record.name).length > 1;
      const name = (record.name || "Agent") + (duplicate ? ` (${String(record.id).slice(0, 6)})` : "") + (record.archived ? " · ended" : "");
      const option = element("option", null, name);
      option.value = String(record.id);
      diagnosticsScope.append(option);
    }
  }
  const selectedRecord = records.find(record => String(record.id) === diagnosticsSelection);
  if (!selectedRecord && resetMissing) diagnosticsSelection = "";
  diagnosticsScope.value = selectedRecord ? diagnosticsSelection : "";
  const summary = selectedRecord?.summary || diagnosticsData.aggregate;
  if (!summary) return;
  const rendered = JSON.stringify([diagnosticsSelection, summary, selectedRecord?.latest_context, selectedRecord?.latest_response, diagnosticsData.pricing]);
  if (rendered === diagnosticsRendered) return;
  diagnosticsRendered = rendered;
  const reasonsExpanded = diagnosticsContent.querySelector("details")?.open || false;
  diagnosticsContent.replaceChildren();
  diagnosticsContent.append(element("h2", null, "Recorded totals"));
  const totals = element("dl", "diagnostics-values");
  const values = summary.totals || {};
  const missing = summary.recorded_requests ? "Incomplete usage" : "No recorded responses";
  diagnosticRow(totals, "Requests", diagnosticNumber(summary.recorded_requests));
  for (const [label, key] of [["Input tokens", "input_tokens"], ["Output tokens", "output_tokens"], ["Cache read", "cached_read_tokens"], ["Cache write", "cache_write_tokens"]]) {
    const value = values[key];
    diagnosticRow(totals, label, diagnosticNumber(value), Number.isFinite(value) ? null : missing);
  }
  diagnosticRow(totals, "Cache reuse", Number.isFinite(values.reuse_pct) ? `${values.reuse_pct.toLocaleString("en-US", { maximumFractionDigits: 1 })}%` : "—", Number.isFinite(values.reuse_pct) ? null : "No known ratio");
  const recordedCost = summary.priced_requests > 0 ? summary.priced_cost_usd : summary.estimated_cost_usd;
  const costCoverage = summary.priced_requests < summary.recorded_requests
    ? `${summary.priced_requests} of ${summary.recorded_requests} recorded requests priced`
    : null;
  diagnosticRow(totals, "Token cost", diagnosticCost(recordedCost), costCoverage);
  diagnosticRow(totals, "Rates updated", diagnosticsData.pricing?.as_of || "—");
  diagnosticsContent.append(totals);
  const manage = element("a", null, "Manage usage in ChatGPT");
  manage.href = "https://chatgpt.com/settings/usage"; manage.target = "_blank"; manage.rel = "noopener";
  diagnosticsContent.append(manage);
  const coverage = [];
  if (summary.in_flight_requests) coverage.push(`${diagnosticNumber(summary.in_flight_requests)} in flight`);
  if (summary.unrecorded_requests) coverage.push(`${diagnosticNumber(summary.unrecorded_requests)} unrecorded`);
  if (!summary.history_complete) coverage.push("Usage coverage is incomplete");
  if (coverage.length) diagnosticsContent.append(element("p", "diagnostics-note", coverage.join(" · ")));
  const reasons = [...new Set((summary.unknown_reasons || []).filter(reason => typeof reason === "string"))];
  if (reasons.length) {
    const disclosure = element("details", "diagnostics-note");
    disclosure.open = reasonsExpanded;
    disclosure.append(element("summary", null, "Coverage details"));
    for (const reason of reasons) disclosure.append(element("p", "diagnostics-note", reason));
    diagnosticsContent.append(disclosure);
  }
  if (selectedRecord) {
    const latest = selectedRecord.latest_response;
    if (latest) {
      diagnosticsContent.append(element("h2", null, "Latest response"));
      const details = element("dl", "diagnostics-values");
      diagnosticRow(details, "Model", latest.model || "—", latest.model ? null : "Not reported");
      diagnosticRow(details, "Service tier", latest.service_tier || "—", latest.service_tier ? null : "Not reported");
      diagnosticsContent.append(details);
    }
    if (!latest) diagnosticsContent.append(element("p", "diagnostics-note", "No recorded response yet."));
  }

}

async function refreshDiagnostics() {
  const generation = diagnosticsGeneration;
  try {
    const data = await request("/api/diagnostics");
    if (!diagnosticsDialog.open || generation !== diagnosticsGeneration) return;
    diagnosticsData = data;
    diagnosticsError.hidden = true;
    renderDiagnostics();
  } catch (error) {
    if (!diagnosticsDialog.open || generation !== diagnosticsGeneration) return;
    if (!diagnosticsData) diagnosticsContent.replaceChildren();
    showError(diagnosticsError, `Could not refresh diagnostics: ${error.message || error}`);
  } finally {
    if (diagnosticsDialog.open && generation === diagnosticsGeneration) diagnosticsTimer = setTimeout(refreshDiagnostics, 2000);
  }
}

diagnosticsButton.addEventListener("click", () => {
  diagnosticsSelection = diagnosticsAgent;
  diagnosticsGeneration++;
  clearTimeout(diagnosticsTimer);
  diagnosticsError.hidden = true;
  diagnosticsDialog.showModal();
  if (diagnosticsData) renderDiagnostics(false);
  else diagnosticsContent.replaceChildren(element("p", "diagnostics-note", "Loading…"));
  diagnosticsScope.focus();
  refreshDiagnostics();
});
diagnosticsScope.addEventListener("change", () => {
  diagnosticsSelection = diagnosticsScope.value;
  renderDiagnostics();
});
document.querySelector("#close-diagnostics").addEventListener("click", () => diagnosticsDialog.close());
diagnosticsDialog.addEventListener("close", () => {
  diagnosticsGeneration++;
  clearTimeout(diagnosticsTimer);
  diagnosticsButton.focus({ preventScroll: true });
});
diagnosticsDialog.addEventListener("keydown", event => {
  event.stopPropagation();
  if (event.key === "Escape" && !event.isComposing) {
    event.preventDefault();
    diagnosticsDialog.close();
  }
});
diagnosticsDialog.addEventListener("cancel", event => { event.preventDefault(); diagnosticsDialog.close(); });
let diagnosticsOutside = false;
function outsideDiagnostics(event) {
  const rect = diagnosticsDialog.getBoundingClientRect();
  return event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom;
}
diagnosticsDialog.addEventListener("pointerdown", event => { diagnosticsOutside = outsideDiagnostics(event); });
diagnosticsDialog.addEventListener("click", event => {
  if (event.target === diagnosticsDialog && diagnosticsOutside && outsideDiagnostics(event)) diagnosticsDialog.close();
});

function requireLogin() {
  login.hidden = false;
  document.body.classList.add("disconnected");
  document.querySelector("#add-agent").disabled = true;
  document.querySelector("#add-terminal").disabled = true;
  diagnosticsButton.disabled = true;
  closeLogin();
  if (diagnosticsDialog.open) diagnosticsDialog.close();
}

async function connect(token) {
  if (token) await request("/api/login", "POST", undefined, { Authorization: `Bearer ${token}` });
  const config = await request("/api/config");
  login.hidden = true;
  workspaceError.hidden = true;
  agentDefaults.cwd = launchCwd || config.cwd;
  agentDefaults.name = config.next_agent_name;
  agentNames = config.agent_names;
  for (const ui of drafts.values()) ui.form.elements.cwd.placeholder = agentDefaults.cwd;
  loadPaneOrder(agentDefaults.cwd);
  document.querySelector("#add-agent").disabled = false;
  document.querySelector("#add-terminal").disabled = false;
  diagnosticsButton.disabled = false;

  startEvents();
  for (const ui of terminals.values()) ui.reconnect();
}

login.addEventListener("submit", async event => {
  event.preventDefault();
  const field = document.querySelector("#token");
  const token = field.value.trim();
  field.value = "";
  try { await connect(token); }
  catch (error) { requireLogin(); showError(workspaceError, error); }
});

async function initialize() {
  const parameters = new URLSearchParams(location.hash.slice(1));
  const token = parameters.get("token");
  try {
    launchCwd = parameters.has("cwd") ? parameters.get("cwd")
      : token ? "" : sessionStorage.getItem("boid:launch-cwd") || "";
    sessionStorage.setItem("boid:launch-cwd", launchCwd);
  } catch { launchCwd = parameters.get("cwd") || ""; }
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);
  try { await connect(token); }
  catch (error) {
    requireLogin();
    if (error.status !== 401) showError(workspaceError, error);
  }
}
document.addEventListener("pointerdown", event => {
  rememberDiagnosticsAgent(event.target);
  selected = event.target.closest(".pane");
});
document.addEventListener("focusin", event => {
  rememberDiagnosticsAgent(event.target);
  selected = event.target.closest(".pane");
});
document.addEventListener("keydown", async event => {
  if (diagnosticsDialog.open || document.querySelector("#plan-welcome").open) return;
  if (loginOwner && selected === loginOwner.pane) {
    if (event.key === "Escape" && !event.isComposing) { event.preventDefault(); closeLogin(); }
    return;
  }
  const skillPicker = [...sessions.values()].find(ui => ui.pane === selected && !ui.skillPicker.hidden);
  if (skillPicker && event.key === "Escape" && !event.isComposing && !event.defaultPrevented) {
    event.preventDefault(); closeSkillPicker(skillPicker); return;
  }
  const picker = [...sessions.values()].find(ui => ui.pane === selected && !ui.modelPicker.hidden);
  if (picker && event.key === "Escape" && !event.isComposing && !event.defaultPrevented) {
    event.preventDefault();
    closeModelPicker(picker);
    return;
  }
  if (!event.defaultPrevented && !event.isComposing && !picker
      && !event.ctrlKey && !event.metaKey && !event.altKey && event.key.length === 1
      && !event.target.closest("input, textarea, [contenteditable], .model-picker, .skill-picker")) {
    const selection = window.getSelection();
    const reader = selection && !selection.isCollapsed && [...sessions.values()].find(ui =>
      ui.trace.contains(selection.anchorNode) && ui.trace.contains(selection.focusNode));
    if (reader) {
      event.preventDefault();
      reader.input.focus({ preventScroll: true });
      reader.input.setRangeText(event.key, reader.input.selectionStart, reader.input.selectionEnd, "end");
      selection.removeAllRanges();
      reader.input.dispatchEvent(new Event("input", { bubbles: true }));
      return;
    }
  }
  if (event.key !== "Escape" || event.isComposing || event.defaultPrevented) return;
  const ui = [...sessions.values()].find(ui => ui.pane === selected);
  if (!ui || (ui.pane.dataset.status !== "working" && !ui.pending.size)) return;
  event.preventDefault();
  if (event.repeat || ui.interrupting || ui.interruptRequested) return;
  ui.interrupting = true;
  ui.interruptRequested = true;
  updateActivity(ui);
  ui.error.hidden = true;
  try {
    await ui.submission?.catch(() => {});
    await request(`/api/sessions/${encodeURIComponent(ui.id)}/interrupt`, "POST");
  }
  catch (error) {
    ui.interruptRequested = false;
    updateActivity(ui);
    showError(ui.error, error);
  }
  finally { ui.interrupting = false; }
});
const loginPanel = document.querySelector("#provider-login");
const providerError = document.querySelector("#provider-login-error");
const providerStatus = document.querySelector("#provider-login-status");
const subscriptionAccount = document.querySelector("#subscription-account");
let loginOwner = null, loginTimer, loginGeneration = 0, loginStatus, loginSignature = "";
let loginMethod = "";
function closeLogin() {
  clearTimeout(loginTimer); loginGeneration++;
  const owner = loginOwner; loginOwner = null;
  loginPanel.hidden = true; document.body.append(loginPanel);
  document.querySelector("#provider-api-key").value = "";
  if (!owner) return;
  const form = owner.composer || owner.form;
  form.hidden = false;
  if (owner.setupBody) owner.setupBody.hidden = false;
  if (owner.composer) updateActivity(owner);
  (owner.input || owner.form.querySelector("input"))?.focus({ preventScroll: true });
}
function openLogin(ui) {
  closeLogin(); loginOwner = ui; loginMethod = ""; loginSignature = ""; loginStatus = null;
  subscriptionAccount.replaceChildren();
  if (ui.composer) {
    ui.slashSuggestion.hidden = true; ui.modelPicker.hidden = true; ui.skillPicker.hidden = true;
    ui.composer.hidden = true; clearPromptSpaceWhenHidden(ui); updateActivity(ui);
  } else { ui.form.hidden = true; ui.setupBody.hidden = true; }
  ui.pane.append(loginPanel); loginPanel.hidden = false;
  providerError.hidden = true; providerStatus.textContent = "";
  document.querySelector("#api-key-login").hidden = true;
  document.querySelector("#subscription-login").hidden = true;
  for (const button of loginPanel.querySelectorAll("[data-login-method]")) button.setAttribute("aria-pressed", "false");
  loginPanel.querySelector("[data-login-method]").focus({ preventScroll: true });
  refreshLogin();
}
function chooseLoginMethod(method) {
  loginMethod = method; providerError.hidden = true;
  for (const button of loginPanel.querySelectorAll("[data-login-method]")) button.setAttribute("aria-pressed", String(button.dataset.loginMethod === method));
  document.querySelector("#api-key-login").hidden = method !== "api_key";
  document.querySelector("#subscription-login").hidden = method !== "chatgpt";
  if (method === "api_key") {
    document.querySelector("#api-key-label").hidden = Boolean(loginStatus?.api_key_available);
    document.querySelector("#api-key-ready").hidden = !loginStatus?.api_key_available;
    if (!loginStatus?.api_key_available) document.querySelector("#provider-api-key").focus();
  } else refreshSubscription();
}
async function refreshLogin() {
  clearTimeout(loginTimer);
  if (!loginOwner) return;
  if (!loginOwner.pane.isConnected) { closeLogin(); return; }
  const generation = loginGeneration;
  try {
    const status = await request("/api/accounts");
    if (!loginOwner || generation !== loginGeneration) return;
    loginStatus = status;
    const signature = JSON.stringify(status);
    if (signature !== loginSignature) {
      loginSignature = signature;
      if (loginMethod) chooseLoginMethod(loginMethod);
    }
  } catch (error) { if (generation === loginGeneration) showError(providerError, error); }
  finally { if (loginOwner && generation === loginGeneration) loginTimer = setTimeout(refreshLogin, 2000); }
}
async function refreshSubscription() {
  if (!loginStatus) return;
  const previous = subscriptionAccount.value;
  subscriptionAccount.replaceChildren();
  for (const account of loginStatus.accounts) {
    const option = element("option", null, `${account.label}${account.connected ? "" : " · signed out"}`);
    option.value = account.id; subscriptionAccount.append(option);
  }
  const preferred = previous || loginOwner?.view?.profile?.chatgpt_account || loginStatus.selected_account;
  if ([...subscriptionAccount.options].some(option => option.value === preferred)) subscriptionAccount.value = preferred;
  document.querySelector("#subscription-selection").hidden = !loginStatus.accounts.length;
  subscriptionActions();
}
function subscriptionActions() {
  const account = loginStatus?.accounts.find(account => account.id === subscriptionAccount.value);
  const submit = document.querySelector("#subscription-selection button"); submit.disabled = true;
  const actions = document.querySelector("#subscription-actions"); actions.replaceChildren();
  if (!account) return;
  const action = (label, run) => {
    const button = element("button", null, label); button.type = "button";
    button.addEventListener("click", async () => { button.disabled = true; try { await run(); }
      catch (error) { showError(providerError, error); } finally { button.disabled = false; } });
    actions.append(button);
  };
  if (!account.connected || !account.plan_enabled) {
    action(account.connected ? "Enable subscription" : "Reconnect", () => signInChatGPT(account.id, !account.plan_enabled));
  }
  if (account.connected) action("Sign out", async () => {
    const result = await request(`/api/accounts/${encodeURIComponent(account.id)}/logout`, "POST");
    loginSignature = ""; await refreshLogin();
    if (!result.revocation_confirmed) showError(providerError, result.message);
  });
  if (account.paused) {
    actions.append(element("p", null, "Requests paused after a usage limit. Review ChatGPT settings before resuming."));
    action("Resume requests", async () => { await request(`/api/accounts/${encodeURIComponent(account.id)}/resume`, "POST"); loginSignature = ""; await refreshLogin(); });
  }
  submit.disabled = !account.connected || !account.plan_enabled || account.paused;
}

async function signInChatGPT(account = null, consent = false) {
  providerError.hidden = true;
  const popup = window.open("about:blank", "boid-chatgpt-sign-in");
  try {
    const result = await request("/api/accounts/sign-in", "POST", { origin: location.origin, account, consent });
    if (popup) { popup.opener = null; popup.location.href = result.start_url; }
    else { const link = element("a", null, "Continue with ChatGPT"); link.href = result.start_url; link.target = "_blank"; link.rel = "noopener";
      providerError.replaceChildren(link); providerError.hidden = false; }
  } catch (error) { popup?.close(); showError(providerError, error); }
}
loginPanel.addEventListener("keydown", event => {
  event.stopPropagation();
  if (event.key === "Escape" && !event.isComposing) { event.preventDefault(); closeLogin(); }
});
document.querySelector("#close-provider-login").addEventListener("click", closeLogin);
function restoreLoginFocus() {
  if (loginOwner) loginPanel.querySelector('[data-login-method="chatgpt"]').focus({ preventScroll: true });
}
document.querySelector("#plan-welcome").addEventListener("close", restoreLoginFocus);
document.querySelector("#plan-welcome form").addEventListener("submit", event => {
  event.preventDefault(); document.querySelector("#plan-welcome").close(); restoreLoginFocus();
});
for (const button of loginPanel.querySelectorAll("[data-login-method]")) button.addEventListener("click", () => chooseLoginMethod(button.dataset.loginMethod));
document.querySelector("#chatgpt-sign-in").addEventListener("click", () => signInChatGPT());
subscriptionAccount.addEventListener("change", subscriptionActions);
document.querySelector("#api-key-login").addEventListener("submit", async event => {
  event.preventDefault(); providerError.hidden = true;
  const owner = loginOwner, generation = loginGeneration;
  const button = event.submitter; button.disabled = true;
  const field = document.querySelector("#provider-api-key"), key = field.value.trim(); field.value = "";
  try {
    if (!loginStatus?.api_key_available) {
      if (!key) throw new Error("Enter an OpenAI API key.");
      await request("/api/accounts/api-key", "POST", { key });
    }
    const result = await request("/api/accounts/select", "POST", { session_id: owner.composer ? owner.id : undefined, billing_route: "api_key" });
    if (owner.composer) updateProfile(owner, result.profile, result.context_window);
    if (loginOwner !== owner || generation !== loginGeneration) return;
    loginSignature = ""; await refreshLogin();
    providerStatus.textContent = "API key connected.";
  } catch (error) { showError(providerError, error); }
  finally { button.disabled = false; }
});
document.querySelector("#subscription-selection").addEventListener("submit", async event => {
  event.preventDefault(); providerError.hidden = true;
  const owner = loginOwner, generation = loginGeneration;
  const button = event.submitter; button.disabled = true;
  try {
    const result = await request("/api/accounts/select", "POST", { session_id: owner.composer ? owner.id : undefined, billing_route: "chatgpt", account: subscriptionAccount.value });
    if (owner.composer) updateProfile(owner, result.profile, result.context_window);
    if (loginOwner !== owner || generation !== loginGeneration) return;
    loginSignature = ""; await refreshLogin();
    providerStatus.textContent = "Subscription connected.";
    if (result.welcome) document.querySelector("#plan-welcome").showModal();
  } catch (error) { showError(providerError, error); }
  finally { if (loginOwner === owner && generation === loginGeneration) subscriptionActions(); }
});

setInterval(() => {
  for (const ui of sessions.values()) if (ui.view?.status === "working") updateActivity(ui);
}, 1000);
initialize();
window.addEventListener("resize", layout, { passive: true });
layout();
