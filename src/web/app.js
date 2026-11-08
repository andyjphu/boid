"use strict";

const workspace = document.querySelector("#workspace");
const empty = document.querySelector("#empty");
const connection = document.querySelector("#connection");
const workspaceError = document.querySelector("#workspace-error");
const login = document.querySelector("#login");
const sessions = new Map();
const terminals = new Map();
let focused = null;
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

function layout() {
  const following = [...sessions.values()].filter(ui => ui.follow);
  for (const ui of sessions.values()) ui.reflow = true;
  const panes = [...workspace.querySelectorAll(".pane")];
  empty.hidden = panes.length > 0;
  const columns = Math.max(1, Math.min(panes.length, Math.floor(workspace.clientWidth / 420)));
  const expanded = focused && panes.length > 1 && columns > 1;
  const spanRows = expanded && columns > 2 ? 2 : 1;
  const cells = panes.length + (expanded ? 2 * spanRows - 1 : 0);
  workspace.style.setProperty("--columns", columns);
  workspace.style.setProperty("--rows", Math.max(1, Math.ceil(cells / columns)));
  for (const pane of panes) {
    const active = pane === focused;
    pane.classList.toggle("focused", active);
    pane.style.gridColumn = active && expanded ? "span 2" : "";
    pane.style.gridRow = active && expanded ? `span ${spanRows}` : "";
    pane.querySelector(".pane-title").setAttribute("aria-expanded", String(active));
  }
  requestAnimationFrame(() => {
    for (const ui of following) {
      if (ui.follow) ui.trace.scrollTop = ui.trace.scrollHeight;
    }
    for (const ui of sessions.values()) ui.reflow = false;
  });
}

function makePane(name, cwd, kind) {
  const pane = element("section", `pane ${kind}-pane`);
  pane.setAttribute("aria-label", name);
  const header = element("header", "pane-header");
  const title = element("span", "pane-title", name);
  title.tabIndex = 0;
  title.setAttribute("role", "button");
  title.title = "Double-click to enlarge; Enter when focused";
  const status = element("span", "pane-status", "idle");
  function toggleSize() { focused = focused === pane ? null : pane; layout(); }
  title.addEventListener("dblclick", toggleSize);
  title.addEventListener("keydown", event => {
    if (event.key === "Enter") { event.preventDefault(); toggleSize(); }
  });
  header.append(title, status);
  const directory = element("p", "pane-cwd", cwd || "Server working directory");
  directory.title = cwd || "Server working directory";
  const error = element("p", "error-banner");
  error.setAttribute("role", "alert");
  error.hidden = true;
  header.append(directory);
  pane.append(header);
  workspace.append(pane);
  return { pane, header, title, status, directory, error };
}

function atBottom(trace) {
  return trace.scrollHeight - trace.scrollTop - trace.clientHeight < 32;
}

