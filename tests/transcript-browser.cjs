// Run with BOID_PLAYWRIGHT pointing to an installed playwright-core package.
// CHROME_PATH can select an installed Chromium executable. No live server is used.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const entries = Array.from({ length: 600 }, (_, i) => ({ id: String(i), kind: 'assistant', text: `Entry ${i}: ` + 'text '.repeat(60) }));
const views = [{ id: 'sender', name: 'canary', cwd: '/tmp', status: 'idle', entries, pending: [] },
  { id: 'recipient', name: 'swift', cwd: '/tmp', status: 'idle', entries: [{ id: 'peer-old', kind: 'peer', text: 'Old message' }], pending: [] }];
const message = { id: 'peer-old', sender_id: 'sender', recipient_id: 'recipient', sender_entry: '10', sender_name: 'canary', recipient_name: 'swift', text: 'Old message', state: 'delivered' };
views.forEach(view => view.communications = [message]);
(async () => {
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
    const errors = [];
    const submitted = [];
    let interrupts = 0;
    let failInterrupt = false;
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.EventSource = class { constructor() { window.testEvents = this; } close() {} };
    });
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', next_agent_name: 'canary', agent_names: ['canary', 'swift'] } });
      if (url.pathname.endsWith('/interrupt')) {
        interrupts++;
        return route.fulfill({ status: failInterrupt ? 500 : 202,
          json: failInterrupt ? { error: 'Fixture failure' } : { status: 'interrupt_requested' } });
      }
      if (url.pathname.endsWith('/messages')) {
        submitted.push(route.request().postDataJSON().text);
        return route.fulfill({ json: {} });
      }
      if (url.pathname.startsWith('/api/sessions/')) return route.fulfill({ json: views.find(view => url.pathname.split('/')[3] === view.id) });
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    await page.goto('http://boid.test');
    await page.waitForFunction(() => window.testEvents);
    const frames = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await page.evaluate(views => snapshot(views, []), views);
    await frames();
    assert.deepEqual(await page.evaluate(() => {
      const ui = sessions.get('sender');
      return [ui.entries.size, ui.mounted.size, ui.entries.get('0').node.isConnected, ui.entries.get('599').node.isConnected];
    }), [600, 600, true, true]);

    // Peer links still navigate to earlier messages in the full transcript.
    await page.locator('[data-pane-key="agent:recipient"] .peer-link').click();
    await page.waitForFunction(() => document.activeElement === sessions.get('sender').entries.get('10').node);
    assert.equal(await page.locator('.transcript-history').count(), 0);
    await page.evaluate(() => { const ui = sessions.get('sender'); ui.follow = true; followLatest(ui); });
    await frames();

    if (process.env.BOID_SCREENSHOT) await page.screenshot({ path: process.env.BOID_SCREENSHOT });

    // Peer updates must not rebuild unrelated expanded tool output.
    await page.evaluate(() => {
      const ui = sessions.get('sender');
      putEntry(ui, { id: 'call', kind: 'tool_call', text: 'exec_command (call-1)\n{"command":"echo hello"}' });
      putEntry(ui, { id: 'result', kind: 'tool_result', text: 'exec_command (call-1)\n{"stdout":{"text":"hello"},"exit_code":0}' });
      putEntry(ui, { id: 'receipt', kind: 'tool_result', text: 'send_message (send-1)\n{"message_id":"new-peer","queued":true}' });
    });
    await frames();
    await page.locator('[data-pane-key="agent:sender"] .tool-operation > summary').first().click();
    await page.evaluate(() => { window.testTool = sessions.get('sender').entries.get('call').body.firstChild; });
    await page.evaluate(message => receiveCommunication({ ...message, id: 'new-peer', sender_entry: '599' }), message);
    await frames();
    assert.deepEqual(await page.evaluate(() => {
      const ui = sessions.get('sender');
      return [ui.entries.get('call').body.firstChild === testTool, testTool.open, ui.entries.get('receipt').node.hidden];
    }), [true, true, true]);

    // Text selected during streaming remains intact until selection clears.
    await page.evaluate(() => {
      const ui = sessions.get('sender'), item = ui.entries.get('598');
      const range = document.createRange(); range.selectNodeContents(item.body);
      const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
      window.testSelection = selection.toString();
      putEntry(ui, { id: '598', text: ' extra streaming text' }, true);
      for (let i = 0; i < 250; i++) putEntry(ui, { id: 'live-' + i, kind: 'assistant', text: 'Live ' + i });
    });
    await frames();
    assert.equal(await page.evaluate(() => getSelection().toString() === testSelection && sessions.get('sender').entries.get('598').node.isConnected), true);
    await page.evaluate(() => getSelection().removeAllRanges());
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').entries.get('598').body.textContent.endsWith('extra streaming text')), true);
    await page.evaluate(() => { const ui = sessions.get('sender'); ui.follow = true; followLatest(ui); });
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 853);

    // Large entries do not remove older transcript content.
    await page.evaluate(() => {
      const ui = sessions.get('sender');
      for (let i = 0; i < 6; i++) putEntry(ui, { id: 'large-' + i, kind: 'assistant', text: 'x'.repeat(30000) });
    });
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 859);
    await page.evaluate(() => putEntry(sessions.get('sender'), { id: 'huge', kind: 'assistant', text: 'y'.repeat(120000) }));
    await frames();
    assert.deepEqual(await page.evaluate(() => {
      const ui = sessions.get('sender'); return [ui.mounted.size, ui.entries.get('huge').body.textContent.length];
    }), [860, 120000]);

    // Hidden reasoning cannot replace the visible tail; replay/reconnect retains all records.
    await page.evaluate(() => putEntry(sessions.get('sender'), { id: 'reasoning', kind: 'reasoning', text: 'z'.repeat(150000) }));
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').entries.get('huge').node.isConnected), true);
    await page.evaluate(views => snapshot(views, []), views);
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').entries.size), 600);
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 600);
    // Real wheel gestures at the bottom must not fight automatic following.
    const trace = page.locator('[data-pane-key="agent:sender"] .trace');
    await trace.hover();
    for (let i = 0; i < 4; i++) {
      await page.mouse.wheel(0, 150);
      await page.waitForTimeout(80);
      assert.ok(await page.evaluate(() => {
        const ui = sessions.get('sender');
        return ui.trace.scrollHeight - ui.trace.clientHeight - ui.trace.scrollTop <= 1.1;
      }));
    }
    await page.evaluate(() => putEntry(sessions.get('sender'), { id: '599', kind: 'assistant', text: 'Tail' }));
    await frames();
    await page.evaluate(() => {
      window.streamTimer = setInterval(() => putEntry(sessions.get('sender'), {
        id: '599', text: '.',
      }, true), 40);
    });
    try {
      for (let i = 0; i < 5; i++) {
        await page.mouse.wheel(0, 150);
        await page.waitForTimeout(80);
        assert.ok(await page.evaluate(() => {
          const ui = sessions.get('sender');
          return ui.trace.scrollHeight - ui.trace.clientHeight - ui.trace.scrollTop <= 1.1;
        }));
      }
      assert.equal(await page.evaluate(() => sessions.get('sender').follow), true);
      for (const amount of [1, 4, 4, 4, 10]) {
        const before = await page.evaluate(() => sessions.get('sender').trace.scrollTop);
        await page.mouse.wheel(0, -amount);
        await page.waitForTimeout(80);
        const after = await page.evaluate(() => {
          const ui = sessions.get('sender'); return { top: ui.trace.scrollTop, follow: ui.follow };
        });
        assert.equal(after.follow, false);
        assert.ok(after.top < before - 0.5, `Small upward gesture stuck: ${before} -> ${after.top}`);
      }
      await page.mouse.wheel(0, 500);
      await page.waitForTimeout(80);
      assert.equal(await page.evaluate(() => sessions.get('sender').follow), true);
      await page.evaluate(() => putEntry(sessions.get('sender'), { id: 'new-tail', kind: 'assistant', text: 'New streamed entry' }));
      await frames();
      assert.ok(await page.evaluate(() => {
        const ui = sessions.get('sender'); return ui.follow && atBottom(ui);
      }));
      await page.mouse.wheel(0, -250);
      await page.waitForTimeout(100);
      assert.equal(await page.evaluate(() => sessions.get('sender').follow), false);
    } finally { await page.evaluate(() => clearInterval(streamTimer)); }

    // A tall prefix stays mounted when new entries arrive, preserving bottom spacing.
    await page.evaluate(() => {
      const history = Array.from({ length: 200 }, (_, i) => ({ id: String(i), kind: 'assistant',
        text: i === 0 ? 'tall line\n'.repeat(200) : 'short line' }));
      snapshot([{ id: 'sender', name: 'canary', cwd: '/tmp', status: 'idle', entries: history, pending: [] }], []);
      const ui = sessions.get('sender'); ui.follow = true;
    });
    await frames();
    await page.evaluate(() => {
      const ui = sessions.get('sender'); ui.trace.scrollTop = messageBottom(ui) + 40;
      retainPromptSpace(ui); ui.autoClearance = false; ui.followClearance = 0;
      putEntry(ui, { id: '200', kind: 'assistant', text: 'new short line' });
    });
    await frames();
    assert.ok(await page.evaluate(() => {
      const ui = sessions.get('sender');
      return ui.trace.scrollTop - messageBottom(ui) <= ui.overlay.getBoundingClientRect().height + composerPadding + 1;
    }));
    assert.equal(await page.evaluate(() => sessions.get('sender').entries.get('0').node.isConnected), true);
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 201);

    // Slash commands are discoverable, keyboard-selectable, and submitted exactly.
    const composer = page.locator('[data-pane-key="agent:sender"] textarea');
    await composer.fill('/');
    assert.deepEqual(await page.locator('.slash-option:visible').allTextContents(), [
      '/login  Choose API or subscription', '/model  Select model and effort', '/skills  Select a skill',
      '/oai-compact  Compact OpenAI context', '/clear  Start fresh model context'
    ]);
    await composer.press('ArrowDown');
    await composer.press('ArrowDown');
    await composer.press('ArrowDown');
    await composer.press('Enter');
    await page.waitForFunction(() => !sessions.get('sender').sending);
    assert.equal(submitted.at(-1), '/oai-compact');
    await composer.fill('/cl');
    await composer.press('Tab');
    assert.equal(await composer.inputValue(), '/clear');
    await composer.press('Enter');
    await page.waitForFunction(() => !sessions.get('sender').sending);
    assert.equal(submitted.at(-1), '/clear');
    await composer.fill('/');
    await page.locator('.slash-option:visible').filter({ hasText: '/oai-compact' }).click();
    await page.waitForFunction(() => !sessions.get('sender').sending);
    assert.equal(submitted.at(-1), '/oai-compact');
    await composer.fill('/clear explanation');
    assert.equal(await page.locator('.slash-option:visible').count(), 0);
    await composer.press('Enter');
    await page.waitForFunction(() => !sessions.get('sender').sending);
    assert.equal(submitted.at(-1), '/clear explanation');
    // Acceptance is not interruption completion. Repeat presses must wait for status.
    await page.evaluate(() => setStatus(sessions.get('sender'), 'working', Date.now()));
    for (let i = 0; i < 5; i++) {
      await composer.press('Escape');
      await page.waitForFunction(() => !sessions.get('sender').interrupting);
    }
    assert.equal(interrupts, 1);
    assert.match(await page.locator('[data-pane-key="agent:sender"] .working-status').textContent(), /Interrupt requested/);
    await page.evaluate(() => {
      const ui = sessions.get('sender');
      setStatus(ui, 'stopped');
      setStatus(ui, 'working', Date.now());
      ui.input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', repeat: true, bubbles: true }));
    });
    assert.equal(interrupts, 1); // A held key must not interrupt the next task.
    await composer.press('Escape');
    await page.waitForFunction(() => !sessions.get('sender').interrupting);
    assert.equal(interrupts, 2);
    await page.evaluate(() => { const ui = sessions.get('sender'); setStatus(ui, 'stopped'); setStatus(ui, 'working', Date.now()); });
    failInterrupt = true;
    await composer.press('Escape');
    await page.waitForFunction(() => !sessions.get('sender').interrupting);
    assert.equal(await page.evaluate(() => sessions.get('sender').interruptRequested), false);
    failInterrupt = false;
    await composer.press('Escape');
    await page.waitForFunction(() => !sessions.get('sender').interrupting);
    assert.equal(interrupts, 4);
    await composer.fill('/');
    await composer.press('Escape');
    assert.equal(await page.locator('.slash-option:visible').count(), 0);
    assert.equal(interrupts, 4); // Dismissing a picker does not interrupt.
    assert.deepEqual(errors, []);
    console.log('PASS: full history, peer navigation, targeted updates, expanded tools, selection/streaming, large entries, hidden reasoning, reconnect, bottom wheel/streaming, small upward gestures, follow resumption, retained composer space');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
