// Real browser, mocked server events. No provider or production requests.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 900, height: 650 } });
    const root = path.resolve(__dirname, '../src/web'), errors = [];
    let seq = 0;
    const agent = { id:'canary', name:'canary', status:'working', cwd:'/tmp', pending:[],
      working_since_ms:Date.now(), retry_at_ms:Date.now()+325000,
      profile:{model:'fixture'}, entries:[{id:'retry',kind:'notice',text:'Request interrupted; retrying (2/4).'}] };
    const screens = { revision:1,count:1,panes:{} };
    const emit = message => page.evaluate(m => testEvents.onmessage({data:JSON.stringify(m)}), {...message,seq:++seq});
    await page.exposeFunction('snapshot', () => emit({type:'snapshot',screens,catalog:[agent.id],sessions:[agent],terminals:[]}));
    await page.addInitScript(() => { window.EventSource = class {
      constructor(){window.testEvents=this;setTimeout(()=>{this.onopen?.();window.snapshot();},0);} close(){}
    }; });
    page.on('pageerror', e => errors.push(e.message));
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if(url.hostname !== 'boid.test') return route.abort();
      if(url.pathname === '/api/config') return route.fulfill({json:{cwd:'/tmp',agent_names:[]}});
      if(url.pathname === '/api/screens') return route.fulfill({json:screens});
      if(url.pathname.startsWith('/api/')) return route.fulfill({json:[]});
      const file=path.join(root,url.pathname==='/'?'index.html':url.pathname);
      return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
    });
    await page.goto('http://boid.test/');
    const activity=page.locator('[data-pane-key="agent:canary"] .working-status');
    await page.waitForFunction(()=>sessions.get('canary')?.activity.textContent.includes('Retrying'));
    assert.match(await activity.textContent(),/5m 2[0-5]s/);
    assert.match(await activity.textContent(),/Esc to interrupt/);
    const before=await activity.textContent();
    await page.waitForFunction(old=>sessions.get('canary').activity.textContent!==old,before);
    if(process.env.SCREENSHOT) await page.screenshot({path:process.env.SCREENSHOT});
    await page.reload();
    await page.waitForFunction(()=>sessions.get('canary')?.activity.textContent.includes('Retrying'));
    await emit({type:'status',session_id:'canary',status:'working',working_since_ms:agent.working_since_ms,retry_at_ms:null});
    await page.waitForFunction(()=>sessions.get('canary').activity.textContent.startsWith('Working'));
    await emit({type:'status',session_id:'canary',status:'working',working_since_ms:agent.working_since_ms,retry_at_ms:Date.now()+325000});
    await page.waitForFunction(()=>sessions.get('canary').activity.textContent.includes('Retrying'));
    await emit({type:'status',session_id:'canary',status:'stopped',working_since_ms:null,retry_at_ms:null});
    assert.doesNotMatch(await activity.textContent(),/Retrying/);
    assert.deepEqual(errors,[]);
    console.log('PASS retry countdown: five-minute wait, one-second decrement, Escape hint, refresh restore, request-start clear, Stop clear; mocked transport only');
  } finally { await browser.close(); }
})().catch(e=>{console.error(e);process.exitCode=1;});