function followLatest(ui) {
  if (ui.followScheduled) return;
  ui.followScheduled = true;
  requestAnimationFrame(() => {
    ui.followScheduled = false;
    if (ui.follow) {
      ui.trace.scrollTop = ui.trace.scrollHeight;
      ui.latest.hidden = true;
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
  ui.latest = element("a", "latest", "↓ Latest");
  ui.latest.href = "#";
  ui.latest.hidden = true;
  ui.latest.addEventListener("click", event => { event.preventDefault(); ui.follow = true; followLatest(ui); });
  ui.trace.addEventListener("scroll", () => {
    if (ui.reflow || ui.trace.clientHeight !== ui.traceHeight) return;
    ui.follow = atBottom(ui.trace);
    ui.latest.hidden = ui.follow;
  }, { passive: true });
  ui.trace.addEventListener("wheel", event => {
    if (event.deltaY < 0) { ui.follow = false; ui.latest.hidden = false; }
  }, { passive: true });
  ui.composer.append(ui.input);
  ui.pane.append(ui.trace, ui.latest, ui.error, ui.composer);
  function resizeInput() {
    ui.input.style.height = "auto";
    ui.input.style.height = `${Math.min(ui.input.scrollHeight, 140)}px`;
  }
  ui.input.addEventListener("input", resizeInput);
  ui.traceHeight = ui.trace.clientHeight;
  const traceResize = new ResizeObserver(() => {
    ui.traceHeight = ui.trace.clientHeight;
    if (ui.follow) followLatest(ui);
  });
  traceResize.observe(ui.trace);
  ui.input.addEventListener("keydown", event => {
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
      if (ui.input.value === text) { ui.input.value = ""; resizeInput(); }
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

function renderEntry(ui, item) {
  const { node, body, kind, text } = item;
  node.dataset.pending = ui.pending.has(item.id) ? "Queued" : "";
  if (kind === "assistant" || kind === "reasoning") {
    body.innerHTML = markdown.render(text);
    for (const link of body.querySelectorAll("a")) {
      link.target = "_blank";
      link.rel = "noopener noreferrer";
    }
  } else if (kind === "tool_call" || kind === "tool_result") {
    const split = text.indexOf("\n");
    const heading = split < 0 ? text : text.slice(0, split);
    const payload = split < 0 ? "" : text.slice(split + 1);
    let value;
    try { value = JSON.parse(payload); } catch { /* Partial output stays visible. */ }
    node.title = heading;
    body.replaceChildren();
    if (kind === "tool_call") {
      const name = heading.replace(/ \(.*$/, "");
      body.append(element("div", "tool-name", name === "exec_command" ? "$" : name));
      body.append(element("pre", "tool-text", value?.command ?? (value ? JSON.stringify(value, null, 2) : payload)));
    } else if (value && typeof value === "object" && !Array.isArray(value) && ("stdout" in value || "stderr" in value)) {
      for (const channel of ["stdout", "stderr"]) {
        if (value[channel]?.text) body.append(element("pre", `tool-text ${channel}`, value[channel].text));
        if (value[channel]?.truncated) body.append(element("div", "tool-note", `${channel} truncated`));
      }
      const outcome = value.timed_out ? "Timed out" : value.signal ? `Signal ${value.signal}` : `Exit ${value.exit_code}`;
      body.append(element("div", "tool-note", outcome));
    } else if (value && typeof value.text === "string") {
      body.append(element("pre", "tool-text", value.text));
      if (value.truncated) body.append(element("div", "tool-note", "Output truncated"));
    } else {
      body.append(element("pre", "tool-text", value ? JSON.stringify(value, null, 2) : payload || text));
    }
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
      if (kind === "reasoning") node.append(element("div", "entry-kind", "Reasoning summary"));
      if (kind === "context") node.append(element("div", "entry-kind", "Context"));
      node.append(body);
    }
    // Transport receipts remain in the full record; they are not model output.
    node.hidden = kind === "notice" && /^(Input .* applied to the conversation\.|Queued message from .* applied to this model request\.|Reasoning shown here is the provider's exposed summary\.)/.test(entry.text || "");
    const previous = [...ui.entries.values()].at(-1);
    if (kind === "tool_call" && previous?.kind === "tool_arguments"
        && previous.text.trim() === (entry.text || "").split("\n").slice(1).join("\n").trim()) {
      previous.node.hidden = true;
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
        renderEntry(ui, item);
        item.rendered = true;
        if (ui.follow) followLatest(ui);
      });
    }
  }
  if (!ui.follow) ui.latest.hidden = false;
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
      setStatus(ui, "error");
      showError(ui.error, "This session is no longer available on the server.");
    }
  }
  for (const view of views) {
    const ui = sessions.get(String(view.id));
    if (!ui) { addSession(view); continue; }
    const follow = ui.follow;
    const position = ui.trace.scrollTop;
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
    else ui.trace.scrollTop = position;
  }
}

function startEvents() {
  events?.close();
  events = new EventSource("/api/events");
  events.onopen = () => { connection.textContent = "Connected"; };
  events.onerror = () => {
    connection.textContent = "Disconnected · reconnecting…";
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
  const close = element("button", null, "Close");
  close.type = "button";
  close.title = "End this terminal's shell session.";
  ui.header.append(close);
  const host = element("div", "terminal-host");
  ui.pane.append(host, ui.error);
  const term = new Terminal({
    cursorBlink: true,
    disableStdin: true,
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    fontSize: 12,
    scrollback: 5000,
    theme: { background: "#1e1e1e", foreground: "#d7d7d7", cursor: "#d7d7d7" },
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(host);
  let socket;
  let closed = false;
  let retry;
  function send(message) {
    if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  }
  function resize() {
    const size = fit.proposeDimensions();
    if (size) term.resize(Math.max(1, Math.min(500, size.cols)), Math.max(1, Math.min(500, size.rows)));
  }
  function connect() {
    if (closed) return;
    clearTimeout(retry);
    if (socket) { socket.onclose = null; socket.close(); }
    ui.status.textContent = "connecting";
    term.options.disableStdin = true;
    const scheme = location.protocol === "https:" ? "wss:" : "ws:";
    socket = new WebSocket(`${scheme}//${location.host}/api/terminals/${encodeURIComponent(id)}/ws`);
    socket.binaryType = "arraybuffer";
    socket.onopen = () => {
      term.reset();
      term.options.disableStdin = false;
      ui.status.textContent = "connected";
      ui.error.hidden = true;
      resize();
      send({ type: "resize", cols: term.cols, rows: term.rows });
    };
    socket.onmessage = event => {
      if (event.data instanceof ArrayBuffer) term.write(new Uint8Array(event.data));
      else term.write(event.data);
    };
    socket.onclose = async () => {
      if (closed) return;
      term.options.disableStdin = true;
      try {
        const list = await request("/api/terminals");
        const current = list.find(item => String(item.id) === id);
        if (!current || !current.running) { ui.status.textContent = "exited"; return; }
      } catch (error) {
        if (error.status === 401) { ui.status.textContent = "disconnected"; requireLogin(); return; }
      }
      ui.status.textContent = "reconnecting";
      retry = setTimeout(connect, 2000);
    };
  }
  term.onData(data => send({ type: "input", data }));
  term.onResize(({ cols, rows }) => send({ type: "resize", cols, rows }));
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
      term.dispose();
      if (focused === ui.pane) focused = null;
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
  connection.textContent = "Authentication required";
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
