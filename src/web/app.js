"use strict";

const workspace = document.querySelector("#workspace");
const empty = document.querySelector("#empty");
const workspaceError = document.querySelector("#workspace-error");
const login = document.querySelector("#login");
const sessions = new Map();
// Requests outlive pane DOM when switching screens.
const questionSubmissions = new Map();
// Opt-in, local-only input timing: boidInputTiming.enable(); then .samples().
// Measures handler/transport/DOM milestones, not physical display scan-out.
const inputTiming = (() => {
  const limit = 64, records = [], entries = new Map();
  let enabled = new URLSearchParams(location.search).get("input-timing") === "1";
  let serial = 0, evictedRecords = 0, evictedEntries = 0;
  const key = (session, message) => JSON.stringify([session, message]);
  const current = record => enabled && record && !record.ended && records.includes(record);
  const elapsed = (record, at) => at == null || at < record.startedAt ? null : at - record.startedAt;
  function correlate(record) {
    if (record.ended || (record.domAt != null && record.ackAt != null)) return;
    const entry = entries.get(key(record.sessionId, record.messageId));
    if (!entry) return;
    record.entryAt = entry.receivedAt; record.entrySource = entry.source;
    record.domAt = entry.domAt; record.inViewport = entry.inViewport;
    record.documentVisible = entry.documentVisible;
  }
  function copy(record) {
    const entryMs = elapsed(record, record.entryAt), domMs = elapsed(record, record.domAt);
    return {
      sample: record.serial, sessionId: record.sessionId, messageId: record.messageId,
      outcome: record.failure || (record.ended ? "observation_disabled" : record.ackAt == null ? "awaiting_http"
        : entryMs == null ? "accepted_waiting_server_entry" : domMs == null ? "accepted_waiting_dom" : "rendered"),
      httpStatus: record.httpStatus,
      submitToHeadersMs: elapsed(record, record.headersAt),
      submitToAcknowledgmentMs: elapsed(record, record.ackAt),
      submitToServerEntryMs: entryMs, submitToDOMRenderMs: domMs,
      submitToComposerClearMs: elapsed(record, record.clearAt),
      entrySource: record.entrySource, inViewport: record.inViewport,
      documentVisible: record.documentVisible, serverTimingMs: { ...record.serverTiming },
    };
  }
  const stats = () => ({ enabled, capacity: limit, retained: records.length, evictedRecords, evictedEntries });
  window.boidInputTiming = Object.freeze({
    enable() { enabled = true; return stats(); },
    disable() { enabled = false; for (const record of records) record.ended = record.domAt == null || record.ackAt == null; entries.clear(); return stats(); },
    clear() { records.length = 0; entries.clear(); evictedRecords = evictedEntries = 0; },
    samples() { return records.map(copy); }, stats,
  });
  return {
    startedAt: () => enabled ? performance.now() : null,
    begin(sessionId, startedAt) {
      if (!enabled || startedAt == null) return null;
      const record = { serial: ++serial, sessionId, startedAt, messageId: null,
        httpStatus: null, headersAt: null, ackAt: null, clearAt: null,
        entryAt: null, entrySource: null, domAt: null, inViewport: null,
        documentVisible: null, serverTiming: {}, failure: null, ended: false };
      records.push(record);
      if (records.length > limit) { records.shift(); evictedRecords++; }
      return record;
    },
    response(record, response, headersAt) {
      if (!current(record)) return;
      record.httpStatus = response.status; record.headersAt = headersAt;
      for (const metric of (response.headers.get("Server-Timing") || "").split(",")) {
        const match = metric.trim().match(/^(boid_(?:lock|mutate|admit|handler))\s*;\s*dur=([0-9]+(?:\.[0-9]+)?)(?:\s*;|\s*$)/);
        if (match && Number.isFinite(Number(match[2]))) record.serverTiming[match[1]] = Number(match[2]);
      }
    },
    acknowledge(record, result) {
      if (!current(record)) return;
      record.ackAt = performance.now();
      if (typeof result?.message_id !== "string") { record.failure = "missing_message_id"; return; }
      record.messageId = result.message_id; correlate(record);
    },
    cleared(record) { if (current(record)) record.clearAt = performance.now(); },
    failed(record, error) {
      if (current(record)) record.failure = Number.isFinite(error?.status) ? "http_error" : "network_error";
    },
    entry(sessionId, messageId, source) {
      if (!enabled) return;
      sessionId = String(sessionId); messageId = String(messageId);
      const id = key(sessionId, messageId);
      if (!entries.has(id)) {
        entries.set(id, { sessionId, messageId, receivedAt: performance.now(), source,
          domAt: null, inViewport: null, documentVisible: null });
        if (entries.size > limit) { entries.delete(entries.keys().next().value); evictedEntries++; }
      }
      for (const record of records) if (record.sessionId === sessionId && record.messageId === messageId) correlate(record);
    },
    rendered(ui) {
      if (!enabled) return;
      const at = performance.now();
      let bounds;
      for (const entry of entries.values()) {
        if (entry.sessionId !== ui.id || entry.domAt != null) continue;
        const item = ui.entries.get(entry.messageId);
        if (!item?.rendered || !ui.mounted.has(item) || !item.node.isConnected || item.node.hidden) continue;
        bounds ||= ui.trace.getBoundingClientRect();
        const box = item.node.getBoundingClientRect();
        entry.domAt = at; entry.inViewport = box.bottom > bounds.top && box.top < bounds.bottom;
        entry.documentVisible = document.visibilityState === "visible";
        for (const record of records) if (record.sessionId === ui.id && record.messageId === entry.messageId) correlate(record);
      }
    },
  };
})();
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
let paneLayout = null;
let paneResize = null;
let paneDrag = null;
const paneResizeLayer = element("div", "pane-resize-layer");
workspace.append(paneResizeLayer);
let paneResizeHandles = new Map();
let paneResizeHandleId = 0;

