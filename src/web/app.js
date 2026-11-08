"use strict";

const workspace = document.querySelector("#workspace");
const empty = document.querySelector("#empty");
const workspaceError = document.querySelector("#workspace-error");
const login = document.querySelector("#login");
const sessions = new Map();
const terminals = new Map();
let selected = null;
const markdown = window.markdownit({ html: false, linkify: false, typographer: false });
markdown.disable("image");
let lastSequence = -1;
let events;

function element(tag, className, text) {
  const node = document.createElement(tag);
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

function makePane(name, cwd, kind) {
  const pane = element("section", `pane ${kind}-pane`);
  pane.setAttribute("aria-label", name);
  const header = element("header", "pane-header");
  const title = element("span", "pane-title", name);
  const status = element("span", "pane-status", "idle");
  header.append(title, status);
  const directory = element("p", "pane-cwd", cwd || "Server working directory");
  directory.title = cwd || "Server working directory";
  const error = element("p", "error-banner");
  error.setAttribute("role", "alert");
  error.hidden = true;
  header.append(directory);
  const close = element("button", "pane-close", "×");
  close.type = "button";
  close.setAttribute("aria-label", `Close ${name}`);
  close.title = kind === "agent" ? "End and close this agent" : "End and close this terminal";
  header.append(close);
  pane.append(header);
  workspace.append(pane);
  return { pane, header, title, status, directory, error, close };
}

function removeSession(ui) {
  ui.traceResize.disconnect();
  if (selected === ui.pane) selected = null;
  ui.pane.remove();
  sessions.delete(ui.id);
  layout();
}

function atBottom(trace) {
  return trace.scrollHeight - trace.scrollTop - trace.clientHeight < 32;
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

function addSession(view) {
  const id = String(view.id);
  if (sessions.has(id)) return sessions.get(id);
  const ui = makePane(view.name || id, view.cwd, "agent");
  ui.id = id;
  ui.entries = new Map();
  ui.pending = new Set(view.pending || []);
  ui.follow = true;
  ui.trace = element("div", "trace");
  ui.trace.tabIndex = 0;
  ui.trace.setAttribute("aria-label", "Live agent trace");
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
  ui.pane.append(ui.trace, ui.error, ui.composer);
  function resizeInput() {
    ui.input.style.height = "auto";
    ui.input.style.height = `${Math.min(ui.input.scrollHeight, 140)}px`;
  }
  let historyIndex = null;
  let savedDraft = "";
  ui.input.addEventListener("input", () => { historyIndex = null; resizeInput(); });
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
  ui.pane.dataset.status = status || "idle";
  ui.status.textContent = status === "working" ? "Working" : status === "idle" ? "" : (status || "");
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
  summary.addEventListener("click", () => {
    item[openKey] = !details.open;
    ui.follow = false;
    rememberPosition(ui);
  });
  details.addEventListener("toggle", () => {
    if (!details.isConnected) return;
    if (Boolean(item[openKey]) === details.open) return;
    item[openKey] = details.open;
    rememberPosition(ui);
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

function renderToolEntry(ui, item) {
  const { node, body, kind } = item;
  const tool = toolEntry(item.text);
  const counterpartKind = kind === "tool_call" ? "tool_result" : "tool_call";
  const counterpart = tool.callId && [...ui.entries.values()].find(entry => {
    if (entry.kind !== counterpartKind) return false;
    const other = toolEntry(entry.text);
    return other.callId === tool.callId && other.heading === tool.heading;
  });
  const call = kind === "tool_call" ? item : counterpart;
  const result = kind === "tool_result" ? item : counterpart;
  const outcome = toolOutcome(result ? toolEntry(result.text) : null);
  let previous = result?.node.previousElementSibling;
  while (previous?.hidden) previous = previous.previousElementSibling;
  const joined = Boolean(call && result && previous === call.node);
  if (call) {
    if (call !== item && selectedWithin(call.node)) {
      call.deferred = true;
    } else {
      call.node.classList.toggle("tool-joined-call", joined);
      call.node.dataset.toolFailed = String(outcome.failed);
      if (call.toolStatus) call.toolStatus.textContent = outcome.label;
    }
  }
  if (result) result.node.classList.toggle("tool-joined-result", joined);
  node.title = tool.heading;
  node.dataset.toolFailed = String(outcome.failed);
  body.replaceChildren();
  const header = element("div", "tool-header");
  let commandContinuation;
  const command = toolEntry(call?.text || item.text);
  const label = command.name === "exec_command" ? "$" : command.name;
  header.append(element("span", "tool-name", label));
  if (kind === "tool_call" || !joined) {
    if (kind === "tool_call" || call) {
      const text = toolCommand(command);
      const newline = text.indexOf("\n");
      const firstLine = newline < 0 ? text : text.slice(0, newline);
      if (newline >= 0) {
        const rest = text.slice(newline + 1);
        commandContinuation = compactOutput(ui, item, element("pre", "tool-text", rest), rest, "Input");
      }
      const commandBody = element("pre", "tool-text", firstLine);
      const commandView = compactOutput(ui, item, commandBody, firstLine, "Command");
      commandView.classList.add("tool-command");
      header.append(commandView);
    }
    const status = element("span", "tool-status", outcome.label);
    header.append(status);
    if (kind === "tool_call") item.toolStatus = status;
  }
  if (kind === "tool_call") {
    body.append(header);
    if (commandContinuation) body.append(commandContinuation);
    return;
  }
  // Keep results at their original event position when another visible event intervenes.
  if (!joined) {
    body.append(header);
    if (commandContinuation) body.append(commandContinuation);
  }
  const output = element("div", "tool-output");
  const value = tool.value;
  if (value?.ok === true && Object.keys(value).length === 1) return;
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
    output.append(element("pre", "tool-text", value !== undefined ? JSON.stringify(value, null, 2) : tool.payload || item.text));
  }
  const preview = [...output.children].map(child => child.textContent).join("\n");
  body.append(compactOutput(ui, item, output, preview));
}

function renderEntry(ui, item) {
  const { node, body, kind, text } = item;
  node.dataset.pending = ui.pending.has(item.id) ? "Queued" : "";
  if (kind === "assistant" || kind === "reasoning") {
    body.innerHTML = markdown.render(text);
    for (const link of body.querySelectorAll("a")) {
      link.target = "_blank";
      link.rel = "noopener noreferrer";
    }
    if (kind === "reasoning") {
      const content = element("div", "reasoning-output");
      content.append(...body.childNodes);
      const preview = [...content.children].map(child => child.textContent).join("\n");
      body.append(compactOutput(ui, item, content, preview, "Reasoning"));
      body.title = "Provider-exposed reasoning summary";
    }
  } else if (kind === "tool_call" || kind === "tool_result") {
    renderToolEntry(ui, item);
  } else if (kind === "tool_arguments") {
    body.replaceChildren(compactOutput(ui, item, element("pre", "tool-text", text), text, "Arguments"));
  } else {
    body.textContent = text;
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
    node.hidden = kind === "notice" && /^(Input .* applied to the conversation\.|Queued message from .* applied to this model request\.|Reasoning shown here is the provider's exposed summary\.)/.test(entry.text || "");
    if (entry.replaces) {
      const replaced = ui.entries.get(String(entry.replaces));
      if (replaced) {
        replaced.superseded = true;
        if (!selectedWithin(replaced.node)) replaced.node.hidden = true;
      }
    }
    ui.trace.append(node);
    item = { id, node, body, kind, text: "" };
    ui.entries.set(id, item);
    if (kind === "user") ui.pending.add(id);
  }
  const text = delta ? item.text + (entry.text || "") : (entry.text || "");
  if (text !== item.text || !item.rendered) {
    item.text = text;
    if (!item.renderScheduled) {
      item.renderScheduled = true;
      requestAnimationFrame(() => {
        item.renderScheduled = false;
        if (selectedWithin(item.node)) { item.deferred = true; return; }
        renderEntry(ui, item);
        item.rendered = true;
        if (ui.follow) followLatest(ui);
        else restorePosition(ui);
      });
    }
  }
}

function setPending(ui, pending) {
  ui.pending = new Set(pending);
  for (const item of ui.entries.values()) {
    if (item.kind === "user") item.node.dataset.pending = ui.pending.has(item.id) ? "Queued" : "";
  }
}

function snapshot(views) {
  const ids = new Set(views.map(view => String(view.id)));
  for (const [id, ui] of sessions) {
    if (!ids.has(id)) {
      removeSession(ui);
    }
  }
  for (const view of views) {
    const ui = sessions.get(String(view.id));
    if (!ui) { addSession(view); continue; }
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
        snapshot(message.sessions);
        return;
      }
      if (message.seq <= lastSequence) return;
      lastSequence = message.seq;
      if (message.type === "session") { addSession(message.session); return; }
      const ui = sessions.get(String(message.session_id));
      if (!ui) return;
      if (message.type === "closed") { removeSession(ui); return; }
      if (message.type === "input") {
        ui.pending.delete(String(message.id));
        setPending(ui, ui.pending);
      }
      if (message.type === "entry") putEntry(ui, message.entry);
      if (message.type === "delta") putEntry(ui, { id: message.entry_id, text: message.text }, true);
      if (message.type === "status") setStatus(ui, message.status);
    } catch (error) { showError(workspaceError, error); }
  };
}

function attachTerminal(info) {
  const id = String(info.id);
  if (terminals.has(id)) return terminals.get(id);
  const ui = makePane(`Terminal ${id.replace(/^term-/, "").slice(0, 8)}`, info.cwd, "terminal");
  ui.title.title = id;
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
        if (!current || !current.running) { ui.status.textContent = "exited"; return; }
      } catch (error) {
        if (error.status === 401) { requireLogin(); return; }
      }
      retry = setTimeout(connect, 2000);
    };
  }
  term.onData(data => { if (controller && !replaying) send({ type: "input", data }); });
  function takeControl() {
    if (replaying) wantsControl = true;
    else if (!controller) send({ type: "control" });
  }
  host.addEventListener("focusin", takeControl);
  host.addEventListener("pointerdown", takeControl);
  function resumeControl() {
    if (host.contains(document.activeElement)) takeControl();
  }
  window.addEventListener("focus", resumeControl);
  const observer = new ResizeObserver(() => {
    if (host.clientWidth > 0 && host.clientHeight > 0) resize();
  });
  observer.observe(host);
  close.addEventListener("click", async () => {
    close.disabled = true;
    try {
      await request(`/api/terminals/${encodeURIComponent(id)}`, "DELETE");
      closed = true;
      clearTimeout(retry);
      socket?.close();
      observer.disconnect();
      window.removeEventListener("focus", resumeControl);
      term.dispose();
      if (selected === ui.pane) selected = null;
      ui.pane.remove();
      terminals.delete(id);
      layout();
    } catch (error) { showError(ui.error, error); close.disabled = false; }
  });
  ui.reconnect = connect;
  terminals.set(id, ui);
  layout();
  connect();
  return ui;
}

