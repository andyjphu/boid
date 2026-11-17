// Real Chrome with a shared mocked catalog. No live agents, shells or provider calls.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {chromium} = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const view = id => ({id, name: id, cwd: '/mock', status: 'idle', pending: [],
  entries: [{id: `${id}-text`, kind: 'assistant', text: `Retained ${id} history`}]});

(async () => {
  const browser = await chromium.launch({headless: true,
    ...(process.env.CHROME_PATH ? {executablePath: process.env.CHROME_PATH} : {})});
  try {
    const context = await browser.newContext({viewport: {width: 1100, height: 750}});
    const pages = [], errors = [], mutations = [], deletions = [], creations = [];
    const agents = [view('a'), view('background')];
    const terms = [{id: 't', name: '#1', cwd: '/mock', running: true}];
    const screens = {revision: 1, count: 6, removed: [], panes: {
      'agent:a': 1, 'agent:background': 1, 'terminal:t': 3,
    }};
    let sequence = 0;
    const clone = value => JSON.parse(JSON.stringify(value));
    const groups = () => ({revision: 1, groups: [
      {id: 'agent:a', screen: 1, tabs: ['agent:a', 'agent:background'], credential_id: null},
      {id: 'terminal:t', screen: 3, tabs: ['terminal:t'], credential_id: null},
      ...agents.slice(2).map(agent => ({id: `agent:${agent.id}`, screen: screens.panes[`agent:${agent.id}`], tabs: [`agent:${agent.id}`], credential_id: null})),
      ...terms.slice(1).map(term => ({id: `terminal:${term.id}`, screen: screens.panes[`terminal:${term.id}`], tabs: [`terminal:${term.id}`], credential_id: null})),
    ]});
    const emit = async message => {
      const event = {...message, seq: ++sequence};
      await Promise.all(pages.map(page => page.evaluate(event =>
        window.testEvents?.onmessage?.({data: JSON.stringify(event)}), event)));
    };
    const snapshot = async (page, url) => {
      const selected = Number(new URL(url, 'http://boid.test').searchParams.get('screen'));
      await page.evaluate(({url, event}) => {
        if (window.testEvents?.url === url) window.testEvents.onmessage?.({data: JSON.stringify(event)});
      }, {url, event: {type: 'snapshot', seq: ++sequence, screens: clone(screens),
        pane_groups: groups(), catalog: agents.map(agent => agent.id),
        sessions: agents.filter(agent => (screens.panes[`agent:${agent.id}`] || 1) === selected),
        terminals: terms}});
    };
    await context.exposeBinding('fixtureSubscribe', ({page}, url) => snapshot(page, url));
    await context.addInitScript(() => {
      window.EventSource = class {
        constructor(url) { this.url = url; window.testEvents = this;
          setTimeout(() => { if (!this.closed) { this.onopen?.(); window.fixtureSubscribe(url); } }, 0); }
        close() { this.closed = true; }
      };
      window.WebSocket = class {
        static OPEN = 1;
        constructor() { this.readyState = 1; }
        close() { this.readyState = 3; }
        send() {}
      };
    });
    await context.route('**/*', route => {
      const url = new URL(route.request().url()), method = route.request().method();
      if (url.hostname !== 'boid.test') return route.abort();
      if (method !== 'GET') mutations.push({method, path: url.pathname});
      if (url.pathname === '/api/config') return route.fulfill({json: {cwd: '/mock', agent_names: []}});
      if (url.pathname === '/api/screens') {
        if (method === 'POST') { screens.count++; screens.revision++; }
        return route.fulfill({json: clone(screens)});
      }
      if (method === 'POST' && ['/api/sessions', '/api/terminals'].includes(url.pathname)) {
        creations.push({kind: url.pathname.endsWith('sessions') ? 'agent' : 'terminal', screen: Number(url.searchParams.get('screen')), route});
        return;
      }
      if (/^\/api\/screens\/\d+$/.test(url.pathname) && method === 'DELETE') {
        assert.equal(route.request().postData(), null);
        const id = Number(url.pathname.split('/').at(-1));
        if (Object.values(screens.panes).includes(id)) return route.fulfill({json: clone(screens)});
        deletions.push({id, route});
        return; // Explicitly hold confirmation to expose optimistic UI and stale-response bugs.
      }
      if (url.pathname === '/api/pane-groups') return route.fulfill({json: groups()});
      if (url.pathname.startsWith('/api/')) return route.fulfill({json: []});
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript'
        : file.endsWith('.css') ? 'text/css' : 'text/html'});
    });
    const frames = page => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const ready = async (page, id) => { await page.waitForFunction(id => catalogReady && activeScreen === id, id); await frames(page); };
    const open = async id => {
      const page = await context.newPage(); pages.push(page); page.on('pageerror', error => errors.push(error.message));
      await page.goto(`http://boid.test/?screen=${id}`); await ready(page, id); return page;
    };
    const select = async (page, id) => { await page.locator(`#screens [data-screen="${id}"]`).click(); await ready(page, id); };
    const until = async predicate => { for (let i = 0; !predicate(); i++) {
      assert.ok(i < 200, 'expected DELETE request'); await new Promise(resolve => setTimeout(resolve, 10));
    }};
    const accept = async id => {
      await until(() => deletions.some(item => item.id === id && !item.accepted));
      const request = deletions.find(item => item.id === id && !item.accepted); request.accepted = true;
      const occupied = Object.values(screens.panes).includes(id);
      if (!occupied && !screens.removed.includes(id) && screens.count - screens.removed.length > 1) {
        screens.removed.push(id); screens.revision++;
      }
      const response = clone(screens); await emit({type: 'screens', screens: response});
      return () => request.route.fulfill({json: response});
    };
    const page = await open(1), other = await open(2);
    await other.reload(); await ready(other, 2);
    assert.equal(deletions.length, 0, 'initial load/reload must not delete viewed empty screens');
    assert.equal(await other.locator('#screens [data-screen="2"]').count(), 1);
    assert.equal(await page.locator('.subpane[hidden]').count(), 1, 'background agent exists in occupied group');
    await page.locator('.subpane[data-pane-key="agent:a"] .composer textarea').fill('Retain unsent draft');

    await page.evaluate(() => { window.oldScreenSource = testEvents; });
    await select(page, 2); assert.equal(deletions.length, 0, 'leaving occupied agent group is not empty');
    await select(page, 3); await until(() => deletions.some(item => item.id === 2));
    assert.equal(await page.locator('#screens [data-screen="2"]').count(), 1, 'no optimistic removal');
    assert.equal(await other.evaluate(() => activeScreen), 2, 'other browser remains until server confirms');
    assert.equal(await page.locator('.terminal-pane').count(), 1);
    await select(page, 4);
    assert.ok(!deletions.some(item => item.id === 3), 'terminal-only screen is occupied');
    const finishTwo = await accept(2); await finishTwo();
    await ready(other, 1); await ready(page, 4);
    // A retired EventSource may still deliver a queued callback after switching.
    await page.evaluate(() => {
      oldScreenSource.onmessage({data: JSON.stringify({type: 'snapshot', seq: 99999,
        screens: {...screenCatalog, revision: 99999, removed: [...screenCatalog.removed, activeScreen]},
        sessions: [], terminals: [], catalog: []})});
      oldScreenSource.onerror();
    });
    await frames(page);
    assert.equal(await page.evaluate(() => activeScreen), 4, 'stale source snapshot cannot select another screen');
    assert.equal(await page.evaluate(() => document.body.classList.contains('disconnected')), false,
      'stale source error cannot mark the current connection offline');
    assert.equal(await page.locator('#screens [data-screen="2"]').count(), 0);
    assert.equal(await page.locator('#screens [data-screen="3"]').innerText(), '2', 'labels remain ordinal, IDs stable');
    assert.equal(await page.locator('#screens [data-screen="4"]').count(), 1, 'current empty screen remains');

    await select(page, 1); const finishFour = await accept(4); await finishFour(); await ready(page, 1);
    assert.equal(await page.locator('.subpane[data-pane-key="agent:a"] .composer textarea').inputValue(), 'Retain unsent draft');
    assert.equal(await page.locator('.pane-tab[data-subpane-key="agent:background"]').count(), 1);
    await select(page, 5); await select(page, 6);
    const finishFive = await accept(5); // Its old HTTP reply remains held after the SSE confirmation.
    await ready(page, 6); await select(page, 1);
    const finishSix = await accept(6); await finishSix(); await finishFive(); await ready(page, 1);
    assert.deepEqual(await page.locator('#screens a').evaluateAll(nodes => nodes.map(node => node.dataset.screen)), ['1', '3']);
    assert.ok(!deletions.some(item => [1, 3].includes(item.id)), 'agent/hidden-agent/terminal screens are retained');
    await page.reload(); await ready(page, 1);
    const beforeOldLink = mutations.length;
    await other.goto('http://boid.test/?screen=2'); await ready(other, 1);
    assert.equal(mutations.length, beforeOldLink, 'loading a removed-screen URL falls back without cleanup');
    // Actual create controls keep their original empty screen while HTTP acceptance is pending.
    for (const kind of ['agent', 'terminal']) {
      await page.locator('#add-screen').click(); const destination = screens.count; await ready(page, destination);
      await page.locator(`#add-${kind}`).click(); await until(() => creations.some(item => item.kind === kind));
      await select(page, 1);
      assert.ok(!deletions.some(item => item.id === destination), 'pending creation protects its original screen');
      const creation = creations.find(item => item.kind === kind);
      const record = kind === 'agent' ? view('late-agent') : {id: 'late-terminal', name: '#2', cwd: '/mock', running: true};
      (kind === 'agent' ? agents : terms).push(record); screens.panes[`${kind}:${record.id}`] = destination; screens.revision++;
      await emit({type: 'screens', screens: clone(screens)});
      await creation.route.fulfill({json: record}); await frames(page);
      assert.equal(await page.evaluate(() => activeScreen), 1, 'late creation never switches the foreground screen');
      assert.ok(!screens.removed.includes(destination));
      await select(page, destination);
      assert.equal(await page.locator(`.subpane[data-pane-key="${kind}:${record.id}"]`).count(), 1);
      await select(page, 1);
    }
    assert.ok(mutations.every(item => (item.method === 'DELETE' && /^\/api\/screens\/\d+$/.test(item.path)) || (item.method === 'POST' && ['/api/screens', '/api/sessions', '/api/terminals'].includes(item.path))), 'only expected cleanup and explicit fixture creation requests occur');
    assert.deepEqual(errors, []);
    if (process.env.BOID_SCREENSHOT_DIR) await page.screenshot({path: path.join(process.env.BOID_SCREENSHOT_DIR, 'empty-screen-removal.png')});
    console.log('PASS: current empty screen retained; departure waits for server; occupied/background/terminal screens retained; two-window fallback; rapid switch and stale HTTP response preserve selection; initial/reload no delete; drafts preserved; pending agent and terminal creation protected');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