// The server owns pane membership; the URL/sessionStorage owns this window's selection.
let screenCatalog = { count: 1, panes: {} };
let activeScreen = 1;
let screenLoaded;
let agentScreenState = {};
const screensNav = document.querySelector("#screens");
const addScreenButton = document.querySelector("#add-screen");
const activeScreenKey = `boid:screen:${location.origin}`;
function paneScreen(key) { return screenCatalog.panes[key] || 1; }
function renderScreens() {
  const wanted = new Set();
  for (let number = 1; number <= screenCatalog.count; number++) {
    const id = String(number); wanted.add(id);
    let link = screensNav.querySelector(`[data-screen="${number}"]`);
    if (!link) {
      link = element("a", "", id); link.dataset.screen = id;
      link.setAttribute("aria-label", `Screen ${number}`);
      link.title = `Screen ${number} · drag a pane tab here to move it`;
      const url = new URL(location.href); url.searchParams.set("screen", id); url.hash = "";
      link.href = url.pathname + url.search;
      link.addEventListener("click", event => {
        if (event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        event.preventDefault(); switchScreen(number);
      });
      link.addEventListener("dragover", event => {
        if (!event.dataTransfer.types.includes("application/x-boid-pane")) return;
        event.preventDefault(); event.dataTransfer.dropEffect = "move"; link.classList.add("drop-target");
      });
      link.addEventListener("dragleave", () => link.classList.remove("drop-target"));
      link.addEventListener("drop", async event => {
        event.preventDefault(); link.classList.remove("drop-target");
        const pane = event.dataTransfer.getData("application/x-boid-pane");
        if (!pane) return;
        try { receiveScreens(await request("/api/screens/move", "POST", { pane, screen: number })); }
        catch (error) { showError(workspaceError, error); }
      });
      screensNav.insertBefore(link, addScreenButton);
    }
    if (number === activeScreen) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  for (const link of screensNav.querySelectorAll("a")) if (!wanted.has(link.dataset.screen)) link.remove();
}
function receiveScreens(value, reconcile = true) {
  if (!Number.isInteger(value?.count) || value.count < 1 || !value.panes || Array.isArray(value.panes)) return;
  if ((value.revision || 0) < (screenCatalog.revision || 0)) return;
  const previous = screenCatalog;
  screenCatalog = value;
  renderScreens();
  if (reconcile && catalogReady) {
    const changed = new Set([...Object.keys(previous.panes), ...Object.keys(value.panes)]);
    if ([...changed].some(key => (previous.panes[key] || 1) !== paneScreen(key)
      && [(previous.panes[key] || 1), paneScreen(key)].includes(activeScreen))) {
      rememberPaneOrder(); startEvents();
    }
  }
}
function storeScreenSelection() {
  try { sessionStorage.setItem(activeScreenKey, String(activeScreen)); } catch {}
  const url = new URL(location.href); url.searchParams.set("screen", String(activeScreen));
  history.replaceState(null, "", url.pathname + url.search);
  renderScreens();
}
function captureAgentState(ui) {
  if (ui.restoringScreen) return;
  rememberPosition(ui);
  const expanded = [];
  for (const [id, item] of ui.entries) {
    let flags;
    for (const key of Object.keys(item)) {
      if (key.startsWith("expanded") && item[key] === true) {
        if (!flags) flags = {};
        flags[key] = true;
      }
    }
    if (flags) expanded.push([id, flags]);
  }
  agentScreenState[ui.id] = {
    input: ui.input.value, follow: ui.follow,
    questions: { drafts: Object.fromEntries(ui.questionDrafts), selected: ui.questionId,
      open: !ui.questionPanel.hidden, known: ui.questions.map(q => q.id) },
    anchor: ui.anchor ? { id: ui.anchor.node.dataset.entryId, offset: ui.anchor.offset } : null,
    expanded: Object.fromEntries(expanded),
  };
}
async function restoreAgentState(ui) {
  const state = agentScreenState[ui.id];
  if (!state) return;
  ui.restoringScreen = true;
  const pendingQuestions = new Set(ui.questions.map(q => q.id));
  ui.questionDrafts = new Map(Object.entries(state.questions?.drafts || {})
    .filter(([id, text]) => pendingQuestions.has(id) && typeof text === "string"));
  const newUrgent = ui.questions.find(q => questionWaiting(q) && !state.questions?.known?.includes(q.id));
  const selectedQuestion = newUrgent?.id || (pendingQuestions.has(state.questions?.selected) ? state.questions.selected : ui.questionId);
  // The pane was recreated, so restore the draft even when its initial selection matches.
  ui.questionId = null;
  selectQuestion(ui, selectedQuestion);
  if (ui.questions.length && (newUrgent || state.questions?.open)) ui.questionPanel.hidden = false;
  else if (state.questions && !newUrgent) ui.questionPanel.hidden = true;
  ui.questionsToggle.setAttribute("aria-expanded", String(!ui.questionPanel.hidden));
  ui.input.value = state.input || "";
  ui.input.dispatchEvent(new Event("input"));
  ui.follow = state.follow !== false;
  if (!ui.follow && state.anchor && !ui.entries.has(state.anchor.id)) await loadHistory(ui, "around", state.anchor.id);
  if (sessions.get(ui.id) !== ui) return;
  for (const [id, flags] of Object.entries(state.expanded || {})) {
    const item = ui.entries.get(id);
    if (item) { Object.assign(item, flags); ui.dirty.add(item); }
  }
  if (!ui.follow && state.anchor) ui.jumpEntry = state.anchor.id;
  scheduleTranscript(ui);
  requestAnimationFrame(() => {
    if (sessions.get(ui.id) !== ui) return;
    const item = state.anchor && ui.entries.get(state.anchor.id);
    if (!ui.follow && item) {
      ui.jumpEntry = null;
      ui.anchor = { node: item.node, offset: state.anchor.offset };
      restorePosition(ui);
    }
    ui.restoringScreen = false;
    rememberPaneOrder();
  });
}
function clearScreenPanes() {
  reconcilingCatalog = true;
  for (const ui of [...sessions.values()]) removeSession(ui);
  for (const ui of [...terminals.values()]) ui.dispose();
  for (const ui of drafts.values()) { if (loginOwner === ui) closeLogin(); ui.pane.remove(); }
  drafts.clear(); selected = null;
  for (const handle of paneResizeHandles.values()) handle.remove();
  paneResizeHandles.clear();
  reconcilingCatalog = false;
}
function switchScreen(number) {
  if (number === activeScreen || number < 1 || number > screenCatalog.count) return Promise.resolve();
  finishPaneDrag();
  finishPaneResize(true);
  rememberPaneOrder();
  catalogReady = false;
  clearScreenPanes();
  activeScreen = number;
  paneOrder = []; paneLayout = null; orderKey = "";
  storeScreenSelection();
  loadPaneOrder(agentDefaults.cwd);
  layout();
  return startEvents();
}
addScreenButton.addEventListener("click", async () => {
  addScreenButton.disabled = true;
  try {
    const result = await request("/api/screens", "POST");
    receiveScreens(result); switchScreen(result.count);
  } catch (error) { showError(workspaceError, error); }
  finally { addScreenButton.disabled = false; }
});
window.addEventListener("pagehide", rememberPaneOrder);

function loadPaneOrder(cwd) {
  const key = `boid:pane-order:v1:${location.origin}:${cwd || ""}${activeScreen === 1 ? "" : `:screen:${activeScreen}`}`;
  if (key === orderKey) return;
  orderKey = key;
  try {
    const saved = JSON.parse(sessionStorage.getItem(key) || "null");
    paneLayout = saved?.layout || null;
    agentScreenState = { ...(saved?.agents || {}), ...agentScreenState };
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
  } catch { paneOrder = []; paneLayout = null; }
}

function rememberPaneOrder() {
  if (!catalogReady || reconcilingCatalog) return;
  const panes = new Map([...workspace.querySelectorAll(".pane")].map(pane => [pane.dataset.paneKey, pane]));
  paneOrder = paneOrder.filter(key => panes.has(key));
  for (const key of panes.keys()) if (!paneOrder.includes(key)) paneOrder.push(key);
  for (let i = 0; i < paneOrder.length; i++) {
    const pane = panes.get(paneOrder[i]);
    const current = workspace.querySelectorAll(".pane")[i];
    if (pane !== current) {
      // Moving a DOM subtree can reset descendant scroll offsets. Restore them
      // before captureAgentState records the new arrangement.
      const scroll = [...pane.querySelectorAll(".trace, .xterm-viewport")]
        .map(node => ({ node, top: node.scrollTop, left: node.scrollLeft }));
      workspace.insertBefore(pane, current || null);
      for (const { node, top, left } of scroll) node.scrollTo({ top, left, behavior: "instant" });
    }
  }
  for (const ui of sessions.values()) captureAgentState(ui);
  try {
    sessionStorage.setItem(orderKey, JSON.stringify({
      version: 2, order: paneOrder, agents: agentScreenState,
      layout: paneLayout?.custom ? paneLayout : null,
      drafts: [...drafts.values()].map(ui => ({ id: ui.id, ...Object.fromEntries(new FormData(ui.form)) })),
    }));
  } catch { /* Storage availability must not prevent session access. */ }
}

function clearPaneDropTarget() {
  for (const pane of workspace.querySelectorAll("[data-pane-drop]")) delete pane.dataset.paneDrop;
}
function finishPaneDrag() {
  clearPaneDropTarget();
  paneDrag?.pane.classList.remove("pane-dragging");
  paneDrag = null;
}
function paneDropTarget(event) {
  if (!paneDrag || !paneDrag.pane.isConnected || paneDrag.screen !== activeScreen) return null;
  const pane = event.target.closest(".pane");
  if (!pane || pane === paneDrag.pane || pane.parentElement !== workspace) return null;
  const box = pane.getBoundingClientRect();
  // In a single column, use top/bottom insertion instead of left/right.
  const vertical = paneLayout?.tiles.every(tile => tile.left === 0 && tile.right === 1);
  const after = vertical ? event.clientY >= box.top + box.height / 2 : event.clientX >= box.left + box.width / 2;
  return { pane, after, edge: vertical ? (after ? "bottom" : "top") : (after ? "right" : "left") };
}
function reorderPane(source, target, after) {
  const before = [...workspace.querySelectorAll(".pane")].map(pane => pane.dataset.paneKey);
  if (source === target || !before.includes(source) || !before.includes(target)) return;
  const next = before.filter(key => key !== source);
  next.splice(next.indexOf(target) + (after ? 1 : 0), 0, source);
  if (next.every((key, index) => key === before[index])) return;
  const focused = document.activeElement;
  for (const ui of sessions.values()) rememberPosition(ui);
  // Keep resized rectangles in place. Only their occupants change.
  if (paneLayout?.custom) {
    const slots = before.map(key => paneLayout.tiles.find(tile => tile.key === key));
    if (slots.every(Boolean)) paneLayout.tiles = slots.map((tile, index) => ({ ...tile, key: next[index] }));
  }
  paneOrder = next;
  layout();
  if (focused?.isConnected && focused !== document.activeElement) focused.focus({ preventScroll: true });
}
workspace.addEventListener("dragover", event => {
  const target = paneDropTarget(event);
  clearPaneDropTarget();
  if (!target) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = "move";
  target.pane.dataset.paneDrop = target.edge;
});
workspace.addEventListener("dragleave", event => {
  if (!workspace.contains(event.relatedTarget)) clearPaneDropTarget();
});
workspace.addEventListener("drop", event => {
  const target = paneDropTarget(event);
  if (!target) { finishPaneDrag(); return; }
  event.preventDefault(); event.stopPropagation();
  const source = paneDrag.pane.dataset.paneKey;
  finishPaneDrag();
  reorderPane(source, target.pane.dataset.paneKey, target.after);
});
document.addEventListener("dragend", finishPaneDrag);
document.addEventListener("keydown", event => {
  if (event.key === "Escape" && paneDrag) {
    event.preventDefault(); event.stopImmediatePropagation(); finishPaneDrag();
  }
}, true);
window.addEventListener("blur", finishPaneDrag);

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

async function request(path, method = "GET", body, headers = {}, observeResponse = null) {
  const options = { method, credentials: "same-origin", headers };
  if (body !== undefined) {
    options.headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  const headersAt = observeResponse ? performance.now() : null;
  if (observeResponse) observeResponse(response, headersAt);
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
  if (paneResize) finishPaneResize(true);
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
  if (catalogReady && !reconcilingCatalog) {
    arrangePanes(panes, columns);
    rememberPaneOrder();
  }
  requestAnimationFrame(() => {
    for (const ui of sessions.values()) restorePosition(ui);
    for (const ui of sessions.values()) ui.reflow = false;
  });
}

// Rectangles use workspace fractions so a resized arrangement scales with the window.
// Each shared edge can move independently until it meets another pane's minimum size.
function validPaneLayout(panes) {
  const tiles = paneLayout?.tiles;
  if (!paneLayout?.custom || !Array.isArray(tiles) || tiles.length !== panes.length) return false;
  if (!tiles.every(tile => tile && typeof tile.key === "string")) return false;
  if (new Set(tiles.map(tile => tile.key)).size !== panes.length) return false;
  if (!panes.every(pane => tiles.some(tile => tile.key === pane.dataset.paneKey))) return false;
  return tiles.every((tile, i) => {
    if (![tile.left, tile.right, tile.top, tile.bottom].every(Number.isFinite)
      || tile.left < 0 || tile.right > 1 || tile.top < 0 || tile.bottom > 1
      || tile.right - tile.left < .001 || tile.bottom - tile.top < .001) return false;
    return tiles.slice(i + 1).every(other => Math.min(tile.right, other.right) - Math.max(tile.left, other.left) < .000001
      || Math.min(tile.bottom, other.bottom) - Math.max(tile.top, other.top) < .000001);
  });
}

function arrangePanes(panes, columns) {
  if (!validPaneLayout(panes)) {
    const rows = Math.max(1, Math.ceil(panes.length / columns));
    paneLayout = { custom: false, minimumHeight: rows * 240, tiles: panes.map((pane, index) => ({
      key: pane.dataset.paneKey,
      left: (index % columns) / columns, right: (index % columns + 1) / columns,
      top: Math.floor(index / columns) / rows, bottom: (Math.floor(index / columns) + 1) / rows,
    })) };
  }
  paintPaneLayout();
}

function paneLayoutSize() {
  const tiles = paneLayout.tiles;
  return {
    width: Math.max(workspace.clientWidth, ...tiles.map(tile => 180 / (tile.right - tile.left))),
    height: Math.max(workspace.clientHeight, Number.isFinite(paneLayout.minimumHeight) ? paneLayout.minimumHeight : 0,
      ...tiles.map(tile => (paneLayout.custom ? 120 : 240) / (tile.bottom - tile.top))),
  };
}

function paintPaneLayout() {
  const tiles = paneLayout.tiles;
  const xs = [...new Set([0, 1, ...tiles.flatMap(tile => [tile.left, tile.right])])].sort((a, b) => a - b);
  const ys = [...new Set([0, 1, ...tiles.flatMap(tile => [tile.top, tile.bottom])])].sort((a, b) => a - b);
  const size = paneResize?.size || paneLayoutSize();
  workspace.style.gridTemplateColumns = xs.slice(1).map((x, i) => `${(x - xs[i]) * size.width}px`).join(" ");
  workspace.style.gridTemplateRows = ys.slice(1).map((y, i) => `${(y - ys[i]) * size.height}px`).join(" ");
  for (const pane of workspace.querySelectorAll(".pane")) {
    const tile = tiles.find(tile => tile.key === pane.dataset.paneKey);
    if (!tile) continue;
    pane.style.gridColumn = `${xs.indexOf(tile.left) + 1} / ${xs.indexOf(tile.right) + 1}`;
    pane.style.gridRow = `${ys.indexOf(tile.top) + 1} / ${ys.indexOf(tile.bottom) + 1}`;
  }
  paintPaneResizeHandles(size);
}

function paintPaneResizeHandles(size) {
  paneResizeLayer.style.width = `${size.width}px`;
  paneResizeLayer.style.height = `${size.height}px`;
  const next = new Map(), segments = new Set();
  for (const tile of paneLayout.tiles) for (const edge of ["left", "right", "top", "bottom"]) {
    const group = paneResizeGroup(tile.key, edge);
    if (!group) continue;
    const segment = `${group.horizontal}:${group.at}:${group.low}:${group.high}`;
    if (segments.has(segment)) continue;
    segments.add(segment);
    const key = `${tile.key}:${edge}`;
    // Keep pointer capture attached if snapping joins this segment to another one.
    const active = paneResize && !paneResize.corner && [...group.members].some(([member, side]) =>
      member.key === paneResize.handle.dataset.ownerKey && side === paneResize.handle.dataset.edge);
    const handle = active ? paneResize.handle : paneResizeHandles.get(key) || makePaneResizeHandle();
    if (!active) { handle.dataset.ownerKey = tile.key; handle.dataset.edge = edge; }
    handle.className = `pane-resize pane-resize-${group.horizontal ? "vertical" : "horizontal"}`;
    handle.setAttribute("aria-label", `Resize ${group.horizontal ? "vertical" : "horizontal"} boundary`);
    handle.setAttribute("aria-orientation", group.horizontal ? "vertical" : "horizontal");
    handle.setAttribute("aria-valuenow", Math.round(group.at * 100));
    Object.assign(handle.style, group.horizontal ? {
      left: `${group.at * size.width - 4}px`, top: `${group.low * size.height}px`,
      width: "8px", height: `${(group.high - group.low) * size.height}px`,
    } : {
      left: `${group.low * size.width}px`, top: `${group.at * size.height - 4}px`,
      width: `${(group.high - group.low) * size.width}px`, height: "8px",
    });
    next.set(key, handle);
  }
  for (const tile of paneLayout.tiles) {
    const groups = paneCornerGroups(tile.key);
    if (!groups) continue;
    const key = `corner:${tile.key}`;
    const handle = paneResizeHandles.get(key) || makePaneResizeHandle(true);
    handle.dataset.cornerOwnerKey = tile.key;
    Object.assign(handle.style, { left: `${tile.right * size.width - 5}px`,
      top: `${tile.bottom * size.height - 5}px`, width: "10px", height: "10px" });
    next.set(key, handle);
  }
  const retained = new Set(next.values());
  for (const handle of paneResizeHandles.values()) if (!retained.has(handle)) handle.remove();
  paneResizeHandles = next;
}

function paneResizeGroup(key, edge) {
  if (!catalogReady || !paneLayout) return null;
  const tiles = paneLayout.tiles;
  const tile = tiles.find(tile => tile.key === key);
  if (!tile) return null;
  const horizontal = edge === "left" || edge === "right";
  const [start, end, crossStart, crossEnd] = horizontal ? ["left", "right", "top", "bottom"] : ["top", "bottom", "left", "right"];
  const at = tile[edge];
  if (at === 0 || at === 1) return null;
  let low = tile[crossStart], high = tile[crossEnd];
  const members = new Map();
  // A staggered junction can join several edges. Move the whole connected segment
  // to keep every pane rectangular, without moving an unrelated row or column.
  let changed = true;
  while (changed) {
    changed = false;
    for (const candidate of tiles) {
      const side = candidate[start] === at ? start : candidate[end] === at ? end : null;
      if (!side || members.has(candidate) || candidate[crossEnd] <= low + .000001 || candidate[crossStart] >= high - .000001) continue;
      members.set(candidate, side);
      low = Math.min(low, candidate[crossStart]); high = Math.max(high, candidate[crossEnd]);
      changed = true;
    }
  }
  const size = paneLayoutSize();
  const pixels = horizontal ? size.width : size.height;
  const minimum = (horizontal ? 180 : 120) / pixels;
  let min = 0, max = 1;
  for (const [candidate, side] of members) {
    if (side === end) min = Math.max(min, candidate[start] + minimum);
    else max = Math.min(max, candidate[end] - minimum);
    // An incomplete last row can leave empty space. Stop at the next occupied
    // rectangle even when it does not yet share the dragged edge.
    for (const other of tiles) {
      if (members.has(other) || other[crossEnd] <= candidate[crossStart] + .000001
        || other[crossStart] >= candidate[crossEnd] - .000001) continue;
      if (side === start && other[end] <= at) min = Math.max(min, other[end]);
      if (side === end && other[start] >= at) max = Math.min(max, other[start]);
    }
  }
  return { members, horizontal, at, pixels, min, max, size, low, high };
}

function paneCornerGroups(key) {
  if (!catalogReady || !paneLayout) return null;
  const tiles = paneLayout.tiles, nw = tiles.find(tile => tile.key === key);
  if (!nw || nw.right === 1 || nw.bottom === 1) return null;
  const ne = tiles.find(tile => tile.left === nw.right && tile.bottom === nw.bottom);
  const sw = tiles.find(tile => tile.right === nw.right && tile.top === nw.bottom);
  const se = tiles.find(tile => tile.left === nw.right && tile.top === nw.bottom);
  if (!ne || !sw || !se) return null;
  // Join the two touching segments on each axis only for this corner gesture.
  return [[nw, sw, "right"], [nw, ne, "bottom"]].map(([first, second, edge]) => {
    const a = paneResizeGroup(first.key, edge), b = paneResizeGroup(second.key, edge);
    return { ...a, snapOrigin: a.at, members: new Map([...a.members, ...b.members]),
      min: Math.max(a.min, b.min), max: Math.min(a.max, b.max) };
  });
}

let paneReflowFrame;
function snappedPaneEdge(group, at) {
  at = Math.max(group.min, Math.min(group.max, at));
  let nearest = at, distance = 8 / group.pixels;
  if (Number.isFinite(group.snapOrigin) && Math.abs(group.snapOrigin - at) <= distance) {
    nearest = group.snapOrigin; distance = Math.abs(group.snapOrigin - at);
  }
  for (const tile of paneLayout.tiles) for (const side of group.horizontal ? ["left", "right"] : ["top", "bottom"]) {
    const target = tile[side];
    if (group.members.get(tile) === side || target <= 0 || target >= 1
      || target < group.min || target > group.max) continue;
    const delta = Math.abs(target - at);
    if (delta <= distance) { nearest = target; distance = delta; }
  }
  return nearest;
}

function resizePaneEdges(moves) {
  // Resolve both axes before changing geometry so one frame sees one layout.
  const resolved = moves.map(({ group, at }) => ({ group, at: snappedPaneEdge(group, at) }));
  for (const ui of sessions.values()) {
    if (!ui.reflow) rememberPosition(ui);
    ui.reflow = true;
  }
  for (const { group, at } of resolved) for (const [tile, side] of group.members) tile[side] = at;
  paneLayout.custom = true;
  paintPaneLayout();
  cancelAnimationFrame(paneReflowFrame);
  paneReflowFrame = requestAnimationFrame(() => {
    for (const ui of sessions.values()) { restorePosition(ui); ui.reflow = false; }
  });
}

function finishPaneResize(cancel = false) {
  const drag = paneResize;
  if (!drag) return;
  paneResize = null;
  drag.handle.classList.remove("dragging");
  if (cancel) paneLayout = drag.before;
  document.body.classList.remove("resizing-panes");
  document.body.style.removeProperty("--resize-cursor");
  if (drag.handle.hasPointerCapture(drag.pointerId)) drag.handle.releasePointerCapture(drag.pointerId);
  paintPaneLayout();
  for (const ui of sessions.values()) restorePosition(ui);
  rememberPaneOrder();
}

function makePaneResizeHandle(corner = false) {
  const handle = element("div", corner ? "pane-resize pane-resize-corner" : "pane-resize");
  handle.id = `pane-resize-${++paneResizeHandleId}`;
  handle.tabIndex = 0;
  handle.setAttribute("role", corner ? "button" : "separator");
  if (corner) {
    handle.setAttribute("aria-label", "Resize four panes; use arrow keys to move the intersection");
  } else {
    handle.setAttribute("aria-valuemin", "0"); handle.setAttribute("aria-valuemax", "100");
  }
  const groupsForHandle = () => corner ? paneCornerGroups(handle.dataset.cornerOwnerKey)
    : [paneResizeGroup(handle.dataset.ownerKey, handle.dataset.edge)].filter(Boolean);
  handle.addEventListener("pointerdown", event => {
    if (event.button !== 0 || !event.isPrimary || paneResize) return;
    const groups = groupsForHandle();
    if (!groups?.length) return;
    event.preventDefault(); event.stopPropagation();
    paneResize = { groups, corner, size: groups[0].size, before: structuredClone(paneLayout),
      handle, pointerId: event.pointerId, originX: event.clientX, originY: event.clientY };
    handle.classList.add("dragging");
    handle.setPointerCapture(event.pointerId);
    document.body.style.setProperty("--resize-cursor", corner ? "all-scroll" : groups[0].horizontal ? "col-resize" : "row-resize");
    document.body.classList.add("resizing-panes");
  });
  handle.addEventListener("pointermove", event => {
    const drag = paneResize;
    if (!drag || drag.handle !== handle || drag.pointerId !== event.pointerId) return;
    resizePaneEdges(drag.groups.map(group => ({ group, at: group.at + (group.horizontal
      ? event.clientX - drag.originX : event.clientY - drag.originY) / group.pixels })));
  });
  handle.addEventListener("pointerup", event => { if (paneResize?.pointerId === event.pointerId) finishPaneResize(); });
  handle.addEventListener("pointercancel", () => finishPaneResize(true));
  handle.addEventListener("lostpointercapture", () => finishPaneResize(true));
  handle.addEventListener("keydown", event => {
    const horizontal = ["ArrowLeft", "ArrowRight"].includes(event.key);
    const group = groupsForHandle()?.find(group => group.horizontal === horizontal);
    const keys = horizontal ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
    if (!group || !keys.includes(event.key)) return;
    event.preventDefault(); event.stopPropagation();
    resizePaneEdges([{ group, at: group.at + (keys.indexOf(event.key) ? 1 : -1) * (event.shiftKey ? 40 : 10) / group.pixels }]);
    rememberPaneOrder();
  });
  paneResizeLayer.append(handle);
  return handle;
}

document.addEventListener("keydown", event => {
  if (event.key === "Escape" && paneResize) {
    event.preventDefault(); event.stopImmediatePropagation(); finishPaneResize(true);
  }
}, true);
window.addEventListener("blur", () => finishPaneResize(true));

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
  tab.draggable = true;
  let dragBlocked = false;
  tab.addEventListener("pointerdown", event => {
    dragBlocked = Boolean(event.target.closest("button, input, textarea, [contenteditable=true]"));
  });
  tab.addEventListener("dragstart", event => {
    if (!catalogReady || dragBlocked || event.target.closest("button, input, textarea, [contenteditable=true]")
      || tab.querySelector("input")) { event.preventDefault(); return; }
    finishPaneResize(true);
    finishPaneDrag();
    paneDrag = { pane, screen: activeScreen };
    pane.classList.add("pane-dragging");
    const key = pane.dataset.paneKey;
    event.dataTransfer.setData("application/x-boid-local-pane", key);
    if (!key.startsWith("draft:")) event.dataTransfer.setData("application/x-boid-pane", key);
    event.dataTransfer.effectAllowed = "move";
  });
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
  const contentHeight = ui.content.offsetHeight;
  // History replacement and collapsed entries can shorten the transcript.
  // Retain only the composer gap, not the removed transcript's coordinates.
  if (contentHeight < ui.promptContentHeight) {
    ui.retainedPromptEnd = Math.max(0, ui.retainedPromptEnd - (ui.promptContentHeight - contentHeight));
  }
  ui.promptContentHeight = contentHeight;
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
  const viewport = ui.trace.getBoundingClientRect();
  const top = viewport.top;
  const anchor = ui.anchor?.node;
  if (anchor?.isConnected && !anchor.hidden) {
    const bounds = anchor.getBoundingClientRect();
    if (bounds.top <= top && bounds.bottom > top) {
      ui.anchor.offset = bounds.top - top;
      return;
    }
  }
  const node = [...ui.trace.querySelectorAll(".entry")].find(node => {
    if (node.hidden) return false;
    const bounds = node.getBoundingClientRect();
    return bounds.bottom > top && bounds.top < viewport.bottom;
  });
  ui.anchor = node ? { node, offset: node.getBoundingClientRect().top - top } : null;
}

function traceEnd(ui) {
  return Math.max(0, ui.trace.scrollHeight - ui.trace.clientHeight);
}

function scrollTrace(ui, top) {
  top = Math.max(0, Math.min(top, traceEnd(ui)));
  // Scroll metrics round to whole pixels; scrollTop can retain a fraction.
  if (Math.abs(ui.trace.scrollTop - top) > 1) ui.trace.scrollTop = top;
  // Programmatic movement must not be interpreted as a new reading gesture.
  ui.lastScrollTop = ui.trace.scrollTop;
}

function atTraceEnd(ui) {
  return ui.trace.scrollTop >= traceEnd(ui) - 1;
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

function questionWaiting(question) {
  return question.blocking && !question.answer && !question.wait_outcome && question.wait_until_ms > Date.now();
}

function createQuestions(ui) {
  ui.questions = [];
  ui.questionDrafts = new Map();
  ui.questionsBox = element("div", "questions-box");
  ui.questionsBox.hidden = true;
  ui.questionsToggle = element("button", "questions-toggle");
  ui.questionsToggle.type = "button";
  ui.questionsToggle.setAttribute("aria-expanded", "false");
  ui.questionPanel = element("div", "question-panel");
  ui.questionPanel.id = `questions-${ui.id}`;
  ui.questionsToggle.setAttribute("aria-controls", ui.questionPanel.id);
  ui.questionPanel.hidden = true;
  ui.questionSelect = element("select", "question-select");
  ui.questionSelect.setAttribute("aria-label", "Choose a question");
  ui.questionText = element("p", "question-text");
  ui.questionText.id = `question-text-${ui.id}`;
  ui.questionMeta = element("p", "question-meta");
  ui.questionMeta.setAttribute("role", "status");
  ui.questionForm = element("form", "question-form");
  ui.questionInput = element("textarea", "question-reply");
  ui.questionInput.rows = 2;
  ui.questionInput.maxLength = 32000;
  ui.questionInput.placeholder = "Reply…";
  ui.questionInput.setAttribute("aria-label", "Answer this question");
  ui.questionInput.setAttribute("aria-describedby", ui.questionText.id);
  ui.questionSubmit = element("button", "question-submit", "Send reply");
  ui.questionSubmit.type = "submit";
  ui.questionError = element("p", "question-error");
  ui.questionError.hidden = true;
  ui.questionError.setAttribute("role", "alert");
  ui.questionForm.append(ui.questionInput, ui.questionSubmit);
  ui.questionPanel.append(ui.questionSelect, ui.questionText, ui.questionMeta, ui.questionForm, ui.questionError);
  ui.questionsBox.append(ui.questionsToggle, ui.questionPanel);
  ui.pane.append(ui.questionsBox);
  ui.questionsToggle.onclick = () => {
    ui.questionPanel.hidden = !ui.questionPanel.hidden;
    ui.questionsToggle.setAttribute("aria-expanded", String(!ui.questionPanel.hidden));
  };
  ui.questionSelect.onchange = () => selectQuestion(ui, ui.questionSelect.value);
  ui.questionInput.oninput = () => ui.questionDrafts.set(ui.questionId, ui.questionInput.value);
  ui.questionInput.addEventListener("keydown", event => {
    if (event.isComposing) return;
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      ui.questionForm.requestSubmit();
    }
  });
  ui.questionForm.onsubmit = async event => {
    event.preventDefault();
    const id = ui.questionId;
    const text = ui.questionInput.value;
    if (ui.questionSending || questionSubmissions.has(ui.id) || !id || !text.trim()) return;
    ui.questionSending = true;
    ui.questionSubmit.disabled = true;
    ui.questionError.hidden = true;
    try {
      const submission = request(`/api/sessions/${encodeURIComponent(ui.id)}/questions/${encodeURIComponent(id)}/answer`, "POST", { text });
      questionSubmissions.set(ui.id, submission);
      await submission;
      ui.questionDrafts.delete(id);
      // The SSE update can arrive before the HTTP response. Never remove a different question.
      setQuestions(ui, ui.questions.filter(q => q.id !== id));
    } catch (error) { showError(ui.questionError, error); }
    finally { questionSubmissions.delete(ui.id); ui.questionSending = false; ui.questionSubmit.disabled = false; }
  };
}

function selectQuestion(ui, id) {
  if (ui.questionId && ui.questionId !== id) ui.questionDrafts.set(ui.questionId, ui.questionInput.value);
  if (ui.questionId !== id) ui.questionInput.value = ui.questionDrafts.get(id) || "";
  ui.questionId = id;
  ui.questionSelect.value = id || "";
  const question = ui.questions.find(q => q.id === id);
  ui.questionText.textContent = question?.question || "";
  ui.questionError.hidden = true;
  updateQuestionAttention(ui);
}

function setQuestions(ui, questions) {
  const previous = new Set(ui.questions.map(q => q.id));
  ui.questions = (questions || []).filter(q => !q.answer);
  ui.view.questions = ui.questions;
  ui.questionsBox.hidden = !ui.questions.length;
  const urgent = ui.questions.find(questionWaiting);
  const freshUrgent = urgent && !previous.has(urgent.id);
  const selected = ui.questions.find(q => q.id === ui.questionId);
  const next = freshUrgent && !ui.questionInput.value ? urgent : selected || urgent || ui.questions[0];
  ui.questionSelect.replaceChildren(...ui.questions.map(q => {
    const option = element("option", "", `${q.blocking ? "! " : ""}${q.question.replace(/\s+/g, " ").slice(0, 100)}`);
    option.value = q.id;
    return option;
  }));
  ui.questionSelect.hidden = ui.questions.length < 2;
  if (freshUrgent) ui.questionPanel.hidden = false;
  selectQuestion(ui, next?.id);
  if (!ui.questions.length) ui.questionPanel.hidden = true;
  ui.questionsToggle.setAttribute("aria-expanded", String(!ui.questionPanel.hidden));
  if (ui.activity) updateActivity(ui);
}

function updateQuestionAttention(ui) {
  if (!ui.questionsBox) return;
  const urgent = ui.questions.some(questionWaiting);
  ui.pane.classList.toggle("needs-answer", urgent);
  ui.questionsBox.classList.toggle("blocking", urgent);
  const count = ui.questions.length;
  ui.questionsToggle.textContent = `${urgent ? "⚠ Reply needed" : "Questions"} · ${count}`;
  const question = ui.questions.find(q => q.id === ui.questionId);
  if (!question) return;
  if (questionWaiting(question)) {
    const seconds = Math.ceil((question.wait_until_ms - Date.now()) / 1000);
    const time = seconds >= 60 ? `${Math.ceil(seconds / 60)}m` : `${seconds}s`;
    ui.questionMeta.textContent = `Waiting for reply · can continue in ${time}`;
  } else {
    ui.questionMeta.textContent = question.blocking ? "Wait ended · a reply is still welcome" : "Reply when convenient · work can continue";
  }
}

function addSession(view) {
  const id = String(view.id);
  if (sessions.has(id)) return sessions.get(id);
  const savedState = agentScreenState[id];
  const ui = makePane(view.name || id, view.cwd, "agent", id);
  ui.view = view;
  ui.orderKey = orderKey;
  ui.history = view.history ? { ...view.history } : null;
  ui.historyGeneration = 0;
  for (const message of view.communications || []) rememberCommunication(message);
  ui.id = id;
  createQuestions(ui);
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
  ui.promptContentHeight = 0;
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
      ? Math.max(0, Math.min(ui.trace.scrollTop, traceEnd(ui)) - messageBottom(ui)) : 0;
    rememberPosition(ui);
    if (ui.follow && !wasFollowing) scheduleTranscript(ui);
    if (ui.history) {
      scheduleViewport(ui);
      if (top < 240 && ui.history.start > 0) loadHistory(ui, "before");
      else if (movedDown && atTraceEnd(ui) && ui.history.end < ui.history.total) loadHistory(ui, "after");
    }
  }, { passive: true });
  ui.trace.addEventListener("wheel", event => {
    if (event.ctrlKey || event.shiftKey || !event.deltaY || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
    const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE
      ? parseFloat(getComputedStyle(ui.trace).lineHeight)
      : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? ui.trace.clientHeight : 1;
    const delta = event.deltaY * unit;
    const top = ui.trace.scrollTop;
    const end = traceEnd(ui);
    const boundary = delta < 0 ? 0 : end;
    const reachesBoundary = delta < 0 ? top + delta <= 0 : top + delta >= end;
    if (reachesBoundary && event.cancelable) {
      // Stop at the boundary before native scrolling and follow restoration compete.
      event.preventDefault();
      scrollTrace(ui, boundary);
      if (delta > 0 && promptVisible(ui)) {
        ui.followClearance = Math.max(0, Math.min(ui.trace.scrollTop, end) - messageBottom(ui));
      }
      if (delta > 0 && ui.history && ui.history.end < ui.history.total) loadHistory(ui, "after");
    }
    if (event.deltaY < 0) {
      ui.follow = false;
      ui.lastScrollTop = ui.trace.scrollTop;
      rememberPosition(ui);
    } else if (event.deltaY > 0 && !ui.follow && (reachesBoundary || atTraceEnd(ui)) && !selectedWithin(ui.trace) && (!ui.history || ui.history.end >= ui.history.total)) {
      ui.follow = true;
      scheduleTranscript(ui);
    }
  }, { passive: false });
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
    const submittedAt = inputTiming.startedAt();
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
    const timing = inputTiming.begin(id, submittedAt);
    try {
      const submitted = !ui.slashSuggestion.hidden ? ui.slashCommand : text;
      ui.submission = request(`/api/sessions/${encodeURIComponent(id)}/messages`, "POST", { text: submitted }, {},
        timing ? (response, headersAt) => inputTiming.response(timing, response, headersAt) : null);
      const acknowledgment = await ui.submission;
      inputTiming.acknowledge(timing, acknowledgment);
      if (agentScreenState[id]?.input === text) agentScreenState[id].input = "";
      try {
        const saved = JSON.parse(sessionStorage.getItem(ui.orderKey));
        if (saved?.agents?.[id]?.input === text) {
          saved.agents[id].input = ""; sessionStorage.setItem(ui.orderKey, JSON.stringify(saved));
        }
      } catch {}
      if (ui.input.value === text) { ui.input.value = ""; historyIndex = null; resizeInput(); if (ui.pane.isConnected) inputTiming.cleared(timing); }
      const current = sessions.get(id);
      if (current && current !== ui && current.input.value === text) {
        current.input.value = "";
        current.input.dispatchEvent(new Event("input"));
        inputTiming.cleared(timing);
      }
    } catch (error) { inputTiming.failed(timing, error); showError(ui.error, error); }
    finally { ui.sending = false; ui.submission = null; }
  });
  sessions.set(id, ui);
  setQuestions(ui, view.questions || []);
  updateContextUsage(ui);
  setStatus(ui, view.status);
  for (const [index, entry] of (view.entries || []).entries()) putEntry(ui, { ...entry, history_index: (view.history?.start || 0) + index });
  setPending(ui, view.pending || []);
  followLatest(ui);
  layout();
  if (savedState) { agentScreenState[id] = savedState; restoreAgentState(ui); }
  return ui;
}

