// Real browser with mocked transport; no live agent or provider calls.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 700 } });
    const errors = [], mutations = []; let seq = 0, reject = false;
    const agents = ['canary', 'swift'].map(id => ({ id, name: id, cwd: '/tmp', status: 'idle', pending: [],
      profile: { model: 'fixture', service_tier: 'default' }, entries: [
        { id: `${id}-result`, kind: 'assistant', text: 'Completed result remains here.' },
        { id: `${id}-interrupt`, kind: 'notice', text: 'Execution interrupted. Completed results retained.' },
      ] }));
    const screens = { revision: 1, count: 1, panes: {} };
    page.on('pageerror', error => errors.push(error.message));
    const emit = message => page.evaluate(message => testEvents.onmessage({ data: JSON.stringify(message) }), { ...message, seq: ++seq });
    await page.exposeFunction('fixtureSnapshot', () => emit({ type: 'snapshot', screens, catalog: agents.map(a => a.id), sessions: agents, terminals: [] }));
    await page.addInitScript(() => { window.EventSource = class {
      constructor() { window.testEvents = this; setTimeout(() => { this.onopen?.(); window.fixtureSnapshot(); }, 0); }
      close() {}
    }; });
    await page.route('**/*', async route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (route.request().method() !== 'GET') mutations.push(url.pathname);
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', agent_names: [] } });
      if (url.pathname === '/api/screens') return route.fulfill({ json: screens });
      if (url.pathname.endsWith('/fast')) {
        if (reject) return route.fulfill({ status: 400, json: { error: 'Fixture fast rejection' } });
        const agent = agents.find(a => url.pathname.includes(`/${a.id}/`));
        const enabled = route.request().postDataJSON().enabled;
        if (enabled && agent.profile.service_tier !== 'priority') agent.entries.push({ id: `fast-${seq}`, kind: 'notice', text: 'Fast mode enabled' });
        agent.profile.service_tier = enabled ? 'priority' : 'default';
        await emit({ type: 'snapshot', screens, catalog: agents.map(a => a.id), sessions: agents, terminals: [] });
        return route.fulfill({ json: { profile: agent.profile } });
      }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    await page.goto('http://boid.test/');
    await page.waitForFunction(() => sessions.has('canary'));
    const pane = page.locator('[data-pane-key="agent:canary"]');
    const input = pane.locator('.composer textarea');
    const notice = pane.locator('.entry[data-kind="notice"]');
    const color = () => page.evaluate(() => getComputedStyle(sessions.get('canary').tab).backgroundColor);
    await notice.getByText('Interrupted.', { exact: true }).waitFor();
    assert.equal(await pane.getByText('Completed result remains here.', { exact: true }).count(), 1);
    const initialColor = await color();
    const submit = async command => { await input.fill(command); await input.press('Enter'); };
    reject = true; await submit('/fast on');
    await pane.getByText('Fixture fast rejection', { exact: false }).waitFor();
    assert.equal(await color(), initialColor);
    assert.equal(await notice.getByText('Fast mode enabled', { exact: true }).count(), 0);
    reject = false; await submit('/fast on');
    await notice.getByText('Fast mode enabled', { exact: true }).waitFor();
    assert.equal(await color(), 'rgb(255, 69, 58)');
    assert.equal(await page.evaluate(() => sessions.get('swift').tab.hasAttribute('data-fast')), false);
    const colors = await notice.evaluateAll(nodes => nodes.map(node => getComputedStyle(node).color));
    assert.equal(colors[0], colors[1], 'fast and interruption share notice style');
    await page.evaluate(() => sessions.get('canary').tab.classList.add('inactive'));
    assert.equal(await color(), 'rgb(255, 69, 58)', 'inactive fast tab stays red');
    await page.evaluate(() => sessions.get('canary').tab.classList.remove('inactive'));
    if (process.env.SCREENSHOT) await page.screenshot({ path: process.env.SCREENSHOT });
    await page.reload(); await page.waitForFunction(() => sessions.has('canary'));
    await notice.getByText('Fast mode enabled', { exact: true }).waitFor();
    assert.equal(await color(), 'rgb(255, 69, 58)', 'initial/reconnected profile restores color');
    await submit('/fast on'); await page.waitForFunction(() => !fastControls.has('canary'));
    assert.equal(await notice.getByText('Fast mode enabled', { exact: true }).count(), 1);
    await submit('/fast off'); await page.waitForFunction(() => !sessions.get('canary').tab.hasAttribute('data-fast'));
    assert.equal(await color(), initialColor);
    assert.equal(await notice.getByText('Fast mode enabled', { exact: true }).count(), 1, 'historical enable notice retained');
    assert.ok(mutations.every(url => url.endsWith('/fast')), 'commands never sent as model messages');
    assert.deepEqual(errors, []);
    console.log('PASS fast feedback: accepted/rejected toggle, per-agent red active/inactive tab, reconnect, notice style/dedup, off reset, shortened interrupt, retained results, no message submissions');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
