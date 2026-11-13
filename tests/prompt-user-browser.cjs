// Real Chromium interaction with isolated API fixtures. No provider calls.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {chromium} = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const q = (id, question, blocking = false) => ({id, question, blocking, created_at_ms: Date.now(),
  wait_until_ms: blocking ? Date.now() + 300000 : null, wait_outcome: null, answer: null, call_ids: [id]});
const view = (id, questions) => ({id, name: id === 'a' ? 'canary' : 'swift', cwd: '/work/boid', status: 'working',
  working_since_ms: Date.now(), pending: [], questions, entries: [
    {id: `${id}-user`, kind: 'user', text: 'Prepare the next release.'},
    {id: `${id}-assistant`, kind: 'assistant', text: 'The changes are ready. I am checking the release notes and the staging configuration.'},
  ]});
(async () => {
  const browser = await chromium.launch({headless: true});
  try {
    const page = await browser.newPage({viewport: {width: 1200, height: 760}});
    const errors = [], answers = [];
    const screens = {revision: 1, count: 2, panes: {}};
    let rejectAnswer = false, seq = 100;
    let holdAnswer = false, releaseAnswer;
    const interrupts = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.addInitScript(() => {window.EventSource = class {constructor() {window.testEvents = this;} close() {}};});
    await page.route('**/*', async route => {
      const u = new URL(route.request().url());
      if (u.hostname !== 'boid.test') return route.abort();
      if (u.pathname.endsWith('/answer')) {
        const text = route.request().postDataJSON().text;
        if (rejectAnswer) return route.fulfill({status: 400, json: {error: 'Session input queue is full.'}});
        answers.push({path: u.pathname, text});
        if (holdAnswer) await new Promise(resolve => { releaseAnswer = resolve; });
        return route.fulfill({status: 202, json: {message_id: `answer-${answers.length}`, status: 'queued'}});
      }
      if (u.pathname.endsWith('/interrupt')) { interrupts.push(u.pathname); return route.fulfill({json: {status: 'interrupt_requested'}}); }
      if (u.pathname === '/api/config') return route.fulfill({json: {cwd: '/work/boid', next_agent_name: 'canary', agent_names: ['canary', 'swift']}});
      if (u.pathname === '/api/screens') return route.fulfill({json: screens});
      if (u.pathname.startsWith('/api/')) return route.fulfill({json: []});
      const file = path.join(root, u.pathname === '/' ? 'index.html' : u.pathname);
      return route.fulfill({body: fs.readFileSync(file), contentType: file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html'});
    });
    await page.goto('http://boid.test');
    await page.waitForFunction(() => window.testEvents);
    await page.evaluate(() => testEvents.onopen());
    const blocking = q('release', 'May I deploy the release to staging, or should I keep it ready for review?', true);
    const optional = q('notes', 'Would shorter release notes be more useful?');
    const second = q('target', 'Which project should I use for the next example?');
    await page.evaluate(views => snapshot(views, []), [view('a', [blocking]), view('b', [optional, second])]);
    const a = page.locator('[data-pane-key="agent:a"]'), b = page.locator('[data-pane-key="agent:b"]');
    assert.equal(await a.locator('.question-panel').isVisible(), true);
    assert.equal(await a.evaluate(e => e.classList.contains('needs-answer')), true);
    assert.equal(await b.locator('.question-panel').isVisible(), false);
    assert.match(await a.locator('.working-label').innerText(), /Waiting for reply/);
    // Opening a question leaves an ordinary message draft alone.
    await b.locator('.composer textarea').fill('Keep this ordinary message draft');
    await b.locator('.questions-toggle').click();
    await b.locator('.question-reply').fill('Shorter, with the compatibility notes retained.');
    await b.locator('.question-select').selectOption('target');
    await b.locator('.question-reply').fill('Use the demo project');
    await b.locator('.question-select').selectOption('notes');
    assert.equal(await b.locator('.question-reply').inputValue(), 'Shorter, with the compatibility notes retained.');
    assert.equal(await b.locator('.composer textarea').inputValue(), 'Keep this ordinary message draft');
    // A reconnect and incoming question preserve both the selected question and its draft.
    await b.locator('.question-reply').focus();
    await page.evaluate(views => snapshot(views, []), [view('a', [blocking]), view('b', [optional, second])]);
    assert.equal(await b.locator('.question-reply').inputValue(), 'Shorter, with the compatibility notes retained.');
    assert.equal(await b.locator('.question-reply').evaluate(e => e === document.activeElement), true);
    // Switching screens discards panes, but preserves questions and their separate drafts.
    await page.locator('#screens [data-screen="2"]').click();
    await page.evaluate(() => snapshot([], [], ['a', 'b']));
    assert.equal(await page.locator('.agent-pane').count(), 0);
    await page.locator('#screens [data-screen="1"]').click();
    await page.evaluate(views => snapshot(views, []), [view('a', [blocking]), view('b', [optional, second])]);
    await page.waitForFunction(() => !sessions.get('b').restoringScreen);
    assert.equal(await b.locator('.question-panel').isVisible(), true);
    assert.equal(await b.locator('.question-reply').inputValue(), 'Shorter, with the compatibility notes retained.');
    await b.locator('.question-select').selectOption('target');
    assert.equal(await b.locator('.question-reply').inputValue(), 'Use the demo project');
    await b.locator('.question-select').selectOption('notes');
    assert.equal(await b.locator('.composer textarea').inputValue(), 'Keep this ordinary message draft');
    assert.equal(await a.evaluate(e => e.classList.contains('needs-answer')), true);
    if (process.env.BOID_SCREENSHOT_DIR) await page.screenshot({path: path.join(process.env.BOID_SCREENSHOT_DIR, 'prompt-user.png')});
    // The answer remains editable after a server rejection.
    rejectAnswer = true;
    await b.locator('.question-submit').click();
    await b.locator('.question-error').waitFor({state: 'visible'});
    assert.match(await b.locator('.question-error').innerText(), /queue is full/);
    assert.equal(await b.locator('.question-reply').inputValue(), 'Shorter, with the compatibility notes retained.');
    rejectAnswer = false;
    await b.locator('.question-reply').press('Enter');
    await page.waitForFunction(() => sessions.get('b').questions.length === 1);
    assert.equal(answers.length, 1);
    assert.equal(answers[0].path, '/api/sessions/b/questions/notes/answer');
    assert.equal(await b.locator('.question-reply').inputValue(), 'Use the demo project');
    // A real stream event displays the user answer in the transcript.
    await page.evaluate(message => testEvents.onmessage({data: JSON.stringify(message)}), {
      seq: ++seq, type: 'entry', session_id: 'b', entry_index: 2,
      entry: {id: 'answer-1', kind: 'user', text: `Reply to: ${optional.question}\n\n${answers[0].text}`},
    });
    await b.locator('[data-entry-id="answer-1"]').waitFor();
    assert.match(await b.locator('[data-entry-id="answer-1"]').innerText(), /Shorter, with the compatibility notes retained/);
    // Expiry removes the urgent pane treatment but preserves a late reply draft.
    await a.locator('.question-reply').fill('Staging only, please.');
    await page.evaluate(message => testEvents.onmessage({data: JSON.stringify(message)}), {
      seq: ++seq, type: 'questions', session_id: 'a', questions: [{...blocking, wait_outcome: 'timed_out'}],
    });
    assert.equal(await a.evaluate(e => e.classList.contains('needs-answer')), false);
    assert.match(await a.locator('.question-meta').innerText(), /Wait ended/);
    assert.equal(await a.locator('.question-reply').inputValue(), 'Staging only, please.');
    await a.locator('.question-reply').press('Enter');
    await page.waitForFunction(() => sessions.get('a').questions.length === 0);
    assert.equal(await a.locator('.questions-box').isVisible(), false);
    assert.equal(answers.length, 2);
    // Important questions remain usable in a small pane without hiding the stream.
    await page.setViewportSize({width: 660, height: 560});
    const pending = q('small', 'Which configuration should I use?', true);
    await page.evaluate(question => setQuestions(sessions.get('a'), [question]), pending);
    assert.equal(await a.locator('.question-reply').isVisible(), true);
    const panel = await a.locator('.questions-box').boundingBox();
    const pane = await a.boundingBox();
    assert.ok(panel.height <= pane.height * .56);
    assert.ok(await a.locator('.trace').evaluate(e => e.clientHeight) > 60);
    // Escape must follow an in-flight answer, even after the pane was recreated.
    holdAnswer = true;
    await a.locator('.question-reply').fill('Use staging');
    await a.locator('.question-submit').click();
    for (let i = 0; !releaseAnswer; i++) { assert.ok(i < 100); await new Promise(resolve => setTimeout(resolve, 10)); }
    await page.locator('#screens [data-screen="2"]').click();
    await page.evaluate(() => snapshot([], [], ['a']));
    await page.locator('#screens [data-screen="1"]').click();
    await page.evaluate(v => snapshot([v], []), view('a', [pending]));
    await a.locator('.question-reply').press('Escape');
    await page.waitForFunction(() => sessions.get('a').interruptRequested);
    assert.equal(interrupts.length, 0, 'interrupt cannot overtake the answer POST');
    releaseAnswer();
    for (let i = 0; !interrupts.length; i++) { assert.ok(i < 100); await new Promise(resolve => setTimeout(resolve, 10)); }
    assert.equal(interrupts.length, 1);
    assert.deepEqual(errors, []);
    console.log('PASS: urgent pane, question picker, drafts, reconnect, screen switches, failed submission, reply stream, timeout, late answer, narrow pane, Escape ordering across remount');
  } finally {await browser.close();}
})().catch(error => {console.error(error); process.exitCode = 1;});
