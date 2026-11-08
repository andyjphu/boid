"use strict";

const workspace = document.querySelector("#workspace");
const empty = document.querySelector("#empty");
const workspaceError = document.querySelector("#workspace-error");
const login = document.querySelector("#login");
const sessions = new Map();
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
let savedHistory = new Set();
let orderKey = "";
let catalogReady = false;
let reconcilingCatalog = false;

function loadPaneOrder(cwd) {
  const key = `boid:pane-order:v1:${location.origin}:${cwd || ""}`;
  if (key === orderKey) return;
  orderKey = key;
  try {
    const saved = JSON.parse(sessionStorage.getItem(key) || "null");
    const valid = key => typeof key === "string" && /^(agent|terminal|draft):.+/.test(key);
    paneOrder = [...new Set((Array.isArray(saved) ? saved : saved?.order || []).filter(valid))];
    savedHistory = new Set((saved?.history || []).filter(key => valid(key) && key.startsWith("terminal:")));
    for (const draft of saved?.drafts || []) {
      if (draft && typeof draft.id === "string" && ["name", "cwd", "message"].every(key => typeof draft[key] === "string")) {
        const restored = { ...draft };
        if (saved.version !== 2) {
          for (const [key, value] of Object.entries(agentDefaults)) if (restored[key] === value) restored[key] = "";
        }
        addSetup(restored);
      }
    }
  } catch { paneOrder = []; savedHistory.clear(); }
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
      history: [...terminals.values()].filter(ui => ui.history).map(ui => `terminal:${ui.id}`),
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
  const close = element("button", "pane-close", "×");
  close.type = "button";
  close.setAttribute("aria-label", `Close ${name}`);
  close.title = kind === "agent" ? "End and close this agent" : "End and close this terminal";
  tab.append(close, paneIcon(kind), title, directory);
  header.append(tab, status);
  pane.append(header);
  workspace.append(pane);
  return { pane, header, tab, title, status, directory, error, close };
}

function removeSession(ui) {
  ui.traceResize.disconnect();
  if (selected === ui.pane) selected = null;
  ui.pane.remove();
  sessions.delete(ui.id);
  layout();
}

function atBottom(trace) {
  return trace.scrollHeight - trace.scrollTop - trace.clientHeight <= 1;
}

function selectedWithin(node) {
  const selection = window.getSelection();
  return selection && !selection.isCollapsed && selection.rangeCount > 0
    && selection.getRangeAt(0).intersectsNode(node);
}

function rememberPosition(ui) {
  if (ui.follow) { ui.anchor = null; return; }
  const top = ui.trace.getBoundingClientRect().top;
  const node = [...ui.trace.querySelectorAll(".entry")].find(node =>
    !node.hidden && node.getBoundingClientRect().bottom > top);
  ui.anchor = node ? { node, offset: node.getBoundingClientRect().top - top } : null;
}

function restorePosition(ui) {
  if (ui.follow && !selectedWithin(ui.trace)) {
    ui.trace.scrollTop = ui.trace.scrollHeight;
  } else if (ui.anchor?.node.isConnected) {
    ui.trace.scrollTop += ui.anchor.node.getBoundingClientRect().top
      - ui.trace.getBoundingClientRect().top - ui.anchor.offset;
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
  }
});

function followLatest(ui) {
  if (ui.followScheduled) return;
  ui.followScheduled = true;
  requestAnimationFrame(() => {
    ui.followScheduled = false;
    if (ui.follow && !selectedWithin(ui.trace)) {
      ui.trace.scrollTop = ui.trace.scrollHeight;
    }
  });
}

function closeModelPicker(ui) {
  ui.modelPicker.hidden = true;
  ui.composer.hidden = false;
  ui.activity.lastChild.textContent = "Esc to interrupt";
  ui.input.focus({ preventScroll: true });
}

