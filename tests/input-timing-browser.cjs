// Real Chrome, controlled HTTP/SSE. These are isolated ordering checks, not live latency measurements.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
(async () => {
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  try {
    const page = await browser.newPage({ viewport: { width: 1000, height: 750 } });
    const errors = [], pending = []; let automatic = false, number = 0, sequence = 0;
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => { window.EventSource = class { constructor() { window.testEvents = this; } close() {} }; });
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', agent_names: [] } });
      if (url.pathname.endsWith('/messages')) {
        const message = { route, id: `message-${++number}` }; pending.push(message);
        if (automatic) return route.fulfill({ status: 202, json: { message_id: message.id, status: 'queued' } });
        return;
      }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    const view = entries => ({ id: 'a', name: 'a', cwd: '/tmp', status: 'idle', pending: [], entries });
    const frames = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const emit = async message => { await page.evaluate(message => testEvents.onmessage({ data: JSON.stringify(message) }), { ...message, seq: ++sequence }); await frames(); };
    const reset = async (entries = []) => { await page.waitForFunction(() => testEvents?.onmessage); await emit({ type: 'snapshot', sessions: [view(entries)], terminals: [], catalog: ['a'] }); };
    const samples = () => page.evaluate(() => boidInputTiming.samples());
    const input = page.locator('.composer textarea');
    const submit = async text => {
      const count = pending.length; await input.fill(text); await input.press('Enter');
      for (let i = 0; pending.length === count; i++) { assert.ok(i < 100); await new Promise(resolve => setTimeout(resolve, 10)); }
      return pending.at(-1);
    };
    const ack = async message => {
      await message.route.fulfill({ status: 202, headers: { 'Server-Timing': 'boid_lock;dur=0.25, boid_mutate;dur=0.75, boid_admit;dur=1.1, boid_handler;dur=1.3' }, json: { message_id: message.id, status: 'queued' } });
      await page.waitForFunction(() => !sessions.get('a')?.sending);
    };
    const entry = message => emit({ type: 'entry', session_id: 'a', entry_index: 0, entry: { id: message.id, kind: 'user', text: 'Server-delivered content' } });
    await page.goto('http://boid.test'); await reset();
    const off = await submit('Wait for real acknowledgment');
    assert.equal(await input.inputValue(), 'Wait for real acknowledgment');
    assert.equal(await page.locator('.entry[data-kind=user]').count(), 0, 'no optimistic bubble');
    await ack(off); assert.equal(await input.inputValue(), ''); assert.deepEqual(await samples(), []);
    await page.evaluate(() => boidInputTiming.enable());
    const early = await submit('Private prompt excluded from diagnostics'); await entry(early);
    assert.equal(await input.inputValue(), 'Private prompt excluded from diagnostics', 'SSE does not clear before HTTP');
    assert.equal(await page.locator(`.entry[data-entry-id="${early.id}"]`).count(), 1); await ack(early);
    let sample = (await samples()).at(-1);
    assert.equal(sample.outcome, 'rendered'); assert.equal(sample.entrySource, 'sse');
    assert.ok(sample.submitToServerEntryMs <= sample.submitToDOMRenderMs && sample.submitToDOMRenderMs <= sample.submitToAcknowledgmentMs);
    assert.ok(sample.submitToComposerClearMs >= sample.submitToAcknowledgmentMs);
    assert.deepEqual(sample.serverTimingMs, { boid_lock: .25, boid_mutate: .75, boid_admit: 1.1, boid_handler: 1.3 });
    assert.ok(!JSON.stringify(await samples()).includes('Private prompt'));
    await page.evaluate(() => { const rows = boidInputTiming.samples(); rows[0].serverTimingMs.boid_lock = 900; rows.length = 0; });
    assert.equal((await samples())[0].serverTimingMs.boid_lock, .25);
    const rejected = await submit('Rejected input'); await rejected.route.fulfill({ status: 400, json: { error: 'Fixture rejection' } });
    await page.waitForFunction(() => !sessions.get('a').sending); sample = (await samples()).at(-1);
    assert.equal(sample.outcome, 'http_error'); assert.equal(sample.httpStatus, 400); assert.equal(sample.submitToDOMRenderMs, null); assert.equal(await input.inputValue(), 'Rejected input');
    const network = await submit('Network failure'); await network.route.abort(); await page.waitForFunction(() => !sessions.get('a').sending);
    assert.equal((await samples()).at(-1).outcome, 'network_error'); assert.equal(await input.inputValue(), 'Network failure');
    const hidden = await submit('Hidden pane'); await emit({ type: 'snapshot', sessions: [], terminals: [], catalog: ['a'] });
    await entry(hidden); await ack(hidden); await page.waitForFunction(() => boidInputTiming.samples().at(-1).messageId !== null);
    sample = (await samples()).at(-1); assert.equal(sample.outcome, 'accepted_waiting_dom'); assert.equal(sample.submitToDOMRenderMs, null); assert.equal(sample.submitToComposerClearMs, null);
    await reset([{ id: hidden.id, kind: 'user', text: 'Hidden pane' }]); assert.equal((await samples()).at(-1).outcome, 'rendered');
    const disabled = await submit('Disable while waiting'); await page.evaluate(() => { boidInputTiming.disable(); boidInputTiming.enable(); }); await ack(disabled);
    assert.equal((await samples())[0].outcome, 'rendered'); assert.equal((await samples()).at(-1).outcome, 'observation_disabled');
    await page.evaluate(() => boidInputTiming.clear()); automatic = true;
    await page.evaluate(async () => { for (let i = 0; i < 65; i++) { const ui = sessions.get('a'); ui.input.value = 'Bounded fixture'; ui.composer.requestSubmit(); await ui.submission; await Promise.resolve(); } });
    assert.equal((await samples()).length, 64); assert.ok((await samples()).every(row => row.outcome === 'accepted_waiting_server_entry'));
    assert.equal((await page.evaluate(() => boidInputTiming.stats())).evictedRecords, 1);
    await page.goto('http://boid.test/?input-timing=1'); await reset(); assert.equal(await page.evaluate(() => boidInputTiming.stats().enabled), true);
    assert.deepEqual(errors, []);
    console.log('PASS: measurement-only UI; HTTP/SSE ordering, correlation, failures, hidden DOM=null, disable/reenable, bounded copies, no content logging, URL opt-in');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
