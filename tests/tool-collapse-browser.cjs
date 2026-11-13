// Real browser UI with isolated API responses; no live agents or provider calls.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {chromium} = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const skillText = '# Humanizer\n\n' + 'Keep the meaning. Write plainly.\n'.repeat(400) + '\nEND OF SKILL';
const call = {id:'call',kind:'tool_call',text:'read_skill (skill-1)\n{"name":"bundled:humanizer"}'};
const result = {id:'result',kind:'tool_result',text:'read_skill (skill-1)\n'+JSON.stringify({directory:null,name:'humanizer',text:skillText})};
const partial = {...result,text:result.text.slice(0,8192),text_complete:false};
const view = entries => ({id:'a',name:'canary',cwd:'/tmp',status:'idle',pending:[],entries});
(async()=>{
  const browser=await chromium.launch({headless:true});
  try {
    const page=await browser.newPage({viewport:{width:950,height:700}});
    const errors=[];let fullReads=0;
    page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(()=>{window.EventSource=class{constructor(){window.testEvents=this;}close(){}};});
    await page.route('**/*',route=>{
      const u=new URL(route.request().url());if(u.hostname!=='boid.test')return route.abort();
      if(u.pathname==='/api/config')return route.fulfill({json:{cwd:'/tmp',next_agent_name:'swift',agent_names:['canary','swift']}});
      if(u.pathname==='/api/sessions/a/history'){
        assert.equal(u.searchParams.get('entry'),'result');fullReads++;
        return route.fulfill({json:{...view([result]),history:{start:2,end:3,total:4,seq:1}}});
      }
      if(u.pathname.startsWith('/api/'))return route.fulfill({json:[]});
      const file=path.join(root,u.pathname==='/'?'index.html':u.pathname);
      return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
    });
    await page.goto('http://boid.test');await page.waitForFunction(()=>window.testEvents);
    const frames=()=>page.evaluate(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))));
    await page.evaluate(v=>snapshot([v],[]),view([{id:'user',kind:'user',text:'Read the humanizer skill.'},call,partial,{id:'answer',kind:'assistant',text:'Ready.'}]));await frames();
    const summary=page.locator('[data-entry-id="call"] summary');
    const output=page.locator('[data-entry-id="result"]');
    assert.match(await summary.innerText(),/read_skill\s+bundled:humanizer/);
    assert.equal(await output.isVisible(),false);
    assert.equal(fullReads,0,'collapsed history must not fetch large output');
    assert.equal(await page.locator('.trace').innerText().then(t=>t.includes('directory')),false);
    assert.ok(await page.locator('[data-entry-id="call"]').evaluate(e=>e.getBoundingClientRect().height)<50);
    if(process.env.BOID_SCREENSHOT_DIR)await page.screenshot({path:path.join(process.env.BOID_SCREENSHOT_DIR,'tool-collapsed.png')});
    // Native keyboard interaction opens the same disclosure as a pointer click.
    await summary.focus();await page.keyboard.press('Enter');await frames();
    assert.equal(await output.isVisible(),true);
    assert.equal(await output.locator('button').innerText(),'Load full result');
    assert.equal(await output.innerText().then(t=>t.includes('directory')),false);
    await output.locator('button').click();await frames();
    assert.equal(fullReads,1);
    assert.equal(await output.locator('.tool-text').textContent(),skillText);
    assert.equal(await page.locator('[data-entry-id="call"] details').getAttribute('open'),'');
    // Expanded skill text is decoded and selectable, without raw JSON escaping.
    await page.evaluate(()=>{const ui=sessions.get('a');ui.follow=false;scrollTrace(ui,0);});
    if(process.env.BOID_SCREENSHOT_DIR)await page.screenshot({path:path.join(process.env.BOID_SCREENSHOT_DIR,'tool-expanded.png')});
    await page.evaluate(()=>{const node=document.querySelector('[data-entry-id="result"] .tool-text');const r=document.createRange();r.selectNodeContents(node);getSelection().removeAllRanges();getSelection().addRange(r);});
    assert.equal(await page.evaluate(()=>getSelection().toString()),skillText);
    await page.evaluate(()=>getSelection().removeAllRanges());
    await summary.focus();await page.keyboard.press('Space');await frames();
    assert.equal(await output.isVisible(),false);
    // A live replacement of the result does not reopen it.
    await page.evaluate(e=>putEntry(sessions.get('a'),e),{...result,text:result.text+' '});await frames();
    assert.equal(await output.isVisible(),false);
    // Interleaved messages retain their sequence when the result expands.
    await page.evaluate(v=>{removeSession(sessions.get("a"));snapshot([v],[]);},view([call,{id:'between',kind:'assistant',text:'Interleaved update'},result]));await frames();
    await page.locator('[data-entry-id="call"] summary').click();await frames();
    assert.deepEqual(await page.locator('.entry').evaluateAll(nodes=>nodes.filter(n=>!n.hidden).map(n=>n.dataset.entryId)),['call','between','result']);
    assert.equal(await page.locator('[data-entry-id="between"]').innerText(),'Interleaved update');
    // A result without a loaded call has its own compact, keyboard-accessible row.
    await page.evaluate(v=>{removeSession(sessions.get("a"));snapshot([v],[]);},view([partial]));await frames();
    assert.equal(await page.locator('[data-entry-id="result"] summary').innerText(),'read_skill');
    assert.equal(await page.locator('[data-entry-id="result"] button').isVisible(),false);
    await page.locator('[data-entry-id="result"] summary').click();await frames();
    assert.equal(await page.locator('[data-entry-id="result"] button').isVisible(),true);
    // Moving the call outside the virtual window transfers the disclosure to its result.
    const history=Array.from({length:95},(_,i)=>({id:`history-${i}`,kind:'assistant',text:`Message ${i}`}));
    history[64]=call;history[94]=result;
    await page.evaluate(v=>{removeSession(sessions.get("a"));snapshot([v],[]);},{...view(history),history:{start:0,end:95,total:95,seq:1}});await frames();
    assert.equal(await page.locator('[data-entry-id="call"] summary').isVisible(),true);
    assert.equal(await page.locator('[data-entry-id="result"]').isVisible(),false);
    await page.evaluate(()=>{const ui=sessions.get('a');for(let i=95;i<105;i++)applyTranscriptEvent(ui,{type:'entry',seq:i,entry_index:i,entry:{id:`history-${i}`,kind:'assistant',text:`Message ${i}`}});});await frames();
    assert.equal(await page.locator('[data-entry-id="call"]').count(),0);
    assert.equal(await page.locator('[data-entry-id="result"] summary').isVisible(),true);
    assert.equal(await page.locator('[data-entry-id="result"] .tool-output').isVisible(),false);
    await page.locator('[data-entry-id="result"] summary').click();await frames();
    assert.equal(await page.locator('[data-entry-id="result"] .tool-text').textContent(),skillText);
    assert.deepEqual(errors,[]);
    console.log('PASS: partial history stays collapsed, skill summary, keyboard toggles, explicit full fetch, decoded selectable text, live update, interleaving, orphan result, virtual-window ownership');
  }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
