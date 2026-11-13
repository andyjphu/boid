// Two real browser windows, one mocked server; no model requests or shell input.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const view = id => ({ id, name: id, cwd: '/tmp', status: 'idle', pending: [], entries:
  Array.from({ length: 80 }, (_, i) => ({ id: `${id}-${i}`, kind: 'assistant', text: `Message ${i}: ` + 'text '.repeat(70) })) });
(async () => {
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  try {
    const context = await browser.newContext({ viewport: { width: 1200, height: 900 } });
    const pages = [], errors = [], mutations = [];
    const screens = { revision: 0, count: 1, panes: {} };
    const agents = [view('a'), view('b')];
    const terms = [{ id: 't', name: '#1', cwd: '/tmp', running: true }];
    let sequence = 0;
    let holdAgent = false, holdTerminal = false;
    let releaseAgent, releaseTerminal, releaseMessage;
    const until = async check => { for (let i = 0; !check(); i++) { assert.ok(i < 300, 'deferred request arrived'); await new Promise(resolve => setTimeout(resolve, 10)); } };
    const emit = async message => {
      message.seq = ++sequence;
      for (const page of pages) await page.evaluate(message => testEvents?.onmessage?.({ data: JSON.stringify(message) }), message);
    };
    const snapshot = async page => {
      await page.evaluate(({ screens, agents, terms, seq }) => {
        const screen = Number(new URL(testEvents.url, location.origin).searchParams.get('screen'));
        testEvents.onmessage({ data: JSON.stringify({ type: 'snapshot', seq, screens, catalog: agents.map(a => a.id),
          sessions: agents.filter(a => (screens.panes[`agent:${a.id}`] || 1) === screen), terminals: terms }) });
      }, { screens, agents, terms, seq: ++sequence });
    };
    await context.route('**/*', async route => {
      const url = new URL(route.request().url()), method = route.request().method();
      if (url.hostname !== 'boid.test') return route.abort();
      if (method !== 'GET') mutations.push({ path: url.pathname, method });
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', next_agent_name: 'swift', agent_names: [] } });
      if (url.pathname === '/api/screens') {
        if (method === 'POST') { screens.count++; screens.revision++; await emit({ type: 'screens', screens }); }
        return route.fulfill({ json: screens });
      }
      if (url.pathname === '/api/screens/move') {
        const { pane, screen } = route.request().postDataJSON(); screens.panes[pane] = screen; screens.revision++;
        await emit({ type: 'screens', screens }); return route.fulfill({ json: screens });
      }
      if (url.pathname.endsWith('/messages') && method === 'POST') {
        await new Promise(resolve => { releaseMessage = resolve; });
        return route.fulfill({ json: { status: 'queued' } });
      }
      if (url.pathname === '/api/sessions' && method === 'POST') {
        const fields = route.request().postDataJSON(); const a = view(fields.name || 'created'); agents.push(a);
        screens.panes[`agent:${a.id}`] = Number(url.searchParams.get('screen') || 1);
        screens.revision++;
        await emit({ type: 'session', screens_revision: screens.revision, session: { ...a, screen: screens.panes[`agent:${a.id}`] } });
        if (holdAgent) await new Promise(resolve => { releaseAgent = resolve; });
        return route.fulfill({ json: a });
      }
      if (url.pathname === '/api/terminals' && method === 'POST') {
        const term = { id: terms.length === 1 ? 'second-terminal' : 'late-terminal', name: '#2', cwd: '/tmp', running: true }; terms.push(term);
        screens.panes[`terminal:${term.id}`] = Number(url.searchParams.get('screen') || 1);
        screens.revision++;
        await emit({ type: 'terminal', screens_revision: screens.revision, terminal: { ...term, screen: screens.panes[`terminal:${term.id}`] } });
        if (holdTerminal) await new Promise(resolve => { releaseTerminal = resolve; });
        return route.fulfill({ json: term });
      }
      if (url.pathname === '/api/sessions') return route.fulfill({ json: agents });
      if (/\/api\/sessions\/[^/]+\/history/.test(url.pathname)) return route.fulfill({ json: agents.find(a => url.pathname.includes(`/${a.id}/`)) });
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    await context.exposeBinding('subscribeScreen', async ({ page }) => { await snapshot(page); });
    await context.addInitScript(() => {
      window.EventSource = class {
        constructor(url) { this.url = url; window.testEvents = this; setTimeout(() => { this.onopen?.(); window.subscribeScreen(); }, 0); }
        close() { this.closed = true; }
      };
      window.testSockets = [];
      window.WebSocket = class {
        static OPEN = 1;
        constructor(url) { this.url = url; this.readyState = 1; testSockets.push(this); }
        close() { this.readyState = 3; }
        send() {}
      };
    });
    const page = await context.newPage(); pages.push(page); page.on('pageerror', e => errors.push(e.message));
    const frames = p => p.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const ready = async (p, screen) => { await p.waitForFunction(n => catalogReady && activeScreen === n, screen); await frames(p); };
    const keys = p => p.locator('.pane').evaluateAll(nodes => nodes.map(n => n.dataset.paneKey).sort());
    await page.goto('http://boid.test'); await ready(page, 1);
    assert.deepEqual(await keys(page), ['agent:a', 'agent:b', 'terminal:t']);
    await page.locator('.agent-pane').first().locator('.composer textarea').fill('Unsent on screen one');
    await page.evaluate(() => {
      const ui = sessions.get('a'); ui.follow = false; scrollTrace(ui, 200); rememberPosition(ui);
    });
    // Resize a boundary before leaving; the same layout must return.
    const divider = page.locator('.pane-resize-vertical').first(); await divider.focus(); await page.keyboard.press('ArrowLeft'); await frames(page);
    const anchor = await page.evaluate(() => { const ui = sessions.get('a'); rememberPosition(ui); return { id: ui.anchor.node.dataset.entryId, offset: ui.anchor.offset }; });
    const width = await page.locator('[data-pane-key="agent:a"]').evaluate(n => n.getBoundingClientRect().width);
    await page.locator('#add-screen').click(); await ready(page, 2);
    assert.deepEqual(await keys(page), []);
    assert.equal(await page.evaluate(() => testSockets.filter(s => s.readyState === 1).length), 0, 'inactive terminals disconnected');
    assert.equal(await page.locator('#screens a').count(), 2);
    assert.equal(await page.locator('#screens a[aria-current]').innerText(), '2');
    assert.equal(await page.evaluate(() => sessions.size), 0, 'no hidden agent UI retained');
    // Off-screen traffic must not recreate panes or transcripts.
    await emit({ type: 'delta', session_id: 'a', entry_id: 'a-79', text: 'hidden output' });
    await frames(page); assert.deepEqual(await keys(page), []);
    await page.locator('#add-agent').click();
    await page.locator('.setup-form input[name=name]').fill('second');
    await page.locator('.setup-submit').click(); await page.waitForSelector('[data-pane-key="agent:second"]');
    assert.equal(screens.panes['agent:second'], 2);
    await page.locator('#add-terminal').click(); await page.waitForSelector('[data-pane-key="terminal:second-terminal"]');
    assert.equal(screens.panes['terminal:second-terminal'], 2);
    const other = await context.newPage(); pages.push(other); other.on('pageerror', e => errors.push(e.message));
    await other.goto('http://boid.test/?screen=1'); await ready(other, 1);
    assert.deepEqual(await keys(other), ['agent:a', 'agent:b', 'terminal:t']);
    assert.deepEqual(await keys(page), ['agent:second', 'terminal:second-terminal']);
    // The selected screen is not shared across windows, but membership is.
    await other.locator('#add-screen').click(); await ready(other, 3);
    await page.waitForFunction(() => screenCatalog.count === 3);
    assert.equal(await page.evaluate(() => activeScreen), 2);
    await page.evaluate(() => receiveScreens({ revision: 0, count: 1, panes: {} }));
    assert.equal(await page.evaluate(() => screenCatalog.count), 3, 'stale responses do not revert shared screens');
    await page.locator('#screens [data-screen="1"]').click(); await ready(page, 1);
    assert.equal(await page.locator('[data-pane-key="agent:a"] .composer textarea').inputValue(), 'Unsent on screen one');
    assert.ok(Math.abs(await page.locator('[data-pane-key="agent:a"]').evaluate(n => n.getBoundingClientRect().width) - width) < 2, 'per-screen size retained');
    const restored = await page.evaluate(id => { const ui = sessions.get('a'); return { follow: ui.follow, offset: ui.entries.get(id).node.getBoundingClientRect().top - ui.trace.getBoundingClientRect().top }; }, anchor.id);
    assert.equal(restored.follow, false); assert.ok(Math.abs(restored.offset - anchor.offset) < 3, 'reading anchor retained');
    // Unsaved setup forms are local to each screen and survive refresh.
    await page.locator('#add-agent').click(); await page.locator('.setup-form input[name=name]').fill('draft-one');
    await page.locator('#screens [data-screen="2"]').click(); await ready(page, 2);
    assert.equal(await page.locator('.setup-pane').count(), 0);
    await page.reload(); await ready(page, 2);
    assert.deepEqual(await keys(page), ['agent:second', 'terminal:second-terminal']);
    await page.locator('#screens [data-screen="1"]').click(); await ready(page, 1);
    assert.equal(await page.locator('.setup-form input[name=name]').inputValue(), 'draft-one');
    await page.locator('[aria-label="Discard agent draft"]').click();
    await page.locator('[data-pane-key="agent:b"] .composer textarea').fill('Unsent while moving');
    await page.locator('[data-pane-key="agent:b"] .pane-tab').dragTo(page.locator('#screens [data-screen="3"]'));
    await page.waitForFunction(() => !sessions.has('b'));
    await other.waitForFunction(() => sessions.has('b'));
    assert.equal(await page.evaluate(() => activeScreen), 1);
    assert.equal(await other.evaluate(() => activeScreen), 3);
    assert.equal(screens.panes['agent:b'], 3);
    await page.locator('#screens [data-screen="3"]').click(); await ready(page, 3);
    assert.equal(await page.locator('[data-pane-key="agent:b"] .composer textarea').inputValue(), 'Unsent while moving');
    await page.locator('#screens [data-screen="1"]').click(); await ready(page, 1);
    const peer = { id: 'b-0', sender_id: 'a', recipient_id: 'b', sender_name: 'a', recipient_name: 'b', sender_entry: 'a-0', text: 'Cross-screen message', state: 'delivered' };
    agents[0].communications = [peer]; agents[1].communications = [peer];
    await emit({ type: 'communication', message: peer });
    await page.getByRole('button', {name: 'Find this message in b', exact: true}).click();
    await ready(page, 3);
    await page.waitForFunction(() => document.activeElement?.dataset.entryId === 'b-0');
    await page.setViewportSize({ width: 390, height: 700 }); await frames(page);
    const overlap = await page.evaluate(() => document.querySelector('#screens').getBoundingClientRect().right > document.querySelector('.workspace-actions').getBoundingClientRect().left);
    assert.equal(overlap, false, 'screen strip does not cover existing controls');
    assert.ok(mutations.every(m => !m.path.endsWith('/messages') && m.method !== 'DELETE'), 'switching/moving sends no input or lifecycle operations');
    await page.setViewportSize({ width: 1200, height: 900 });
    // An accepted message must clear the remounted composer, but preserve newer edits.
    for (const edit of [false, true]) {
      releaseMessage = null;
      const text = edit ? 'Second accepted message' : 'First accepted message';
      await page.locator('[data-pane-key="agent:b"] .composer textarea').fill(text);
      await page.locator('[data-pane-key="agent:b"] .composer textarea').press('Enter');
      await until(() => releaseMessage);
      await page.locator('#screens [data-screen="1"]').click(); await ready(page, 1);
      await page.locator('#screens [data-screen="3"]').click(); await ready(page, 3);
      assert.equal(await page.locator('[data-pane-key="agent:b"] .composer textarea').inputValue(), text);
      if (edit) await page.locator('[data-pane-key="agent:b"] .composer textarea').fill('Newer edit');
      releaseMessage();
      if (!edit) await page.waitForFunction(() => sessions.get('b').input.value === '');
      else { await frames(page); assert.equal(await page.locator('[data-pane-key="agent:b"] .composer textarea').inputValue(), 'Newer edit'); }
    }
    // A move missed while disconnected arrives only in the reconnect snapshot.
    await page.locator('[data-pane-key="agent:b"] .composer textarea').fill('Typed since the last save');
    screens.panes['agent:b'] = 1; screens.revision++;
    await snapshot(page); await frames(page);
    assert.equal(await page.locator('[data-pane-key="agent:b"]').count(), 0);
    await page.locator('#screens [data-screen="1"]').click(); await ready(page, 1);
    assert.equal(await page.locator('[data-pane-key="agent:b"] .composer textarea').inputValue(), 'Typed since the last save');
    // A newer cross-window move wins over a delayed create response for either pane kind.
    holdAgent = true;
    await page.locator('#add-agent').click(); await page.locator('.setup-form input[name=name]').fill('late-agent');
    await page.locator('.setup-submit').click(); await until(() => releaseAgent);
    screens.panes['agent:late-agent'] = 2; screens.revision++;
    await emit({ type: 'screens', screens });
    await page.waitForFunction(() => !sessions.has('late-agent'));
    releaseAgent(); holdAgent = false;
    await page.waitForFunction(() => drafts.size === 0); await frames(page);
    assert.equal(await page.evaluate(() => paneScreen('agent:late-agent')), 2);
    assert.equal(await page.locator('[data-pane-key="agent:late-agent"]').count(), 0);
    holdTerminal = true;
    await page.locator('#add-terminal').click(); await until(() => releaseTerminal);
    screens.panes['terminal:late-terminal'] = 2; screens.revision++;
    await emit({ type: 'screens', screens });
    await page.waitForFunction(() => !terminals.has('late-terminal'));
    releaseTerminal(); holdTerminal = false;
    await page.waitForFunction(() => !document.querySelector('#add-terminal').disabled);
    assert.equal(await page.evaluate(() => paneScreen('terminal:late-terminal')), 2);
    assert.equal(await page.locator('[data-pane-key="terminal:late-terminal"]').count(), 0);
    assert.deepEqual(errors, []);
    if (process.env.SCREENSHOT) { await page.setViewportSize({ width: 1200, height: 900 }); await frames(page); await page.screenshot({ path: process.env.SCREENSHOT }); }
    console.log('PASS: independent windows, shared screen membership, creation, drag move, inactive DOM/socket release, per-screen drafts/layout/reading position, refresh, narrow controls, delayed response and reconnect races');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exit(1); });
