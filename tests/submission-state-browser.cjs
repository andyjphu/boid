// Real Chrome with held HTTP responses and mocked screen snapshots; no live agent/provider calls.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
(async () => {
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 800 } });
    const pending = [], errors = []; let sequence = 0;
    page.on('pageerror', error => errors.push(error.message));
    const views = ['a', 'b'].map(id => ({ id, name: id, cwd: '/tmp', status: 'idle', pending: [], entries: [] }));
    const screens = { revision: 1, count: 2, panes: {} };
    await page.addInitScript(() => { window.EventSource = class {
      constructor(url) { this.url = url; window.testEvents = this; setTimeout(() => { this.onopen?.(); window.fixtureSnapshot(); }, 0); }
      close() {}
    }; });
    await page.exposeFunction('fixtureSnapshot', () => page.evaluate(({ views, screens, seq }) => {
      const screen = Number(new URL(testEvents.url, location.origin).searchParams.get('screen'));
      testEvents.onmessage({ data: JSON.stringify({ type: 'snapshot', seq, screens, catalog: ['a', 'b'], sessions: screen === 1 ? views : [], terminals: [] }) });
    }, { views, screens, seq: ++sequence }));
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', agent_names: [] } });
      if (url.pathname === '/api/screens') return route.fulfill({ json: screens });
      if (url.pathname.endsWith('/messages')) { pending.push({ route, text: route.request().postDataJSON().text }); return; }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    const until = async check => { for (let i = 0; !check(); i++) { assert.ok(i < 200, 'request arrived'); await new Promise(resolve => setTimeout(resolve, 10)); } };
    const frames = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const input = page.locator('[data-pane-key="agent:a"] .composer textarea');
    const error = page.locator('[data-pane-key="agent:a"] .error-banner');
    const submit = async text => { await input.fill(text); await input.press('Enter'); };
    const ack = async index => { await pending[index].route.fulfill({ status: 202, json: { message_id: `m${index}`, status: 'queued' } }); await frames(); };
    const remount = async () => {
      await page.locator('#screens [data-screen="2"]').click(); await page.waitForFunction(() => activeScreen === 2 && !sessions.has('a'));
      await page.locator('#screens [data-screen="1"]').click(); await page.waitForFunction(() => sessions.has('a') && !sessions.get('a').restoringScreen);
    };
    await page.goto('http://boid.test/?input-timing=1'); await page.waitForFunction(() => sessions.has('a'));
    await submit('Same board'); await until(() => pending.length === 1);
    await page.locator('[data-pane-key="agent:b"] .composer textarea').focus();
    assert.equal(await input.inputValue(), 'Same board'); await ack(0); assert.equal(await input.inputValue(), '');
    // Second Enter is an explicit new intent. Transport remains serial and neither draft clears early.
    await submit('First'); await until(() => pending.length === 2); await submit('Second');
    await input.press('Enter'); await frames(); assert.equal(pending.length, 2, 'unchanged pending revision deduplicates');
    assert.equal(await input.inputValue(), 'Second'); assert.equal(await page.locator('.entry[data-kind=user]').count(), 0);
    assert.match(await page.locator('[data-pane-key="agent:a"] .working-status').innerText(), /1 waiting locally/);
    await ack(1); await until(() => pending.length === 3); assert.equal(pending[2].text, 'Second'); assert.equal(await input.inputValue(), 'Second');
    await ack(2); assert.equal(await input.inputValue(), '');
    const timings = await page.evaluate(() => boidInputTiming.samples());
    assert.ok(timings.at(-1).submitToDispatchMs > 0); assert.ok(timings.at(-1).submitToHeadersMs >= timings.at(-1).submitToDispatchMs);
    // A newer edit with identical text has a new identity and survives the older acknowledgment.
    await submit('Identical'); await until(() => pending.length === 4); await input.fill('Changed'); await input.fill('Identical');
    await ack(3); assert.equal(await input.inputValue(), 'Identical');
    await input.press('Enter'); await until(() => pending.length === 5); await ack(4);
    // Detaching DOM cannot reset the in-flight guard, and acknowledgment clears the current pane.
    await submit('Remount'); await until(() => pending.length === 6); await remount();
    await input.press('Enter'); await frames(); assert.equal(pending.length, 6); await ack(5); assert.equal(await input.inputValue(), '');
    await submit('Old after remount'); await until(() => pending.length === 7); await remount(); await input.fill('New edit');
    await ack(6); assert.equal(await input.inputValue(), 'New edit');
    // Failure retains both explicit intents, pauses dispatch and follows the session to its new DOM.
    await submit('Rejected'); await until(() => pending.length === 8); await submit('After rejection'); await remount();
    await pending[7].route.fulfill({ status: 400, json: { error: 'Fixture rejection' } }); await frames();
    assert.equal(pending.length, 8); assert.match(await error.innerText(), /Fixture rejection/); assert.equal(await input.inputValue(), 'After rejection');
    if (process.env.SCREENSHOT) await page.screenshot({ path: process.env.SCREENSHOT });
    await error.getByRole('button', { name: 'Retry', exact: true }).click(); await until(() => pending.length === 9);
    assert.equal(pending[8].text, 'Rejected'); await ack(8); await until(() => pending.length === 10);
    assert.equal(pending[9].text, 'After rejection'); await ack(9); assert.equal(await input.inputValue(), '');
    // Refresh preserves recoverable intents but never resends uncertain requests automatically.
    await submit('Uncertain'); await until(() => pending.length === 11); await submit('Never dispatched');
    await page.reload(); await page.waitForFunction(() => sessions.has('a')); await frames();
    assert.equal(pending.length, 11); assert.equal(await input.inputValue(), 'Never dispatched'); assert.match(await error.innerText(), /interrupted by reload/);
    await error.getByRole('button', { name: 'Resend (may duplicate)', exact: true }).click(); await until(() => pending.length === 12);
    await ack(11); await until(() => pending.length === 13); assert.equal(pending[12].text, 'Never dispatched'); await ack(12);
    // The bounded queue refuses excess input visibly and retains the current text.
    for (let i = 0; i < 8; i++) await submit(`Bounded ${i}`);
    await until(() => pending.length === 14); await submit('Ninth retained draft');
    assert.match(await error.innerText(), /Eight messages/); assert.equal(await input.inputValue(), 'Ninth retained draft');
    for (let i = 13; i < 21; i++) { await until(() => pending.length > i); await ack(i); }
    assert.equal(await input.inputValue(), 'Ninth retained draft'); assert.equal(await page.locator('.entry[data-kind=user]').count(), 0);
    // Recovery storage is best effort: it cannot gate real dispatch, ACK clearing or the FIFO.
    await page.evaluate(() => {
      window.recoveryOrder = []; window.originalFetch = window.fetch;
      window.fetch = function(url, options) { if (String(url).endsWith('/messages')) recoveryOrder.push('dispatch'); return originalFetch.call(this, url, options); };
      window.originalSetItem = Storage.prototype.setItem; Storage.prototype.setItem = function(key, value) {
      if (key.startsWith('boid:submissions:')) { recoveryOrder.push('save'); throw new Error('Fixture storage failure'); }
      return originalSetItem.call(this, key, value);
    }; });
    await submit('Storage failure still sends'); await until(() => pending.length === 22);
    await frames(); const order = await page.evaluate(() => recoveryOrder); assert.ok(order.indexOf('dispatch') < order.indexOf('save'), 'fetch precedes recovery storage');
    await submit('Follow-up still sends'); await frames(); assert.match(await error.innerText(), /Recovery storage unavailable/);
    await ack(21); await until(() => pending.length === 23); assert.equal(pending[22].text, 'Follow-up still sends');
    await ack(22); assert.equal(await input.inputValue(), '');
    await page.evaluate(() => { Storage.prototype.setItem = originalSetItem; window.fetch = originalFetch; });
    assert.deepEqual(errors, []);
    console.log('PASS: serial explicit Enter intents, same-board focus, screen remount deduplication, exact draft revision clearing, paused failures/retry, refresh recovery, bounded queue, dispatch timing, no optimistic messages');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