async function openModelPicker(ui) {
  ui.slashSuggestion.hidden = true;
  ui.composer.hidden = true;
  ui.modelPicker.hidden = false;
  ui.activity.lastChild.textContent = "";
  ui.modelAvailable = false;
  ui.modelPicker.replaceChildren(element("div", "model-heading", "Select model and effort"), element("div", "model-hint", "Loading…"));
  ui.modelPicker.focus({ preventScroll: true });
  try {
    const view = await request(`/api/sessions/${encodeURIComponent(ui.id)}`);
    ui.view = view;
    const config = view.profile || { model: view.model };
    if (ui.modelPicker.hidden) return;
    ui.modelAvailable = config.model === "gpt-6-sol" && config.reasoning_effort === "medium"
      && config.cyber_access_program === "daybreak_blue";
    const option = element("div", "model-option", "GPT-6 Sol · Medium · Daybreak" + (ui.modelAvailable ? " (current)" : ""));
    option.tabIndex = 0;
    option.setAttribute("role", "option");
    option.setAttribute("aria-selected", String(ui.modelAvailable));
    option.setAttribute("aria-disabled", String(!ui.modelAvailable));
    option.addEventListener("click", () => { if (ui.modelAvailable) closeModelPicker(ui); });
    const choices = element("div", "model-options");
    choices.setAttribute("role", "listbox");
    choices.setAttribute("aria-label", "Model and effort");
    choices.append(option);
    ui.modelPicker.replaceChildren(element("div", "model-heading", "Select model and effort"), choices);
    if (!ui.modelAvailable) {
      const current = [config.model, config.reasoning_effort, config.cyber_access_program].filter(Boolean).join(" · ");
      ui.modelPicker.append(element("div", "model-hint", `Current: ${current}. This session retains its original profile.`));
    }
    ui.modelPicker.append(element("div", "model-hint", "Enter select · Esc back"));
    option.focus({ preventScroll: true });
  } catch (error) {
    if (!ui.modelPicker.hidden) { closeModelPicker(ui); showError(ui.error, error); }
  }
}

