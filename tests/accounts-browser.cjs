// Browser interaction fixture; no real OAuth consent or provider request.
// Same BOID_PLAYWRIGHT and CHROME_PATH setup as transcript-browser.cjs.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
(async () => {
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
    const errors = [], actions = [];
    const status = { billing_route: 'api_key', selected_account: null, model: 'first', api_key_available: true,
      accounts: [{ id: 'fixture-account', label: 'fixture@example.invalid · account', connected: true, plan_enabled: true, paused: false }] };
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => { window.EventSource = class { constructor() { window.testEvents = this; } close() {} }; });
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', next_agent_name: 'canary', agent_names: ['canary'] } });
      if (url.pathname === '/api/accounts') return route.fulfill({ json: status });
      if (url.pathname.endsWith('/models')) return route.fulfill({ json: { models: [{ slug: 'second', display_name: 'Second model' }, { slug: 'first', display_name: 'First model' }] } });
      if (url.pathname === '/api/accounts/select') {
        const body = route.request().postDataJSON(); actions.push(body);
        Object.assign(status, { billing_route: body.billing_route, selected_account: body.account, model: body.model });
        return route.fulfill({ json: { welcome: true } });
      }
      if (url.pathname.endsWith('/logout')) {
        status.accounts[0].connected = false;
        return route.fulfill({ json: { revocation_confirmed: false, message: 'Signed out locally; remote revocation was not confirmed.' } });
      }
      if (url.pathname.endsWith('/resume')) {
        status.accounts[0].paused = false; actions.push('resume'); return route.fulfill({ json: {} });
      }
      if (url.pathname.endsWith('/stop')) { actions.push('stop'); return route.fulfill({ json: {} }); }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    await page.goto('http://boid.test');
    await page.waitForFunction(() => window.testEvents);
    await page.evaluate(() => snapshot([{ id: 'agent', name: 'canary', cwd: '/tmp', status: 'working', entries: [], pending: [], profile: { billing_route: 'chatgpt', model: 'first' } }], []));
    assert.match(await page.locator('.billing-route').textContent(), /Using ChatGPT plan/);
    await page.locator('#open-accounts').click();
    await page.locator('#account-route').selectOption('fixture-account');
    await page.waitForFunction(() => document.querySelector('#account-model').options.length === 2);
    assert.deepEqual(await page.locator('#account-model option').allTextContents(), ['Second model', 'First model']);
    await page.locator('#account-model').selectOption('second');
    await page.locator('#account-selection button').click();
    await page.waitForFunction(() => document.querySelector('#plan-welcome').open);
    assert.deepEqual(actions[0], { billing_route: 'chatgpt', account: 'fixture-account', model: 'second' });
    await page.getByRole('button', { name: 'Got it' }).click();
    await page.keyboard.press('Escape');
    await page.waitForFunction(() => !document.querySelector('#accounts-dialog').open);
    assert(!actions.includes('stop'), 'Escape in account settings must not interrupt an agent');
    status.accounts[0].paused = true;
    await page.locator('#open-accounts').click();
    await page.getByRole('button', { name: 'Resume requests' }).click();
    assert(actions.includes('resume'));
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    await page.waitForFunction(() => !document.querySelector('#accounts-error').hidden);
    assert.match(await page.locator('#accounts-error').textContent(), /revocation was not confirmed/);
    assert.equal(await page.locator('#account-route option[value="fixture-account"]').isDisabled(), true);
    assert.deepEqual(errors, []);
    if (process.env.BOID_SCREENSHOT) await page.screenshot({ path: process.env.BOID_SCREENSHOT });
    console.log('PASS accounts: explicit billing/model selection, welcome, plan label, Escape isolation, limit resume, sign-out warning; no browser errors.');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