const dialog = document.querySelector("#create-dialog");
const createForm = document.querySelector("#create-form");
const createError = document.querySelector("#create-error");
const createButton = document.querySelector("#submit-create");
let createKind = "agent";
function openCreate(kind) {
  createKind = kind;
  createError.hidden = true;
  document.querySelector("#create-title").textContent = kind === "agent" ? "New agent" : "New terminal";
  createButton.textContent = kind === "agent" ? "Create agent" : "Open terminal";
  document.querySelector("#name-field").hidden = kind !== "agent";
  document.querySelector("#message-field").hidden = kind !== "agent";
  dialog.showModal();
  createForm.elements[kind === "agent" ? "name" : "cwd"].focus();
}
document.querySelector("#add-agent").addEventListener("click", () => openCreate("agent"));
document.querySelector("#add-terminal").addEventListener("click", () => openCreate("terminal"));
document.querySelector("#cancel-create").addEventListener("click", () => dialog.close());
createForm.addEventListener("submit", async event => {
  event.preventDefault();
  createButton.disabled = true;
  createError.hidden = true;
  const fields = Object.fromEntries(new FormData(createForm));
  try {
    if (createKind === "agent") {
      const view = await request("/api/sessions", "POST", fields);
      const ui = addSession(view);
      dialog.close();
      createForm.elements.name.value = "";
      createForm.elements.message.value = "";
      ui.input.focus();
    } else {
      const info = await request("/api/terminals", "POST", { cwd: fields.cwd, cols: 80, rows: 24 });
      attachTerminal(info);
      dialog.close();
    }
  } catch (error) { showError(createError, error); }
  finally { createButton.disabled = false; }
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
  createForm.elements.cwd.value ||= config.cwd || "";
  document.querySelector("#add-agent").disabled = false;
  document.querySelector("#add-terminal").disabled = false;
  startEvents();
  const list = await request("/api/terminals");
  for (const info of list) {
    const existing = terminals.get(String(info.id));
    if (existing) existing.reconnect();
    else attachTerminal(info);
  }
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
  if (event.key !== "Escape" || event.isComposing || event.defaultPrevented || dialog.open) return;
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