function addSession(view) {
  const id = String(view.id);
  if (sessions.has(id)) return sessions.get(id);
  const ui = makePane(view.name || id, view.cwd, "agent", id);
  ui.view = view;
  ui.id = id;
  ui.entries = new Map();
  ui.pending = new Set(view.pending || []);
  ui.follow = true;
  ui.trace = element("div", "trace");
  ui.trace.tabIndex = 0;
  ui.trace.setAttribute("aria-label", "Live agent trace");
  ui.activity = element("div", "working-status");
  ui.activity.setAttribute("role", "status");
  ui.activity.append("Esc to interrupt");
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
      if (event.key === "Escape" || (event.key === "Enter" && ui.modelAvailable)) closeModelPicker(ui);
      else ui.modelPicker.querySelector(".model-option")?.focus({ preventScroll: true });
    }
  });
  ui.slashSuggestion = element("div", "slash-suggestion", "/model  Select model and effort");
  ui.slashSuggestion.hidden = true;
  ui.slashSuggestion.addEventListener("mousedown", event => event.preventDefault());
  ui.slashSuggestion.addEventListener("click", () => { ui.input.value = "/model"; ui.composer.requestSubmit(); });
  ui.composer = element("form", "composer");
  ui.input = element("textarea");
  ui.input.rows = 1;
  ui.input.placeholder = "Message…";
  ui.input.title = "Enter to send · Shift+Enter for a new line · Esc to interrupt";
  ui.input.setAttribute("aria-label", `Message ${view.name || id}`);
  ui.sending = false;
  ui.trace.addEventListener("scroll", () => {
    if (ui.reflow || ui.trace.clientHeight !== ui.traceHeight) return;
    ui.follow = atBottom(ui.trace) && !selectedWithin(ui.trace);
    rememberPosition(ui);
  }, { passive: true });
  ui.trace.addEventListener("wheel", event => {
    if (event.deltaY < 0) { ui.follow = false; rememberPosition(ui); }
  }, { passive: true });
  ui.composer.append(ui.input);
  ui.pane.append(ui.trace, ui.error, ui.activity, ui.modelPicker, ui.slashSuggestion, ui.composer);
  function resizeInput() {
    ui.input.style.height = "auto";
    ui.input.style.height = `${Math.min(ui.input.scrollHeight, 140)}px`;
    const text = ui.input.value.trim();
    ui.slashSuggestion.hidden = ui.slashDismissed || !text || !"/model".startsWith(text) || ui.input.value.includes("\n");
  }
  let historyIndex = null;
  let savedDraft = "";
  ui.input.addEventListener("input", () => { historyIndex = null; ui.slashDismissed = false; resizeInput(); });
  ui.traceHeight = ui.trace.clientHeight;
  const traceResize = new ResizeObserver(() => {
    ui.traceHeight = ui.trace.clientHeight;
    restorePosition(ui);
  });
  traceResize.observe(ui.trace);
  ui.traceResize = traceResize;
  ui.close.addEventListener("click", async () => {
    ui.close.disabled = true;
    try {
      await request(`/api/sessions/${encodeURIComponent(id)}`, "DELETE");
      if (sessions.has(id)) removeSession(ui);
    } catch (error) { showError(ui.error, error); ui.close.disabled = false; }
  });
  ui.input.addEventListener("keydown", event => {
    if (!event.isComposing && !ui.slashSuggestion.hidden && !event.ctrlKey && !event.metaKey && !event.altKey) {
      if (["Escape", "Tab", "ArrowUp", "ArrowDown"].includes(event.key)) {
        event.preventDefault();
        if (event.key === "Escape") { ui.slashDismissed = true; ui.slashSuggestion.hidden = true; }
        if (event.key === "Tab") { ui.input.value = "/model"; resizeInput(); }
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
    if (text.trim() === "/model" || (!ui.slashSuggestion.hidden && "/model".startsWith(text.trim()))) {
      ui.input.value = "";
      historyIndex = null;
      resizeInput();
      await openModelPicker(ui);
      return;
    }
    ui.sending = true;
    ui.error.hidden = true;
    try {
      ui.submission = request(`/api/sessions/${encodeURIComponent(id)}/messages`, "POST", { text });
      await ui.submission;
      if (ui.input.value === text) { ui.input.value = ""; historyIndex = null; resizeInput(); }
    } catch (error) { showError(ui.error, error); }
    finally { ui.sending = false; ui.submission = null; }
  });
  sessions.set(id, ui);
  setStatus(ui, view.status);
  for (const entry of view.entries || []) putEntry(ui, entry);
  setPending(ui, view.pending || []);
  followLatest(ui);
  layout();
  return ui;
}

function setStatus(ui, status) {
  if (ui.view) ui.view.status = status;
  ui.pane.dataset.status = status || "idle";
  ui.activity.hidden = status !== "working";
  ui.activity.lastChild.textContent = ui.modelPicker.hidden ? "Esc to interrupt" : "";
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
  const counterpart = tool.callId && [...ui.entries.values()].find(entry => {
    if (entry.kind !== counterpartKind) return false;
    const other = toolEntry(entry.text);
    return other.callId === tool.callId && other.heading === tool.heading;
  });
  const call = item.kind === "tool_call" ? item : counterpart;
  const result = item.kind === "tool_result" ? item : counterpart;
  const owner = call || result;
  const command = toolEntry(owner.text);
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
  if (result && result !== owner) result.body.replaceChildren();
  const header = element(expandable ? "summary" : "div", "tool-header");
  header.append(element("span", "tool-name", command.name === "exec_command" ? "$" : command.name));
  if (firstLine) header.append(element("span", "tool-command", firstLine));
  header.append(element("span", "tool-status", outcome.label));
  let details;
  if (expandable) {
    details = element("details", "activity-details tool-operation");
    details.open = Boolean(owner.expandedTool);
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
  showResult(Boolean(owner.expandedTool));
  if (details) {
    header.addEventListener("click", event => {
      event.preventDefault();
      rememberPosition(ui);
      details.open = !details.open;
      owner.expandedTool = details.open;
      showResult(owner.expandedTool);
      restorePosition(ui);
    });
    details.addEventListener("toggle", () => {
      if (!details.isConnected || Boolean(owner.expandedTool) === details.open) return;
      owner.expandedTool = details.open;
      showResult(details.open);
      restorePosition(ui);
    });
  }
}

function renderEntry(ui, item) {
  const { node, body, kind, text } = item;
  node.dataset.pending = ui.pending.has(item.id) ? "Queued" : "";
  if (kind === "reasoning") return;
  if (kind === "assistant") {
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
  let previous;
  for (const item of ui.entries.values()) {
    item.node.classList.remove("user-joined-before", "user-joined-after");
  }
  for (const item of ui.entries.values()) {
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
    node.hidden = kind === "reasoning" || (kind === "notice" && /^(Input .* applied to the conversation\.|Queued message from .* applied to this model request\.|Reasoning shown here is the provider's exposed summary\.)/.test(entry.text || ""));
    if (entry.replaces) {
      const replaced = ui.entries.get(String(entry.replaces));
      if (replaced) {
        replaced.superseded = true;
        if (!selectedWithin(replaced.node)) replaced.node.hidden = true;
      }
    }
    ui.trace.append(node);
    item = { id, node, body, kind, text: "", groupChanged: true };
    ui.entries.set(id, item);
    if (kind === "user") ui.pending.add(id);
  }
  const batchChanged = !delta && item.inputBatch !== entry.input_batch;
  if (batchChanged) item.groupChanged = true;
  if (!delta) item.inputBatch = entry.input_batch;
  const text = delta ? item.text + (entry.text || "") : (entry.text || "");
  if (text !== item.text || !item.rendered || batchChanged) {
    item.text = text;
    if (!item.renderScheduled) {
      item.renderScheduled = true;
      requestAnimationFrame(() => {
        item.renderScheduled = false;
        if (selectedWithin(item.node)) { item.deferred = true; return; }
        rememberPosition(ui);
        renderEntry(ui, item);
        item.rendered = true;
        if (item.groupChanged) {
          groupUserEntries(ui);
          item.groupChanged = false;
        }
        restorePosition(ui);
      });
    }
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
    throw new Error("This agent has ended. Choose it from Inactive to reopen.");
  }
  return addSession(view);
}

function snapshot(views, terminalViews = []) {
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
  if (!catalogReady) {
    for (const info of terminalViews) {
      if (info.archived && savedHistory.has(`terminal:${info.id}`)) attachTerminal(info, true);
    }
  }
  const ids = new Set(views.map(view => String(view.id)));
  for (const [id, ui] of sessions) {
    if (!ids.has(id)) {
      removeSession(ui);
    }
  }
  for (const view of views) {
    const ui = sessions.get(String(view.id));
    if (!ui) { addSession(view); continue; }
    ui.view = view;
    const follow = ui.follow;
    rememberPosition(ui);
    ui.title.textContent = view.name || String(view.id);
    ui.directory.textContent = view.cwd;
    ui.directory.title = view.cwd;
    const present = new Set();
    for (const entry of view.entries || []) {
      present.add(String(entry.id));
      putEntry(ui, entry);
    }
    for (const [id, entry] of ui.entries) {
      if (!present.has(id)) { entry.node.remove(); ui.entries.delete(id); }
    }
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

function startEvents() {
  events?.close();
  events = new EventSource("/api/events");
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
      if (["session", "closed", "status", "terminal"].includes(message.type)) refreshChoices();
      if (message.type === "session") { addSession(message.session); return; }
      if (message.type === "terminal") {
        terminalCatalog.set(String(message.terminal.id), message.terminal);
        reconcileTerminals([...terminalCatalog.values()]);
        return;
      }
      const ui = sessions.get(String(message.session_id));
      if (!ui) return;
      if (message.type === "closed") { removeSession(ui); return; }
      if (message.type === "input") {
        const item = ui.entries.get(String(message.id));
        if (item) item.inputBatch = message.input_batch;
        ui.pending.delete(String(message.id));
        setPending(ui, ui.pending);
      }
      if (message.type === "entry") putEntry(ui, message.entry);
      if (message.type === "delta") putEntry(ui, { id: message.entry_id, text: message.text }, true);
      if (message.type === "status") setStatus(ui, message.status);
    } catch (error) { showError(workspaceError, error); }
  };
}

function reconcileTerminals(infos) {
  terminalCatalog.clear();
  for (const info of infos) terminalCatalog.set(String(info.id), info);
  for (const [id, ui] of terminals) {
    const info = terminalCatalog.get(id);
    if (!info || (info.archived && !ui.history)) ui.dispose();
    else ui.update(info);
  }
  for (const info of infos) {
    if (!info.archived && !terminals.has(String(info.id))) attachTerminal(info);
  }
}

function attachTerminal(info, history = false) {
  const id = String(info.id);
  if (terminals.has(id)) return terminals.get(id);
  const ui = makePane(info.name || "#?", info.cwd, "terminal", id);
  ui.id = id;
  ui.title.title = "Double-click or press F2 to rename";
  ui.title.tabIndex = 0;
  function setName(name) {
    ui.name = name || "#?";
    ui.title.textContent = ui.name;
    ui.pane.setAttribute("aria-label", ui.name);
    ui.close.setAttribute("aria-label", `Close ${ui.name}`);
  }
  setName(info.name);
  ui.setName = setName;
  let editing = false;
  function rename() {
    if (editing) return;
    editing = true;
    const input = element("input", "pane-name-input");
    input.type = "text";
    input.value = ui.name;
    input.maxLength = 128;
    input.size = Math.max(4, Math.min(24, ui.name.length + 2));
    input.setAttribute("aria-label", "Terminal name");
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
      try {
        const summary = await request(`/api/terminals/${encodeURIComponent(id)}`, "PATCH", { name });
        setName(summary.name);
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
  ui.history = history;
  const close = ui.close;
  if (history) close.title = "Close retained terminal record";
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
    ui.status.textContent = info.running ? "" : (info.status || "ended");
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
        if (message.type === "name") setName(message.name);
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
        if (!current.running) {
          ui.update(current);
          ui.pane.classList.remove("disconnected");
          return;
        }
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
    info = latest;
    setName(info.name);
    ui.directory.textContent = info.cwd || "";
    ui.directory.title = (info.cwd || "") + (info.running && info.cwd_observed === false ? " (last known directory)" : "");
    ui.status.textContent = info.running ? "" : (info.status || "ended");
    if (info.persistence_error) showError(ui.error, `History could not be saved: ${info.persistence_error}`);
    if (!info.running) term.options.disableStdin = true;
  };
  close.addEventListener("click", async () => {
    if (ui.history) { ui.dispose(); return; }
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
  name: "canary", cwd: "/work", message: "Inspect the working directory and summarize what is here.",
};
let choicesVersion = 0;
let choicesTimer;

function focusSession(ui) {
  selected = ui.pane;
  ui.pane.scrollIntoView({ block: "nearest", inline: "nearest" });
  if (ui.input) ui.input.focus({ preventScroll: true });
  else ui.title.focus({ preventScroll: true });
}

function discardSetup(ui, replacement) {
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
    name: "", cwd: "", message: "",
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
  ui.form.append(element("h2", null, "New agent"));
  for (const [key, label] of [["name", "Name"], ["cwd", "Working directory"], ["message", "Task"]]) {
    const wrapper = element("label", null, label);
    const input = element(key === "message" ? "textarea" : "input");
    input.name = key;
    input.value = values[key];
    if (key === "message") input.rows = 4;
    input.placeholder = agentDefaults[key];
    input.addEventListener("input", rememberPaneOrder);
    wrapper.append(input);
    ui.form.append(wrapper);
  }
  const submit = element("button", "setup-submit", "Create agent");
  submit.type = "submit";
  ui.form.append(submit);
  const choices = element("div", "session-choices");
  ui.active = element("div", "active-agents");
  ui.inactive = element("div", "inactive-agents");
  ui.terminalHistory = element("div", "terminal-history");
  choices.append(element("h2", null, "Active"), ui.active, element("h2", null, "Inactive"), ui.inactive, element("h2", null, "Terminals"), ui.terminalHistory);
  body.append(ui.form, choices);
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
      for (const [key, value] of Object.entries(agentDefaults)) fields[key] ||= value;
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
  heading.append(element("span", "session-card-name", record.name || "Terminal"), element("span", "session-card-status", record.status || "idle"));
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
          const view = await request(`/api/sessions/${encodeURIComponent(record.id)}${archived ? "/reopen" : ""}`, archived ? "POST" : "GET");
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
      const [active, inactive, terminalRecords] = await Promise.all([
        request("/api/sessions"), request("/api/sessions/archived"), request("/api/terminals"),
      ]);
      if (version !== choicesVersion) return;
      for (const ui of drafts.values()) {
        renderAgentChoices(ui, active, inactive);
        const focused = focusedChoice(ui.terminalHistory);
        ui.terminalHistory.replaceChildren();
        const archived = terminalRecords.filter(record => record.archived);
        if (!archived.length) ui.terminalHistory.append(element("p", "session-empty", "None"));
        for (const record of archived) {
          const card = sessionCard(record, "View read-only terminal history");
          card.addEventListener("click", () => {
            if (ui.busy) return;
            const existing = terminals.get(String(record.id));
            const terminal = attachTerminal(record, true);
            if (existing) { discardSetup(ui); focusSession(terminal); }
            else discardSetup(ui, terminal);
          });
          ui.terminalHistory.append(card);
        }
        restoreChoiceFocus(ui.terminalHistory, focused);
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
    const info = await request("/api/terminals", "POST", { cols: 80, rows: 24 });
    attachTerminal(info);
  } catch (error) { showError(workspaceError, error); }
  finally { button.disabled = false; }
});

function requireLogin() {
  login.hidden = false;
  document.body.classList.add("disconnected");
  document.querySelector("#add-agent").disabled = true;
  document.querySelector("#add-terminal").disabled = true;
}

async function connect(token) {
  if (token) await request("/api/login", "POST", undefined, { Authorization: `Bearer ${token}` });
  const config = await request("/api/config");
  login.hidden = true;
  workspaceError.hidden = true;
  loadPaneOrder(config.cwd);
  document.querySelector("#add-agent").disabled = false;
  document.querySelector("#add-terminal").disabled = false;
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
  const token = new URLSearchParams(location.hash.slice(1)).get("token");
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);
  try { await connect(token); }
  catch (error) {
    requireLogin();
    if (error.status !== 401) showError(workspaceError, error);
  }
}
document.addEventListener("pointerdown", event => {
  selected = event.target.closest(".pane");
});
document.addEventListener("focusin", event => {
  selected = event.target.closest(".pane");
});
document.addEventListener("keydown", async event => {
  const picker = [...sessions.values()].find(ui => ui.pane === selected && !ui.modelPicker.hidden);
  if (picker && event.key === "Escape" && !event.isComposing && !event.defaultPrevented) {
    event.preventDefault();
    closeModelPicker(picker);
    return;
  }
  if (!event.defaultPrevented && !event.isComposing && !picker
      && !event.ctrlKey && !event.metaKey && !event.altKey && event.key.length === 1
      && !event.target.closest("input, textarea, [contenteditable], .model-picker")) {
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
  if (!ui || ui.interrupting || (ui.pane.dataset.status !== "working" && !ui.pending.size)) return;
  event.preventDefault();
  ui.interrupting = true;
  ui.error.hidden = true;
  try {
    await ui.submission?.catch(() => {});
    await request(`/api/sessions/${encodeURIComponent(ui.id)}/interrupt`, "POST");
  }
  catch (error) { showError(ui.error, error); }
  finally { ui.interrupting = false; }
});
initialize();
window.addEventListener("resize", layout, { passive: true });
layout();
