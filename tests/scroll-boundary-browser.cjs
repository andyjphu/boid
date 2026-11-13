// Real wheel input and synthetic streaming. No live agent requests.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright');
const root = process.env.BOID_WEB_ROOT || path.resolve(__dirname, '../src/web');
(async () => {
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
    const errors = [], results = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.EventSource = class { constructor() { window.testEvents = this; } close() {} };
    });
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', agent_names: [] } });
      if (url.pathname === '/api/sessions/a/history' && url.searchParams.get('after') === '300') {
        return route.fulfill({ json: { id: 'a', entries: [{ id: '300', kind: 'assistant', text: 'Next history page' }],
          history: { start: 300, end: 301, total: 301, seq: 2 } } });
      }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js')
        ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    await page.goto('http://boid.test');
    await page.waitForFunction(() => window.testEvents);
    await page.evaluate(() => {
      window.moves = [];
      const original = scrollTrace;
      scrollTrace = (ui, top) => {
        const before = ui.trace.scrollTop;
        original(ui, top);
        const after = ui.trace.scrollTop;
        if (Math.abs(after - before) > 0.5) moves.push({ before, after });
      };
      window.inspectScroll = () => {
        const ui = sessions.get('a');
        return { top: ui.trace.scrollTop, end: ui.trace.scrollHeight - ui.trace.clientHeight, follow: ui.follow };
      };
    });
    for (const history of [false, true]) for (const zoom of [1, 1.1, 1.25]) {
      await page.evaluate(({ history, zoom }) => {
        for (const ui of [...sessions.values()]) removeSession(ui);
        document.body.style.zoom = zoom;
        const entries = Array.from({ length: 300 }, (_, i) => ({ id: String(i), kind: 'assistant', text: `Line ${i}` }));
        snapshot([{ id: 'a', name: 'a', cwd: '/tmp', status: 'working', pending: [], entries,
          ...(history ? { history: { start: 0, end: 300, total: 300, seq: 1 } } : {}) }], []);
      }, { history, zoom });
      await page.locator('.trace').hover();
      await page.waitForTimeout(200);
      await page.evaluate(() => {
        moves.length = 0;
        window.streamTimer = setInterval(() => putEntry(sessions.get('a'), { id: '299', text: '.' }, true), 8);
      });
      try {
        for (let i = 0; i < 30; i++) { await page.mouse.wheel(0, 300); await page.waitForTimeout(10); }
      } finally { await page.evaluate(() => clearInterval(streamTimer)); }
      await page.waitForTimeout(150);
      const result = { history, zoom, ...await page.evaluate(() => ({
        state: inspectScroll(), upwardCorrections: moves.filter(move => move.after < move.before).length,
      })) };
      results.push(result);
      assert.ok(Math.abs(result.state.top - result.state.end) <= 1, JSON.stringify(result));
      assert.equal(result.upwardCorrections, 0, `Downward wheel fought following: ${JSON.stringify(result)}`);

      // Small reverse gestures must leave follow mode immediately.
      const before = await page.evaluate(() => inspectScroll());
      await page.mouse.wheel(0, -4);
      await page.waitForTimeout(150);
      const reading = await page.evaluate(() => inspectScroll());
      assert.equal(reading.follow, false);
      assert.ok(reading.top < before.top - 1, 'Small upward wheel remained stuck');
      await page.mouse.wheel(0, 100000);
      await page.waitForTimeout(150);
      const resumed = await page.evaluate(() => inspectScroll());
      assert.equal(resumed.follow, true, JSON.stringify({ history, zoom, before, reading, resumed }));
    }

    // Edge events must be consumed before a browser can overscroll. Interior
    // movement and zoom/horizontal gestures retain their native defaults.
    const defaults = await page.evaluate(() => {
      const ui = sessions.get('a');
      const wheel = args => {
        const event = new WheelEvent('wheel', { bubbles: true, cancelable: true, ...args });
        ui.trace.dispatchEvent(event);
        return event.defaultPrevented;
      };
      const bottom = wheel({ deltaY: 500 });
      const zoom = wheel({ deltaY: 500, ctrlKey: true });
      const horizontal = wheel({ deltaX: 500 });
      ui.follow = false;
      ui.trace.scrollTop = 500;
      const interior = wheel({ deltaY: -4 });
      const topCrossing = wheel({ deltaY: -100000, deltaMode: WheelEvent.DOM_DELTA_LINE });
      const top = ui.trace.scrollTop;
      const topOutward = wheel({ deltaY: -50 });
      const pageCrossing = wheel({ deltaY: 1000, deltaMode: WheelEvent.DOM_DELTA_PAGE });
      return { bottom, zoom, horizontal, interior, topCrossing, top, topOutward, pageCrossing };
    });
    assert.deepEqual(defaults, { bottom: true, zoom: false, horizontal: false, interior: false,
      topCrossing: true, top: 0, topOutward: true, pageCrossing: true });
    // Consuming the native event must still load newer history at the boundary.
    await page.evaluate(() => {
      const ui = sessions.get('a');
      ui.follow = false;
      ui.history.total = 301;
      ui.trace.dispatchEvent(new WheelEvent('wheel', { deltaY: 100000, bubbles: true, cancelable: true }));
    });
    await page.waitForFunction(() => sessions.get('a').history.end === 301);
    await page.waitForTimeout(150);
    await page.evaluate(() => { document.body.style.zoom = 1; window.scrollTo(0, 0); layout(); restorePosition(sessions.get("a")); });
    await page.waitForTimeout(150);
    if (process.env.BOID_SCREENSHOT) await page.screenshot({ path: process.env.BOID_SCREENSHOT });
    assert.deepEqual(errors, []);
    console.log(JSON.stringify({ results, defaults, errors }, null, 2));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