function updateActivity(ui) {
  const since = ui.view?.working_since_ms;
  const seconds = Number.isFinite(since) ? Math.max(0, Math.floor((Date.now() - since) / 1000)) : null;
  const elapsed = seconds === null ? "" : seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  ui.activity.firstChild.textContent = ui.interruptRequested ? "Interrupting" : (ui.questions || []).some(questionWaiting) ? "Waiting for reply" : "Working";
  if (ui.interruptRequested) ui.pane.dataset.interruptRequested = "true";
  else delete ui.pane.dataset.interruptRequested;
  const hint = ui.interruptRequested || pickerOpen(ui) ? "" : "Esc to interrupt";
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
    scheduleViewport(ui);
  });
  details.addEventListener("toggle", () => {
    if (!details.isConnected) return;
    if (Boolean(item[openKey]) === details.open) return;
    item[openKey] = details.open;
    restorePosition(ui);
    scheduleViewport(ui);
  });
  return block;
}

function toolCommand(tool) {
  const value = tool.value;
  if (!value || typeof value !== "object") return tool.payload;
  if (tool.name === "read_skill" && typeof value.name === "string") return value.name;
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
  const counterpart = related || null;
  const call = item.kind === "tool_call" ? item : counterpart;
  const result = item.kind === "tool_result" ? item : counterpart;
  const owner = call && ui.mounted.has(call) ? call : result;
  const expandedTool = Boolean(call?.expandedTool || result?.expandedTool);
  if (!owner) return;
  const command = toolEntry((call || owner).text);
  const outcome = toolOutcome(result ? toolEntry(result.text) : null);
  const text = call && !call.partial ? toolCommand(command) : "";
  const newline = text.indexOf("\n");
  const firstLine = newline < 0 ? text : text.slice(0, newline);
  const rest = newline < 0 ? "" : text.slice(newline + 1);
  const output = result ? (result.partial ? fullEntryButton(ui, result, "Load full result") : toolOutput(toolEntry(result.text))) : null;
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
  if (result && result !== owner) result.body.replaceChildren();
  const header = element("summary", "tool-header");
  header.append(element("span", "tool-name", command.name === "exec_command" ? "$" : command.name));
  if (firstLine) header.append(element("span", "tool-command", firstLine));
  header.append(element("span", "tool-status", outcome.label));
  const details = element("details", "activity-details tool-operation");
  details.open = expandedTool;
  header.title = "Expand or collapse tool call and result";
  details.append(header);
  if (call?.partial) details.append(fullEntryButton(ui, call, "Load full arguments"));
  else if (rest) details.append(element("pre", "tool-text activity-full", rest));
  owner.body.append(details);
  if (result && output) {
    // Intervening peer or assistant text keeps its place between call and result.
    if (call && !joined && result !== owner) result.body.append(element("div", "tool-note", `${command.name} result`));
    result.body.id = `tool-result-${result.id}`;
    header.setAttribute("aria-controls", result.body.id);
    (result === owner ? details : result.body).append(output);
  }
  function showResult(open) {
    if (!result) return;
    result.node.hidden = result !== owner && !open;
    if (output) output.hidden = !open;
  }
  showResult(expandedTool);
  header.addEventListener("click", event => {
    event.preventDefault();
    rememberPosition(ui);
    details.open = !details.open;
    for (const entry of [call, result].filter(Boolean)) entry.expandedTool = details.open;
    showResult(details.open);
    for (const entry of [call, result].filter(Boolean)) if (ui.mounted.has(entry)) entry.height = measuredHeight(entry);
    restorePosition(ui);
    scheduleViewport(ui);
  });
  details.addEventListener("toggle", () => {
    if (!details.isConnected || Boolean(owner.expandedTool) === details.open) return;
    for (const entry of [call, result].filter(Boolean)) entry.expandedTool = details.open;
    showResult(details.open);
    restorePosition(ui);
    scheduleViewport(ui);
  });
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
    scheduleViewport(ui);
  });
  details.addEventListener("toggle", () => {
    if (!details.isConnected || Boolean(item.expandedSkill) === details.open) return;
    item.expandedSkill = details.open;
    restorePosition(ui);
    scheduleViewport(ui);
  });
}

