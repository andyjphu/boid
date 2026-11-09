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
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.EventSource = class { constructor() { window.testEvents = this; } close() {} };
    });
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', next_agent_name: 'canary', agent_names: ['canary', 'swift'] } });
      if (url.pathname.startsWith('/api/sessions/')) return route.fulfill({ json: views.find(view => url.pathname.endsWith('/' + view.id)) });
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
    }), [600, 200, false, true]);

    // Loading older history preserves the currently read line's screen position.
    const offset = await page.evaluate(() => {
      const ui = sessions.get('sender'); ui.follow = false; ui.trace.scrollTop = 0;
      const item = [...ui.mounted][0]; window.testAnchor = item;
      const top = item.node.getBoundingClientRect().top;
      ui.historyOlder.click(); return top;
    });
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 400);
    assert.ok(Math.abs(await page.evaluate(() => testAnchor.node.getBoundingClientRect().top) - offset) < 2);

    // A peer link reveals a culled destination, and Latest restores the bounded view.
    await page.locator('[data-pane-key="agent:recipient"] .peer-link').click();
    await page.waitForFunction(() => sessions.get('sender').entries.get('10').node.isConnected);
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').entries.get('10').node.isConnected), true);
    await page.evaluate(() => sessions.get('sender').historyOlder.click());
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 600);
    assert.equal(await page.locator('[data-pane-key="agent:sender"] .transcript-history').isVisible(), true);
    await page.evaluate(() => sessions.get('sender').historyLatest.click());
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 200);

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
    await page.evaluate(() => sessions.get('sender').historyLatest.click());
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 200);

    // The character budget can cull fewer than 200 entries; a single large entry stays whole.
    await page.evaluate(() => {
      const ui = sessions.get('sender');
      for (let i = 0; i < 6; i++) putEntry(ui, { id: 'large-' + i, kind: 'assistant', text: 'x'.repeat(30000) });
    });
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 3);
    await page.evaluate(() => putEntry(sessions.get('sender'), { id: 'huge', kind: 'assistant', text: 'y'.repeat(120000) }));
    await frames();
    assert.deepEqual(await page.evaluate(() => {
      const ui = sessions.get('sender'); return [ui.mounted.size, ui.entries.get('huge').body.textContent.length];
    }), [1, 120000]);

    // Hidden reasoning cannot replace the visible tail; replay/reconnect retains all records.
    await page.evaluate(() => putEntry(sessions.get('sender'), { id: 'reasoning', kind: 'reasoning', text: 'z'.repeat(150000) }));
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').entries.get('huge').node.isConnected), true);
    await page.evaluate(views => snapshot(views, []), views);
    await frames();
    assert.equal(await page.evaluate(() => sessions.get('sender').entries.size), 600);
    assert.equal(await page.evaluate(() => sessions.get('sender').mounted.size), 200);
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
    await page.evaluate(() => {
      let n = 0;
      window.streamTimer = setInterval(() => putEntry(sessions.get('sender'), {
        id: 'wheel-' + n++, kind: 'assistant', text: 'stream '.repeat(40),
      }), 40);
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
      await page.mouse.wheel(0, -250);
      await page.waitForTimeout(100);
      assert.equal(await page.evaluate(() => sessions.get('sender').follow), false);
    } finally { await page.evaluate(() => clearInterval(streamTimer)); }

    // Removing a tall prefix must move retained composer space with the content.
    await page.evaluate(() => {
      const history = Array.from({ length: 200 }, (_, i) => ({ id: String(i), kind: 'assistant',
        text: i === 0 ? 'tall line\n'.repeat(200) : 'short line' }));
      snapshot([{ id: 'sender', name: 'canary', cwd: '/tmp', status: 'idle', entries: history, pending: [] }], []);
      const ui = sessions.get('sender'); ui.follow = true;
      ui.historyLimit = transcriptEntries; ui.historyCharacters = transcriptCharacters;
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
    assert.deepEqual(errors, []);
    console.log('PASS: bounded history, older history/scroll anchor, peer navigation, targeted updates, expanded tools, selection/streaming, character budget, hidden reasoning, reconnect, bottom wheel/streaming, retained composer space');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
