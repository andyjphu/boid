// Real keyboard and focus behavior against shipped UI; all server traffic mocked.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
(async () => {
  const browser = await chromium.launch({headless:true,...(process.env.CHROME_PATH ? {executablePath:process.env.CHROME_PATH} : {})});
  try {
    const page = await browser.newPage({viewport:{width:1200,height:900}});
    page.setDefaultTimeout(5000);
    const errors = [], sent = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.addInitScript(() => {
      window.EventSource = class { constructor(){window.testEvents=this;} close(){} };
      window.WebSocket = class { static OPEN=1; constructor(){this.readyState=1;} send(){} close(){} };
    });
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.hostname !== 'boid.test') return route.abort();
      if (url.pathname.endsWith('/messages')) sent.push(route.request().postDataJSON());
      if (url.pathname === '/api/config') return route.fulfill({json:{cwd:'/tmp',agent_names:[]}});
      if (url.pathname.startsWith('/api/')) return route.fulfill({json:[]});
      const file=path.join(root,url.pathname==='/'?'index.html':url.pathname);
      return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
    });
    const frames = () => page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
    const input = id => page.locator(`.subpane[data-pane-key="agent:${id}"] .composer textarea`);
    const focused = () => page.evaluate(() => [...sessions.values()].find(ui => ui.input === document.activeElement)?.id);
    await page.goto('http://boid.test');
    await page.waitForFunction(() => window.testEvents?.onmessage);
    await page.evaluate(() => testEvents.onmessage({data:JSON.stringify({type:'snapshot',seq:1,
      sessions:['a','b','c','hidden'].map(id=>({id,name:id,cwd:'/tmp',status:'idle',pending:[],entries:[{id:`${id}-1`,kind:'assistant',text:'Existing agent output'}]})),
      terminals:[{id:'t',name:'#1',cwd:'/tmp',running:true}]
    })}));
    await frames();
    await page.evaluate(() => {
      moveLocalSubpane(sessions.get('hidden'), sessions.get('a').group);
      selectSubpane(sessions.get('a'));
      reorderPane('terminal:t','agent:b',false);
    });
    await frames();
    const positions = await page.evaluate(() => [...paneGroups.values()].map(g => ({key:g.record.id,top:g.pane.getBoundingClientRect().top,left:g.pane.getBoundingClientRect().left})));
    assert.deepEqual(positions.sort((a,b)=>a.top-b.top||a.left-b.left).map(p=>p.key),['agent:a','terminal:t','agent:b','agent:c']);
    await input('a').fill('draft A');
    await input('a').press('Tab'); assert.equal(await focused(),'b');
    await page.keyboard.type('draft B');
    await page.keyboard.press('Tab'); assert.equal(await focused(),'c');
    await page.keyboard.type('draft C');
    await page.keyboard.press('Tab'); assert.equal(await focused(),'a');
    await page.keyboard.press('Shift+Tab'); assert.equal(await focused(),'c');
    for (const id of ['a','b','c']) assert.equal(await input(id).inputValue(),`draft ${id.toUpperCase()}`);
    assert.equal(await page.evaluate(()=>sessions.get('hidden').inactive),true);
    await input('a').fill('/cl');
    await page.keyboard.press('Tab'); assert.equal(await focused(),'b');
    assert.equal(await input('a').inputValue(),'/cl','Tab must not complete or submit a slash command');
    await page.locator('.pane-group[data-pane-key="agent:a"] .pane-header').click({position:{x:400,y:10}});
    await page.keyboard.press('Tab'); assert.equal(await focused(),'b','clicking blank pane header selects that pane');
    await page.locator('.subpane[data-pane-key="agent:b"] .trace').click();
    await page.keyboard.press('Tab'); assert.equal(await focused(),'c','clicking trace selects that pane');
    // Follow screen geometry after reordering, not session creation/name order.
    await page.evaluate(()=>reorderPane('agent:c','agent:a',false)); await frames();
    await input('c').focus(); await page.keyboard.press('Tab'); assert.equal(await focused(),'a');
    await page.setViewportSize({width:600,height:900}); await frames();
    await input('a').focus(); await page.keyboard.press('Tab'); assert.equal(await focused(),'b','single-column row navigation');
    // Picker dialogs keep their own keyboard traversal.
    await page.evaluate(()=>{
      const ui=sessions.get('a'); selectSubpane(ui); ui.modelPicker.hidden=false;
      ui.modelPicker.replaceChildren(...['one','two'].map(text=>{const b=document.createElement('button');b.className='model-option';b.textContent=text;return b;}));
      ui.modelPicker.firstChild.focus();
    });
    await page.keyboard.press('Tab');
    assert.equal(await page.evaluate(()=>document.activeElement.textContent),'two');
    await page.evaluate(()=>sessions.get('a').modelPicker.hidden=true);
    assert.deepEqual(sent,[],'navigation must not send any draft');
    assert.deepEqual(errors,[]);
    console.log('PASS: agent Tab order, wrapping, reverse, drafts, slash precedence, pane clicks, hidden tabs, terminal skipping, reordering, responsive layout, picker isolation; zero message sends.');
  } finally { await browser.close(); }
})().catch(error=>{console.error(error);process.exitCode=1;});