function communicationFor(ui, item) {
  return communicationEntries.get(`${ui.id}:${item.id}`);
}

async function locateCommunication(ui, message, otherId) {
  try {
    const screen = paneScreen(`agent:${otherId}`);
    if (screen !== activeScreen) await switchScreen(screen);
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
    const heights = history.map(item => item.node.isConnected ? measuredHeight(item) : (item.height ?? 60));
    let start = 0, end = history.length;
    if (ui.follow) {
      // A count of entries does not describe viewport coverage: collapsed tool
      // results can have zero height. Include enough pixels above the tail.
      let height = 0;
      start = end;
      while (start > 0 && (end - start < 35 || height < ui.trace.clientHeight + 700)) height += heights[--start];
    }
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
      while (end < history.length && position < bottom) position += heights[end++];
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
  for (const item of new Set([...ui.mounted, ...wanted])) {
    if (ui.mounted.has(item) === wanted.has(item) || !["tool_call", "tool_result"].includes(item.kind)) continue;
    const pair = ui.toolPairs.get(toolEntry(item.text).heading);
    for (const entry of Object.values(pair || {})) if (wanted.has(entry)) ui.dirty.add(entry);
  }
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
      const readingTop = !ui.follow ? ui.trace.scrollTop : null;
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
      if (readingTop !== null && !ui.anchor?.node.isConnected) scrollTrace(ui, readingTop);
      restorePosition(ui);
      if (ui.history) {
        // Newly measured entries can be much shorter than their cached estimates.
        // Refill only when a virtual spacer intrudes into the visible viewport.
        const bounds = ui.trace.getBoundingClientRect();
        const before = ui.spacerBefore.getBoundingClientRect();
        const after = ui.spacerAfter.getBoundingClientRect();
        if ((before.height > 0 && before.bottom > bounds.top + 10)
            || (after.height > 0 && after.top < bounds.bottom - 10)) scheduleViewport(ui);
      }
      if (ui.history && ui.history.start > 0 && ui.content.offsetHeight < ui.trace.clientHeight) loadHistory(ui, "before");
      inputTiming.rendered(ui);
    });
  });
}

