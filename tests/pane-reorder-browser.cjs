// Real native drag gestures against the shipped UI. Model and PTY traffic is mocked.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const views = ['a', 'b'].map(id => ({ id, name: id, cwd: '/tmp', status: id === 'b' ? 'working' : 'idle', pending: [],
  entries: Array.from({ length: 40 }, (_, i) => ({ id: `${id}-${i}`, kind: 'assistant', text: `Message ${i}\n` + 'Existing output\n'.repeat(3) })) }));
const terms = [{ id: 't', name: '#1', cwd: '/tmp', running: true }];
(async () => {
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
    const errors = [], mutations = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.EventSource = class { constructor() { window.testEvents = this; } close() {} };
      window.testSockets = [];
      window.WebSocket = class {
        static OPEN = 1;
        constructor() { this.readyState = 1; this.sent = []; testSockets.push(this); }
        close() { this.readyState = 3; }
        send(data) { this.sent.push(JSON.parse(data)); if (JSON.parse(data).type === 'resize') this.onmessage?.({ data }); }
      };
    });
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (route.request().method() !== 'GET') mutations.push(url.pathname);
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', next_agent_name: 'swift', agent_names: [] } });
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    const frames = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const snapshot = async () => {
      await page.waitForFunction(() => window.testEvents?.onmessage);
      await page.evaluate(([sessions, terminals]) => testEvents.onmessage({ data: JSON.stringify({ type: 'snapshot', seq: 1, sessions, terminals }) }), [views, terms]);
      await frames();
    };
    const pane = key => page.locator(`.pane[data-pane-key="${key}"]`);
    const keys = () => page.locator('.pane').evaluateAll(nodes => nodes.map(n => n.dataset.paneKey));
    async function drag(source, target, after) {
      const box = await pane(target).boundingBox();
      await pane(source).locator('.pane-tab').dragTo(pane(target), { targetPosition: { x: box.width * (after ? .8 : .2), y: box.height * .4 } });
      await frames();
    }
    async function beginDrag(source, target) {
      const from = await pane(source).locator('.pane-tab').boundingBox();
      const to = await pane(target).boundingBox();
      await page.mouse.move(from.x + 15, from.y + from.height / 2);
      await page.mouse.down();
      await page.mouse.move(from.x + 30, from.y + from.height / 2, { steps: 3 });
      await page.mouse.move(to.x + to.width * .2, to.y + to.height * .4, { steps: 8 });
      await page.mouse.move(to.x + to.width * .21, to.y + to.height * .4);
      await page.waitForSelector('[data-pane-drop]');
    }
    await page.goto('http://boid.test'); await snapshot();
    await pane('agent:b').locator('textarea').fill('Keep this unsent instruction');
    await page.locator('#add-agent').click();
    await page.locator('.setup-form input[name=name]').fill('draft-name');
    const draft = await page.locator('.setup-pane').getAttribute('data-pane-key');
    const original = await keys();
    assert.deepEqual(original, ['terminal:t', 'agent:a', 'agent:b', draft]);
    await page.evaluate(() => {
      window.retainedPanes = new Map([...document.querySelectorAll('.pane')].map(p => [p.dataset.paneKey, p]));
      const ui = sessions.get('b'); ui.follow = false; scrollTrace(ui, 120); rememberPosition(ui);
      window.retainedAnchor = { id: ui.anchor.node.dataset.entryId, offset: ui.anchor.offset };
    });
    await beginDrag('agent:b', 'agent:a');
    assert.equal(await pane('agent:a').getAttribute('data-pane-drop'), 'left');
    if (process.env.SCREENSHOT) await page.screenshot({ path: process.env.SCREENSHOT.replace(/\.png$/, '-marker.png') });
    await page.mouse.up(); await frames();
    assert.deepEqual(await keys(), ['terminal:t', 'agent:b', 'agent:a', draft]);
    await drag('terminal:t', 'agent:a', true);
    assert.deepEqual(await keys(), ['agent:b', 'agent:a', 'terminal:t', draft]);
    await drag(draft, 'agent:b', true);
    assert.deepEqual(await keys(), ['agent:b', draft, 'agent:a', 'terminal:t']);
    assert.equal(await pane('agent:b').locator('textarea').inputValue(), 'Keep this unsent instruction');
    assert.equal(await page.locator('.setup-form input[name=name]').inputValue(), 'draft-name');
    assert.ok(await page.evaluate(() => [...retainedPanes].every(([key, node]) => node === document.querySelector(`[data-pane-key="${key}"]`))), 'reordering retains the original pane nodes');
    assert.equal(await page.evaluate(() => testSockets.length), 1, 'reordering does not reconnect the terminal');
    const position = await page.evaluate(() => { const ui = sessions.get('b'); const item = ui.entries.get(retainedAnchor.id); return { follow: ui.follow, offset: item.node.getBoundingClientRect().top - ui.trace.getBoundingClientRect().top, expected: retainedAnchor.offset }; });
    console.log('reading-position', position);
    assert.equal(position.follow, false); assert.ok(Math.abs(position.offset - position.expected) < 3, 'reading anchor retained');
    // Moving into a custom layout changes occupants, not the resized rectangles.
    await page.locator('.pane-resize-vertical').first().focus(); await page.keyboard.press('ArrowRight'); await frames();
    const slots = await page.evaluate(() => [...paneOrder].map(key => { const { key: _, ...rect } = paneLayout.tiles.find(tile => tile.key === key); return rect; }));
    await drag('terminal:t', 'agent:b', false);
    const reordered = ['terminal:t', 'agent:b', draft, 'agent:a'];
    assert.deepEqual(await keys(), reordered);
    const afterSlots = await page.evaluate(() => [...paneOrder].map(key => { const { key: _, ...rect } = paneLayout.tiles.find(tile => tile.key === key); return rect; }));
    assert.deepEqual(afterSlots, slots, 'custom rectangle positions and sizes are unchanged');
    // Escape during a real native drag must not turn into an agent interruption.
    await beginDrag('agent:b', 'terminal:t');
    await page.keyboard.press('Escape'); await page.mouse.up(); await frames();
    assert.deepEqual(await keys(), reordered);
    assert.equal(await page.locator('[data-pane-drop]').count(), 0);
    assert.equal(await page.evaluate(() => paneDrag), null);
    await beginDrag('agent:b', 'terminal:t');
    await page.mouse.move(350, 2, { steps: 8 }); await page.mouse.up(); await frames();
    assert.deepEqual(await keys(), reordered, 'dropping outside panes cancels');
    assert.deepEqual(mutations, [], 'local reordering never creates, ends, sends to, or interrupts a session');
    await page.reload(); await snapshot();
    assert.deepEqual(await keys(), reordered, 'refresh restores the chosen order');
    assert.equal(await pane('agent:b').locator('textarea').inputValue(), 'Keep this unsent instruction');
    assert.equal(await page.locator('.setup-form input[name=name]').inputValue(), 'draft-name');
    assert.deepEqual(await page.evaluate(() => [...paneOrder].map(key => { const { key: _, ...rect } = paneLayout.tiles.find(tile => tile.key === key); return rect; })), slots);
    assert.deepEqual(errors, []);
    if (process.env.SCREENSHOT) await page.screenshot({ path: process.env.SCREENSHOT });
    console.log('PASS: agent, terminal and draft reordering; insertion marker; retained text/nodes/terminal connection/reading anchor; resized slots; Escape/outside cancellation; refresh; no lifecycle or input requests');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
