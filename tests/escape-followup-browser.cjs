// Real browser gestures with delayed cancellation and synthetic server history.
// No live agents, provider calls, or private transcripts.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {chromium} = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const entries = Array.from({length:180}, (_, i) => ({
  id:`e${i}`, kind:'assistant', text:`Message ${i}\n` + 'research output\n'.repeat(12),
}));
const view = (start = 0) => ({id:'a', name:'canary', cwd:'/tmp', status:'working',
  working_since_ms:Date.now()-5000, entries:entries.slice(start), pending:[],
  history:{start, end:entries.length, total:entries.length, seq:1}});
(async () => {
  const browser = await chromium.launch({headless:true,
    ...(process.env.CHROME_PATH ? {executablePath:process.env.CHROME_PATH} : {})});
  try {
    const page = await browser.newPage({viewport:{width:1100,height:800}});
    const errors=[]; page.on('pageerror', error => errors.push(error.message));
    let interruptRoute, interrupts=0, submissions=0;
    await page.addInitScript(() => {window.EventSource=class {constructor(){window.testEvents=this;}close(){}};});
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname === '/api/config') return route.fulfill({json:{cwd:'/tmp',agent_names:[]}});
      if (url.pathname.endsWith('/interrupt')) {interrupts++; interruptRoute=route; return;}
      if (url.pathname.endsWith('/messages')) {submissions++; return route.fulfill({json:{}});}
      if (url.pathname.startsWith('/api/')) return route.fulfill({json:[]});
      const file = path.join(root, url.pathname === '/' ? 'index.html' : url.pathname);
      return route.fulfill({body:fs.readFileSync(file), contentType:file.endsWith('.js') ? 'text/javascript' : file.endsWith('.css') ? 'text/css' : 'text/html'});
    });
    await page.goto('http://boid.test'); await page.waitForFunction(() => window.testEvents);
    const frames = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await page.evaluate(v => snapshot([v], []), view()); await frames();
    const input = page.locator('.composer textarea');
    await input.fill('Please continue with a narrower task.'); await frames();
    await input.press('Escape'); await frames();
    const immediate = await page.evaluate(() => {
      const ui=sessions.get('a');
      return {label:ui.activity.firstChild.textContent, titleAnimation:getComputedStyle(ui.title).animationName,
        status:ui.view.status, requested:ui.interruptRequested};
    });
    console.log('immediate', immediate);
    // Accept the request later; the actual worker is still working until its event.
    await page.waitForFunction(() => sessions.get('a').interruptRequested);
    for (let i=0; !interruptRoute && i<100; i++) await new Promise(r => setTimeout(r,5));
    assert.ok(interruptRoute);
    await interruptRoute.fulfill({status:202,json:{status:'interrupt_requested'}});
    await page.waitForFunction(() => !sessions.get('a').interrupting);
    await input.press('Escape'); assert.equal(interrupts,1);
    // Hiding the composer retains a small intentional gap. Then a reconnect
    // replaces the accumulated history with a recent tail before the follow-up.
    await input.fill(''); await page.evaluate(() => document.activeElement.blur());
    await page.mouse.move(0,0); await frames();
    const before = await page.evaluate(() => {
      const ui=sessions.get('a');return {top:ui.trace.scrollTop,bottom:messageBottom(ui),retained:ui.retainedPromptEnd};
    });
    await page.evaluate(v => {snapshot([v],[]);setStatus(sessions.get('a'),'stopped');}, view(145));
    await frames();
    await input.fill('Follow up after cancellation.'); await input.press('Enter');
    await page.waitForFunction(() => !sessions.get('a').sending);
    await page.evaluate(() => {
      const ui=sessions.get('a');
      putEntry(ui,{id:'followup',kind:'user',text:'Follow up after cancellation.',history_index:180});
      setStatus(ui,'working',Date.now());
      putEntry(ui,{id:'answer',kind:'assistant',text:'I will continue with the narrower task.',history_index:181});
    }); await frames();
    const after = await page.evaluate(() => {
      const ui=sessions.get('a'), trace=ui.trace.getBoundingClientRect(), last=ui.entries.get('answer').node.getBoundingClientRect();
      return {top:ui.trace.scrollTop,bottom:messageBottom(ui),retained:ui.retainedPromptEnd,
        gap:ui.trace.scrollTop-messageBottom(ui),clearance:ui.overlay.offsetHeight+8,
        lastVisible:last.bottom>trace.top && last.top<trace.bottom, label:ui.activity.firstChild.textContent};
    });
    console.log('follow-up', {before, after, submissions});
    if(process.env.BOID_SCREENSHOT) await page.screenshot({path:process.env.BOID_SCREENSHOT});
    assert.ok(after.lastVisible, 'follow-up response must remain visible after history shrinks');
    assert.ok(after.gap <= after.clearance+2, `stale blank area: ${after.gap}px`);
    assert.equal(immediate.label,'Interrupting');
    assert.equal(immediate.titleAnimation,'none');
    assert.equal(immediate.status,'working', 'do not claim server stopped before confirmation');
    assert.equal(after.label,'Working');
    // The same coordinate change can occur without reconnecting: an expanded
    // output or a provisional entry is replaced by shorter final content.
    await page.evaluate(() => putEntry(sessions.get('a'),{
      id:'provisional',kind:'assistant',text:'provisional output\n'.repeat(150),history_index:182,
    })); await frames();
    await input.fill('another follow-up'); await frames();
    await input.fill(''); await page.evaluate(() => document.activeElement.blur());
    await page.mouse.move(0,0); await frames();
    await page.evaluate(() => putEntry(sessions.get('a'),{
      id:'final',kind:'assistant',text:'Short final result',replaces:'provisional',history_index:183,
    })); await frames();
    assert.ok(await page.evaluate(() => {
      const ui=sessions.get('a'), last=ui.entries.get('final').node.getBoundingClientRect();
      return last.bottom>ui.trace.getBoundingClientRect().top
        && ui.trace.scrollTop-messageBottom(ui)<=ui.overlay.offsetHeight+10;
    }), 'superseding a tall entry must not retain its removed height');
    assert.deepEqual(errors,[]);
    console.log('PASS: immediate cancellation feedback, deduplication, follow-up visible after history replacement');
  } finally {await browser.close();}
})().catch(error => {console.error(error);process.exitCode=1;});
