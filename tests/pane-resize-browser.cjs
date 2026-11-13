// Exercise the shipped UI with real pointer gestures. Provider/PTY traffic is mocked.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const views = ['a', 'b', 'c', 'd'].map(id => ({
  id, name: id, cwd: '/tmp', status: 'idle', pending: [],
  entries: Array.from({ length: 80 }, (_, i) => ({ id: `${id}-${i}`, kind: 'assistant', text: `Message ${i}: ` + 'text '.repeat(50) })),
}));
const near = (a, b, label) => assert.ok(Math.abs(a - b) < 2, `${label}: ${a} vs ${b}`);
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
        send(data) {
          const message = JSON.parse(data); this.sent.push(message);
          if (message.type === 'resize') this.onmessage({ data });
        }
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
    const snapshot = async (agents = views, terminals = []) => {
      await page.evaluate(([a, t]) => window.testEvents.onmessage({ data: JSON.stringify({ type: 'snapshot', seq: 1, sessions: a, terminals: t }) }), [agents, terminals]);
      await frames();
    };
    const boxes = () => page.locator('.pane').evaluateAll(nodes => Object.fromEntries(nodes.map(node => [node.dataset.paneKey, node.getBoundingClientRect().toJSON()])));
    const handle = (key, edge) => page.locator(`[data-pane-key="${key}"] > .pane-resize-${edge}`);
    async function beginDrag(key, edge) {
      const box = await handle(key, edge).boundingBox();
      assert.ok(box, `${key} ${edge} is draggable`);
      const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
      await page.mouse.move(point.x, point.y); await page.mouse.down();
      return point;
    }
    async function drag(key, edge, delta) {
      const p = await beginDrag(key, edge);
      await page.mouse.move(p.x + (['left', 'right'].includes(edge) ? delta : 0), p.y + (['top', 'bottom'].includes(edge) ? delta : 0), { steps: 8 });
      await page.mouse.up(); await frames();
    }
    async function reset(agents = views, terminals = []) {
      await page.evaluate(() => { paneLayout = null; }); await snapshot(agents, terminals);
    }
    async function noOverlap() {
      const values = Object.values(await boxes());
      for (let i = 0; i < values.length; i++) {
        assert.ok(values[i].width >= 179 && values[i].height >= 119, 'minimum usable pane size');
        for (const b of values.slice(i + 1)) {
          const a = values[i];
          assert.ok(Math.min(a.right, b.right) - Math.max(a.left, b.left) < 1 || Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) < 1, 'panes must not overlap');
        }
      }
    }
    await page.goto('http://boid.test'); await page.waitForFunction(() => window.testEvents);
    // Restored drafts are interactive before the first catalog establishes a layout.
    await page.evaluate(() => sessionStorage.setItem(orderKey, JSON.stringify({
      version: 2, order: ['draft:saved'], layout: null,
      drafts: [{ id: 'saved', name: '', cwd: '' }],
    })));
    await page.reload(); await page.waitForFunction(() => window.testEvents);
    assert.equal(await page.evaluate(() => paneLayout), null);
    await handle('draft:saved', 'right').focus(); await page.keyboard.press('ArrowRight');
    await drag('draft:saved', 'right', 20);
    assert.deepEqual(errors, [], 'restored draft resize before the first snapshot must be harmless');
    await page.evaluate(() => discardSetup(drafts.get('saved')));
    await snapshot();
    const original = await boxes();
    await drag('agent:a', 'right', 120);
    let changed = await boxes();
    near(changed['agent:a'].width, original['agent:a'].width + 120, 'right drag expands');
    near(changed['agent:b'].width, original['agent:b'].width - 120, 'neighbor shrinks');
    near(changed['agent:c'].width, original['agent:c'].width, 'other row stays put');
    await drag('agent:b', 'left', -70);
    near((await boxes())['agent:a'].width, original['agent:a'].width + 50, 'left drag reverses');
    const saved = await boxes();
    await snapshot();
    near((await boxes())['agent:a'].width, saved['agent:a'].width, 'reconnect retains sizes');
    await page.reload(); await page.waitForFunction(() => window.testEvents); await snapshot();
    near((await boxes())['agent:a'].width, saved['agent:a'].width, 'refresh retains sizes');
    await reset();
    await drag('agent:a', 'bottom', 90);
    changed = await boxes();
    near(changed['agent:a'].height, original['agent:a'].height + 90, 'bottom drag expands');
    near(changed['agent:c'].height, original['agent:c'].height - 90, 'lower neighbor shrinks');
    near(changed['agent:b'].height, original['agent:b'].height, 'other column stays put');
    await drag('agent:c', 'top', -40);
    near((await boxes())['agent:a'].height, original['agent:a'].height + 50, 'top drag reverses');
    // Staggered junctions must remain rectangular and joined, including extreme drags.
    await drag('agent:a', 'right', 80); await noOverlap();
    await drag('agent:a', 'right', 3000); await noOverlap();
    near((await boxes())['agent:b'].width, 180, 'width clamp');
    await drag('agent:a', 'bottom', -3000); await noOverlap();
    near((await boxes())['agent:a'].height, 120, 'height clamp');
    await reset();
    const beforeCancel = await boxes();
    let point = await beginDrag('agent:a', 'right');
    await page.mouse.move(point.x + 100, point.y); await page.keyboard.press('Escape'); await page.mouse.up(); await frames();
    near((await boxes())['agent:a'].width, beforeCancel['agent:a'].width, 'Escape cancels resize');
    assert.equal(await page.locator('body').evaluate(node => node.classList.contains('resizing-panes')), false);
    point = await beginDrag('agent:a', 'bottom');
    await page.mouse.move(point.x, point.y + 80);
    await handle('agent:a', 'bottom').dispatchEvent('pointercancel'); await page.mouse.up(); await frames();
    near((await boxes())['agent:a'].height, beforeCancel['agent:a'].height, 'pointer cancellation restores');
    await handle('agent:a', 'right').focus(); await page.keyboard.press('ArrowRight'); await frames();
    near((await boxes())['agent:a'].width, beforeCancel['agent:a'].width + 10, 'keyboard resize');
    // Reading and following retain their distinct behavior while text rewraps.
    const anchor = await page.evaluate(() => {
      const ui = sessions.get('a'); ui.follow = false; scrollTrace(ui, ui.trace.scrollTop - 300); rememberPosition(ui);
      return { id: ui.anchor.node.dataset.entryId, offset: ui.anchor.offset };
    });
    await drag('agent:a', 'right', 100);
    const reading = await page.evaluate(id => {
      const ui = sessions.get('a'); return { follow: ui.follow, offset: ui.entries.get(id).node.getBoundingClientRect().top - ui.trace.getBoundingClientRect().top };
    }, anchor.id);
    assert.equal(reading.follow, false); near(reading.offset, anchor.offset, 'reading anchor survives rewrap');
    await page.evaluate(() => { const ui = sessions.get('a'); ui.follow = true; restorePosition(ui); });
    await drag('agent:a', 'bottom', 60);
    assert.ok(await page.evaluate(() => { const ui = sessions.get('a'); return ui.follow && atBottom(ui); }), 'following survives resize');
    await page.setViewportSize({ width: 900, height: 700 }); await frames(); await noOverlap();
    await page.setViewportSize({ width: 390, height: 700 }); await frames(); await noOverlap();
    await page.setViewportSize({ width: 1200, height: 900 }); await frames();
    // An incomplete grid must also stop before a previously separated rectangle.
    await reset(views.slice(0, 3));
    await drag('agent:b', 'bottom', -140); await drag('agent:c', 'right', 100);
    await drag('agent:a', 'bottom', -3000); await noOverlap();
    await reset();
    await drag('agent:a', 'right', 80);
    await snapshot(views.slice(0, 3)); await noOverlap();
    await snapshot(views); await noOverlap();
    // Resizing in a workspace taller than the viewport must not collapse other rows.
    await page.setViewportSize({ width: 1200, height: 400 }); await reset();
    const overflowing = await boxes();
    await drag('agent:a', 'right', 50);
    near((await boxes())['agent:c'].height, overflowing['agent:c'].height, 'overflowing row height retained');
    await page.setViewportSize({ width: 1200, height: 900 }); await frames();
    // The existing xterm fit/PTY resize path receives updated dimensions.
    const terminal = { id: 't', name: '#1', cwd: '/tmp', running: true };
    await reset([views[0]], [terminal]);
    await page.evaluate(() => {
      const socket = testSockets.at(-1); socket.onopen();
      for (const message of [{ type: 'hello', id: 'test' }, { type: 'replay_end' }, { type: 'control', id: 'test' }]) socket.onmessage({ data: JSON.stringify(message) });
    });
    await frames();
    const firstSize = await page.evaluate(() => testSockets.at(-1).sent.filter(m => m.type === 'resize').at(-1));
    assert.ok(firstSize, 'terminal initial fit');
    await drag('terminal:t', 'left', -100); await frames();
    const lastSize = await page.evaluate(() => testSockets.at(-1).sent.filter(m => m.type === 'resize').at(-1));
    assert.ok(lastSize.cols > firstSize.cols, 'terminal columns grow with pane');
    assert.equal(await page.evaluate(() => testSockets.at(-1).sent.some(m => m.type === 'input')), false, 'resize sends no terminal input');
    // Corrupt or obsolete saved layouts recover to the normal grid.
    await page.evaluate(() => { const saved = JSON.parse(sessionStorage.getItem(orderKey)); saved.layout.tiles[0].right = 'bad'; sessionStorage.setItem(orderKey, JSON.stringify(saved)); });
    await page.reload(); await page.waitForFunction(() => window.testEvents); await snapshot(); await noOverlap();
    await drag('agent:a', 'right', 120);
    if (process.env.BOID_RESIZE_SCREENSHOT) await page.screenshot({ path: process.env.BOID_RESIZE_SCREENSHOT });
    assert.deepEqual(mutations, [], 'resizing must not change agent execution');
    assert.deepEqual(errors, []);
    console.log('PASS: all four edges, independent panes, neighbor constraints, min sizes, refresh/reconnect, cancel, keyboard, scroll anchors, viewport resize, add/remove, incomplete grid, terminal fit, corrupt storage');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exit(1); });
