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
    const profiles = { agent: { billing_route: 'api_key', model: 'first', reasoning_effort: 'medium' }, other: { billing_route: 'api_key', model: 'first' } };
    const status = { billing_route: 'api_key', selected_account: null, model: 'first', api_key_available: true,
      accounts: [{ id: 'fixture-account', label: 'fixture@example.invalid · account', connected: true, plan_enabled: true, paused: false }] };
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => { window.EventSource = class { constructor() { window.testEvents = this; } close() {} }; });
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname === '/api/config') return route.fulfill({ json: { cwd: '/tmp', next_agent_name: 'canary', agent_names: ['canary'] } });
      if (url.pathname === '/api/accounts/api-key') {
        assert.equal(route.request().postDataJSON().key, 'synthetic-browser-key');
        status.api_key_available = true; actions.push('key-saved'); return route.fulfill({ json: { ok: true } });
      }
      if (url.pathname === '/api/sessions' && route.request().method() === 'POST') return route.fulfill({ status: 400, json: { error: 'Use /login to connect an API key or ChatGPT subscription before creating an agent.' } });
      if (url.pathname.endsWith('/messages')) { actions.push('model-message'); return route.fulfill({ json: {} }); }
      if (url.pathname === '/api/terminals' && route.request().method() === 'POST') return route.fulfill({ status: 500, json: { error: 'Fixture terminal failure' } });
      if (url.pathname === '/api/accounts') return route.fulfill({ json: status });
      if (['/api/sessions/agent', '/api/sessions/agent/history'].includes(url.pathname)) return route.fulfill({ json: { id: 'agent', profile: profiles.agent } });
      if (url.pathname === '/api/sessions/agent/model') {
        const body = route.request().postDataJSON(); actions.push({ action: 'model', ...body });
        profiles.agent.model = body.model; return route.fulfill({ json: { profile: profiles.agent } });
      }
      if (url.pathname.endsWith('/models')) return route.fulfill({ json: { models: [{ slug: 'second', display_name: 'Second model' }, { slug: 'first', display_name: 'First model' }] } });
      if (url.pathname === '/api/accounts/select') {
        const body = route.request().postDataJSON(); actions.push(body);
        let profile;
        if (body.session_id) {
          profile = profiles[body.session_id]; Object.assign(profile, { billing_route: body.billing_route, chatgpt_account: body.account });
        } else {
          Object.assign(status, { billing_route: body.billing_route, selected_account: body.account });
          profile = { billing_route: body.billing_route, model: status.model };
        }
        return route.fulfill({ json: { welcome: body.billing_route === 'chatgpt', profile } });
      }
      if (url.pathname.endsWith('/logout')) {
        status.accounts[0].connected = false;
        return route.fulfill({ json: { revocation_confirmed: false, message: 'Signed out locally; remote revocation was not confirmed.' } });
      }
      if (url.pathname.endsWith('/resume')) {
        status.accounts[0].paused = false; actions.push('resume'); return route.fulfill({ json: {} });
      }
      if (url.pathname.endsWith('/interrupt')) { actions.push('stop'); return route.fulfill({ json: {} }); }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ json: [] });
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({ body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html' });
    });
    await page.goto('http://boid.test');
    await page.waitForFunction(() => window.testEvents);
    await page.evaluate(() => snapshot([{ id: 'agent', name: 'canary', cwd: '/tmp', status: 'idle', entries: [], pending: [], profile: { billing_route: 'api_key', model: 'first' } }], []));
    assert.equal(await page.locator('.billing-route').count(), 0);
    assert.equal(await page.locator('#open-accounts, #accounts-dialog').count(), 0);
    const composer = page.locator('[data-pane-key="agent:agent"] .composer textarea');
    async function command() {
      await composer.fill('/lo'); await composer.press('Tab'); assert.equal(await composer.inputValue(), '/login');
      await composer.press('Enter'); await page.locator('#provider-login').waitFor({ state: 'visible' });
      await page.waitForFunction(() => loginStatus);
    }
    await command();
    assert(!actions.includes('model-message'), '/login is a local command, not a model prompt');
    await page.getByRole('button', { name: 'ChatGPT subscription', exact: true }).click();
    assert.equal(await page.locator('#subscription-model').count(), 0);
    assert.equal(await page.getByText('Manage usage', { exact: true }).count(), 0);
    await page.getByRole('button', { name: 'Use subscription', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('#plan-welcome').open);
    assert.deepEqual(actions[0], { session_id: 'agent', billing_route: 'chatgpt', account: 'fixture-account' });
    await page.getByRole('button', { name: 'Got it' }).click();
    await page.keyboard.press('Escape');
    await page.waitForFunction(() => document.querySelector('#provider-login').hidden);
    assert(!actions.includes('stop'), 'Escape in /login must not interrupt an agent');
    assert.equal(await composer.isVisible(), true);
    assert.equal(profiles.agent.billing_route, 'chatgpt'); assert.equal(profiles.other.billing_route, 'api_key');
    assert.equal(status.billing_route, 'api_key', 'Pane selection must not change the default');
    assert.equal(await page.locator('.billing-route').count(), 0);
    await composer.fill('/model'); await composer.press('Enter');
    await page.getByRole('option', { name: 'Second model', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('.model-picker').hidden);
    assert.equal(profiles.agent.model, 'second');
    assert.equal(await page.evaluate(() => sessions.get('agent').view.profile.model), 'second');
    status.accounts[0].paused = true;
    await command();
    await page.getByRole('button', { name: 'ChatGPT subscription', exact: true }).click();
    await page.getByRole('button', { name: 'Resume requests' }).click();
    assert(actions.includes('resume'));
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    await page.waitForFunction(() => !document.querySelector('#provider-login-error').hidden);
    assert.match(await page.locator('#provider-login-error').textContent(), /revocation was not confirmed/);
    assert.equal(await page.getByRole('button', { name: 'Use subscription', exact: true }).isDisabled(), true);
    await page.keyboard.press('Escape');
    status.accounts = []; status.api_key_available = false; status.billing_route = 'api_key';
    await page.locator('#add-agent').click();
    await page.getByRole('button', { name: 'Connect provider', exact: true }).click();
    await page.waitForFunction(() => loginStatus && !loginStatus.api_key_available);
    await page.getByRole('button', { name: 'API key', exact: true }).click();
    await page.locator('#provider-api-key').fill('synthetic-browser-key');
    await page.getByRole('button', { name: 'Use API key', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('#provider-login-status').textContent.includes('API key connected'));
    assert(actions.includes('key-saved')); assert.equal(actions.at(-1).billing_route, 'api_key'); assert.equal(actions.at(-1).session_id, undefined);
    assert.equal(await page.locator('#provider-api-key').inputValue(), '');
    assert.equal(await page.evaluate(() => JSON.stringify({ ...sessionStorage, ...localStorage }).includes('synthetic-browser-key')), false);
    assert(!actions.includes('model-message'));
    if (process.env.BOID_SCREENSHOT) await page.screenshot({ path: process.env.BOID_SCREENSHOT });
    await page.keyboard.press('Escape');
    await page.locator('#add-terminal').click();
    await page.waitForFunction(() => !document.querySelector('#add-terminal').disabled);
    assert.match(await page.locator('#workspace-error').textContent(), /Fixture terminal failure/);
    assert.deepEqual(errors, []);
    console.log('PASS /login: slash interception, API/subscription choice, first-key setup without model calls or browser credential storage, model selection, Escape, usage resume, sign-out warning, terminal-button recovery.');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