function fullEntryButton(ui, item, label = "Load full message") {
  const load = element("button", "history-more", label); load.type = "button";
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
  return load;
}

function renderEntry(ui, item) {
  if (!ui.mounted.has(item)) { item.rendered = false; return; }
  const { node, body, kind, text } = item;
  node.dataset.pending = ui.pending.has(item.id) ? "Queued" : "";
  if (kind === "reasoning") return;
  if (item.partial && kind !== "tool_call" && kind !== "tool_result") {
    body.replaceChildren(element("pre", "tool-text", text), fullEntryButton(ui, item));
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

function isSkillCatalog(entry) {
  if (entry.kind !== "skill") return false;
  try { return JSON.parse(entry.text)?.event === "catalog"; }
  catch {
    // Older history pages truncate the JSON; the serialized event field comes first.
    return entry.text_complete === false && /^\s*\{\s*"event"\s*:\s*"catalog"\s*[,}]/.test(entry.text || "");
  }
}

function putEntry(ui, entry, delta = false) {
  const id = String(entry.id);
  if (!delta && entry.kind === "user") inputTiming.entry(ui.id, id, "history");
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
    node.hidden = kind === "reasoning" || isSkillCatalog(entry) || (kind === "notice" && /^(Input .* applied to the conversation\.|Queued message (?:[^ ]+ )?from .* applied to this model request\.|Reasoning shown here is the provider's exposed summary\.)/.test(entry.text || ""));
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

function snapshot(views, terminalViews = [], catalog = views.map(view => String(view.id))) {
  communications.clear();
  communicationEntries.clear();
  for (const view of views) for (const message of view.communications || []) rememberCommunication(message);
  const revision = ++catalogRevision;
  const active = new Set(catalog);
  for (const [id, lifecycle] of sessionLifecycle) {
    if (lifecycle.open && !active.has(id)) sessionLifecycle.set(id, { open: false, revision });
  }
  for (const id of active) sessionLifecycle.set(id, { open: true, revision });
  views = views.filter(view => paneScreen(`agent:${view.id}`) === activeScreen);
  views = [...views].sort((a, b) => String(a.id).localeCompare(String(b.id)));
  terminalViews = [...terminalViews].sort((a, b) => String(a.id).localeCompare(String(b.id)));
  for (const ui of sessions.values()) captureAgentState(ui);
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
    setQuestions(ui, view.questions || []);
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
  screenLoaded?.();
  const ready = new Promise(resolve => { screenLoaded = resolve; });
  const source = events = new EventSource(`/api/events?history=recent&screen=${activeScreen}`);
  events.onopen = () => { document.body.classList.remove("disconnected"); };
  events.onerror = () => {
    document.body.classList.add("disconnected");
    request("/api/config").catch(error => {
      if (error.status === 401) { events.close(); requireLogin(); }
    });
  };
  events.onmessage = event => {
    if (events !== source) return;
    try {
      const message = JSON.parse(event.data);
      if (message.type === "snapshot") {
        lastSequence = message.seq;
        receiveScreens(message.screens, false);
        snapshot(message.sessions, message.terminals || [], message.catalog);
        screenLoaded?.(); screenLoaded = null;
        return;
      }
      if (message.seq <= lastSequence) return;
      lastSequence = message.seq;
      const revision = ++catalogRevision;
      if (message.type === "session") sessionLifecycle.set(String(message.session.id), { open: true, revision });
      if (message.type === "closed") sessionLifecycle.set(String(message.session_id), { open: false, revision });
      if (["session", "closed", "status", "name"].includes(message.type)) refreshChoices();
      if (message.type === "screens") { receiveScreens(message.screens); return; }
      if (message.type === "session") {
        const key = `agent:${message.session.id}`;
        if (message.session.screen) screenCatalog.panes[key] = message.session.screen;
        screenCatalog.revision = Math.max(screenCatalog.revision || 0, message.screens_revision || 0);
        if (paneScreen(key) !== activeScreen) return;
        addSession(message.session);
        refreshPeerEntries(String(message.session.id));
        return;
      }
      if (message.type === "terminal") {
        screenCatalog.revision = Math.max(screenCatalog.revision || 0, message.screens_revision || 0);
        if (message.terminal.screen) screenCatalog.panes[`terminal:${message.terminal.id}`] = message.terminal.screen;
        terminalCatalog.set(String(message.terminal.id), message.terminal);
        reconcileTerminals([...terminalCatalog.values()]);
        return;
      }
      if (message.type === "communication") { receiveCommunication(message.message); return; }
      if (message.type === "entry" && message.entry?.kind === "user") inputTiming.entry(message.session_id, message.entry.id, "sse");
      const ui = sessions.get(String(message.session_id));
      if (message.type === "closed") {
        reportUnsavedEnd(message);
        if (ui) removeSession(ui);
        refreshPeerEntries(String(message.session_id));
        return;
      }
      if (!ui) return;
      if (message.type === "questions") { setQuestions(ui, message.questions); return; }
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
  return ready;
}

function reconcileTerminals(infos) {
  terminalCatalog.clear();
  for (const info of infos) terminalCatalog.set(String(info.id), info);
  for (const [id, ui] of terminals) {
    const info = terminalCatalog.get(id);
    if (!info || info.archived || !info.running || paneScreen(`terminal:${id}`) !== activeScreen) ui.dispose();
    else ui.update(info);
  }
  for (const info of infos) {
    if (info.running && !info.archived && paneScreen(`terminal:${info.id}`) === activeScreen && !terminals.has(String(info.id))) attachTerminal(info);
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
  ui.screen = activeScreen;
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
      const view = await request(`/api/sessions?screen=${ui.screen}`, "POST", fields);
      const paneKey = `agent:${view.id}`;
      screenCatalog.panes[paneKey] ??= ui.screen;
      if (ui.screen !== activeScreen || drafts.get(ui.id) !== ui) {
        const key = `boid:pane-order:v1:${location.origin}:${agentDefaults.cwd}${ui.screen === 1 ? "" : `:screen:${ui.screen}`}`;
        try { const saved = JSON.parse(sessionStorage.getItem(key));
          if (saved) { saved.drafts = saved.drafts.filter(draft => draft.id !== ui.id); sessionStorage.setItem(key, JSON.stringify(saved)); }
        } catch {}
        return;
      }
      if (paneScreen(paneKey) !== activeScreen) { discardSetup(ui); return; }
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
          if (ui.screen !== activeScreen || drafts.get(ui.id) !== ui) return;
          const destination = paneScreen(`agent:${view.id}`);
          if (destination !== activeScreen) {
            discardSetup(ui); await switchScreen(destination);
            const target = sessions.get(String(view.id)); if (target) focusSession(target);
            return;
          }
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
  const screen = activeScreen;
  try {
    const info = await request(`/api/terminals?screen=${screen}`, "POST", { cols: 80, rows: 24, cwd: agentDefaults.cwd });
    const paneKey = `terminal:${info.id}`;
    screenCatalog.panes[paneKey] ??= screen;
    if (!terminalCatalog.has(String(info.id))) terminalCatalog.set(String(info.id), info);
    if (paneScreen(paneKey) === activeScreen) attachTerminal(terminalCatalog.get(String(info.id)));
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

let enrollmentCapability = "";
const passkeySignIn = document.querySelector("#passkey-sign-in");
const passkeyEnroll = document.querySelector("#passkey-enroll");

function fromBase64url(value) {
  const raw = atob(value.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(value.length / 4) * 4, "="));
  return Uint8Array.from(raw, c => c.charCodeAt(0));
}
function toBase64url(value) {
  return btoa(String.fromCharCode(...new Uint8Array(value))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function passkeyOptions(options, registration) {
  const publicKey = options.publicKey;
  publicKey.challenge = fromBase64url(publicKey.challenge);
  if (registration) publicKey.user.id = fromBase64url(publicKey.user.id);
  for (const item of publicKey[registration ? "excludeCredentials" : "allowCredentials"] || []) item.id = fromBase64url(item.id);
  return { publicKey };
}
function passkeyResponse(credential) {
  const response = { clientDataJSON: toBase64url(credential.response.clientDataJSON) };
  for (const key of ["attestationObject", "authenticatorData", "signature", "userHandle"]) {
    if (credential.response[key]) response[key] = toBase64url(credential.response[key]);
  }
  if (credential.response.getTransports) response.transports = credential.response.getTransports();
  return { id: credential.id, rawId: toBase64url(credential.rawId), type: credential.type, response,
    extensions: credential.getClientExtensionResults() };
}
async function refreshPasskeyActions() {
  passkeyEnroll.hidden = !enrollmentCapability;
  document.querySelector("#token-fallback").hidden = !!enrollmentCapability;
  passkeySignIn.hidden = true;
  if (!window.PublicKeyCredential || !window.isSecureContext) {
    if (enrollmentCapability) showError(workspaceError, "Passkeys are unavailable at this address or in this browser.");
    passkeyEnroll.disabled = true;
    document.querySelector("#token-fallback").hidden = false;
    return;
  }
  try {
    const status = await request("/auth/passkey/status");
    passkeySignIn.hidden = !status.available || !!enrollmentCapability;
  } catch { /* Token fallback stays available if passkeys are unavailable. */ }
}
async function usePasskey(registration) {
  passkeyEnroll.disabled = passkeySignIn.disabled = true;
  workspaceError.hidden = true;
  try {
    const prefix = registration ? "/auth/passkey/register" : "/auth/passkey/sign-in";
    const capability = enrollmentCapability;
    if (registration) enrollmentCapability = "";
    const options = await request(`${prefix}/start`, "POST", registration ? { capability } : undefined);
    const credential = registration
      ? await navigator.credentials.create(passkeyOptions(options, true))
      : await navigator.credentials.get(passkeyOptions(options, false));
    if (!credential) throw new Error("Passkey operation was cancelled.");
    await request(`${prefix}/finish`, "POST", passkeyResponse(credential));
    await connect();
  } catch (error) {
    requireLogin();
    const message = error.name === "NotAllowedError" ? "Passkey operation was cancelled or timed out." : error.message;
    showError(workspaceError, message + (registration ? " Create a new enrollment link through SSH to try again." : ""));
  } finally { passkeyEnroll.disabled = passkeySignIn.disabled = false; }
}
passkeySignIn.addEventListener("click", () => usePasskey(false));
passkeyEnroll.addEventListener("click", () => usePasskey(true));

function requireLogin() {
  login.hidden = false;
  refreshPasskeyActions();
  document.body.classList.add("disconnected");
  addScreenButton.disabled = true;
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
  receiveScreens(await request("/api/screens"), false);
  if (!orderKey) {
    let saved; try { saved = sessionStorage.getItem(activeScreenKey); } catch {}
    const number = Number(new URLSearchParams(location.search).get("screen") || saved || 1);
    activeScreen = Number.isInteger(number) && number > 0 && number <= screenCatalog.count ? number : 1;
  }
  storeScreenSelection();
  loadPaneOrder(agentDefaults.cwd);
  addScreenButton.disabled = false;
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
  enrollmentCapability = parameters.get("enroll") || "";
  try {
    launchCwd = parameters.has("cwd") ? parameters.get("cwd")
      : token ? "" : sessionStorage.getItem("boid:launch-cwd") || "";
    sessionStorage.setItem("boid:launch-cwd", launchCwd);
  } catch { launchCwd = parameters.get("cwd") || ""; }
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);
  if (enrollmentCapability) { requireLogin(); return; }
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
    await questionSubmissions.get(ui.id)?.catch(() => {});
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
  for (const ui of sessions.values()) {
    updateQuestionAttention(ui);
    if (ui.view?.status === "working") updateActivity(ui);
  }
}, 1000);
window.addEventListener("hashchange", initialize);
initialize();
window.addEventListener("resize", layout, { passive: true });
layout();
