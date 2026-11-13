// Real Chrome, mocked server and terminal sockets. No provider calls or shell input.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const view = id => ({ id, name: id, cwd: '/mock/home', status: 'idle', pending: [], entries: Array.from({length: 80}, (_, i) => ({id:`${id}-${i}`,kind:'assistant',text:`${id} message ${i}: `+'reading '.repeat(55)})) });
(async () => {
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 850 } });
    const errors = [], mutations = [], posts = []; let seq = 0, created = 0;
    const agents = [view('a'),view('b')], archived = [{...view('old'),archived:true}], terminals = [{id:'t',name:'#1',cwd:'/tmp',running:true}];
    const screens = {revision:1,count:2,panes:{}};
    page.on('pageerror', error => errors.push(error.message));
    const frames = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const emit = async message => { await page.evaluate(message => testEvents.onmessage({data:JSON.stringify(message)}), {...message,seq:++seq}); await frames(); };
    await page.exposeFunction('fixtureSnapshot', async () => {
      const screen = await page.evaluate(() => Number(new URL(testEvents.url,location.origin).searchParams.get('screen')));
      await emit({type:'snapshot',screens,catalog:agents.map(a=>a.id),sessions:agents.filter(a=>(screens.panes[`agent:${a.id}`]||1)===screen),terminals});
    });
    await page.addInitScript(() => {
      window.EventSource = class { constructor(url) {this.url=url;window.testEvents=this;setTimeout(()=>{this.onopen?.();window.fixtureSnapshot();},0)} close(){} };
      window.testSockets=[];
      window.WebSocket=class {static OPEN=1;constructor(url){this.url=url;this.readyState=1;testSockets.push(this)}close(){this.readyState=3}send(){} };
    });
    await page.route('**/*', async route => {
      const url=new URL(route.request().url()), method=route.request().method();
      if(url.hostname!=='boid.test')return route.abort();
      if(method!=='GET')mutations.push({path:url.pathname,method,body:route.request().postDataJSON()});
      if(url.pathname==='/api/config')return route.fulfill({json:{cwd:'/tmp',agent_names:['wren','finch']}});
      if(url.pathname==='/api/screens')return route.fulfill({json:screens});
      if(url.pathname.endsWith('/messages')){posts.push(route);return;}
      if(url.pathname==='/api/sessions/archived')return route.fulfill({json:archived});
      if(url.pathname==='/api/sessions'&&method==='POST'){
        assert.deepEqual(route.request().postDataJSON(),{name:'',cwd:'~'});
        const agent=view(`wren-${++created}`);agents.push(agent);screens.panes[`agent:${agent.id}`]=Number(url.searchParams.get('screen')||1);
        await emit({type:'session',session:agent});return route.fulfill({json:agent});
      }
      if(url.pathname==='/api/sessions')return route.fulfill({json:agents});
      if(url.pathname.endsWith('/reopen')){const a={...archived.shift(),archived:false};agents.push(a);await emit({type:'session',session:a});return route.fulfill({json:a});}
      if(url.pathname.endsWith('/history')){const agent=agents.find(a=>url.pathname.includes(`/${a.id}/`));return route.fulfill({json:url.searchParams.has('around')?{...agent,history:{start:0,end:agent.entries.length,total:agent.entries.length,seq}}:agent});}
      if(url.pathname.startsWith('/api/sessions/')&&method==='DELETE'){
        const id=url.pathname.split('/').at(-1);agents.splice(agents.findIndex(a=>a.id===id),1);await emit({type:'closed',session_id:id});return route.fulfill({json:{}});
      }
      if(url.pathname==='/api/terminals'&&method==='POST'){assert.equal(route.request().postDataJSON().cwd,'/tmp');const t={id:'t2',name:'#2',cwd:'/tmp',running:true};terminals.push(t);return route.fulfill({json:t});}
      if(url.pathname.startsWith('/api/'))return route.fulfill({json:[]});
      const file=path.join(root,url.pathname==='/'?'index.html':url.pathname);
      return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
    });
    const tab = key => page.locator(`.pane-tab[data-subpane-key="${key}"]`);
    const group = key => page.locator(`.pane-group[data-pane-key="${key}"]`);
    const input = id => page.locator(`.subpane[data-pane-key="agent:${id}"] .composer textarea`);
    const order = key => group(key).locator('.pane-tab').evaluateAll(nodes=>nodes.map(node=>node.dataset.subpaneKey));
    const until = async check => {for(let i=0;!check();i++){assert.ok(i<200);await new Promise(resolve=>setTimeout(resolve,10));}};
    await page.goto('http://boid.test');await page.waitForFunction(()=>sessions.size===2&&terminals.size===1);await frames();
    assert.equal(await page.locator('.pane-group').count(),3);assert.equal(await page.locator('.pane-cwd:visible').count(),0);
    await input('a').fill('Draft retained in original session');
    await page.evaluate(()=>{window.originalA=sessions.get('a');window.originalSocket=testSockets[0];});
    await tab('agent:b').dragTo(group('agent:a').locator('.subpane-add'));await frames();
    assert.deepEqual(await order('agent:a'),['agent:a','agent:b']);assert.equal(await page.locator('.pane-group').count(),2);
    await tab('agent:a').click();assert.equal(await input('a').inputValue(),'Draft retained in original session');
    // Old peer messages and urgent questions remain reachable behind inactive tabs.
    await page.evaluate(() => {
      const b=sessions.get('b');
      mergeHistory(b,{entries:[...b.entries.values()].slice(60).map(item=>({id:item.id,kind:item.kind,text:item.text})),history:{start:60,end:80,total:80,seq:0}},true);
    });
    await page.evaluate(()=>locateCommunication(sessions.get('a'),{id:'b-0',recipient_id:'b',sender_id:'a',sender_entry:'a-0'},'b'));
    await page.waitForFunction(()=>document.activeElement?.dataset.entryId==='b-0');
    assert.equal(await tab('agent:b').getAttribute('aria-selected'),'true');
    await tab('agent:a').click();
    await page.evaluate(()=>setQuestions(sessions.get('b'),[{id:'q',question:'Review this',blocking:true,wait_until_ms:Date.now()+60000,wait_outcome:null,answer:null}]));
    assert.equal(await tab('agent:b').evaluate(node=>node.classList.contains('needs-answer')),true,'inactive urgent tab stays discoverable');
    assert.equal(await group('agent:a').evaluate(node=>node.classList.contains('needs-answer')),true);
    await page.evaluate(()=>setQuestions(sessions.get('b'),[]));
    assert.equal(await tab('agent:b').locator('.pane-title').getAttribute('tabindex'),'-1');
    await tab('agent:a').focus();await page.keyboard.press('ArrowRight');
    assert.equal(await tab('agent:b').getAttribute('aria-selected'),'true');
    assert.equal(await page.evaluate(()=>selected===sessions.get('b').pane&&!sessions.get('b').inactive),true);
    await tab('agent:a').click();

    const anchor=await page.evaluate(()=>{const ui=sessions.get('a');ui.follow=false;scrollTrace(ui,300);rememberPosition(ui);return {id:ui.anchor.node.dataset.entryId,offset:ui.anchor.offset};});
    await tab('agent:b').click();await emit({type:'delta',session_id:'a',entry_id:'a-79',text:'Hidden update'});
    assert.equal(await page.evaluate(()=>sessions.get('a')===originalA),true,'selection retains session object');
    await tab('agent:a').click();await frames();
    const offset=await page.evaluate(id=>{const ui=sessions.get('a');return ui.entries.get(id).node.getBoundingClientRect().top-ui.trace.getBoundingClientRect().top},anchor.id);
    assert.ok(Math.abs(offset-anchor.offset)<2,'reading anchor survives hidden stream update');
    assert.deepEqual(await order('agent:a'),['agent:a','agent:b'],'selection never reorders');
    await tab('terminal:t').dragTo(group('agent:a').locator('.subpane-add'));await frames();
    assert.equal(await page.evaluate(()=>testSockets.length===1&&testSockets[0]===originalSocket&&testSockets[0].readyState===1),true,'tab move/selection retains terminal socket');
    await tab('terminal:t').dragTo(tab('agent:a'),{targetPosition:{x:2,y:8}});await frames();
    assert.deepEqual(await order('agent:a'),['terminal:t','agent:a','agent:b']);
    await tab('agent:a').click();assert.deepEqual(await order('agent:a'),['terminal:t','agent:a','agent:b']);
    assert.equal(await group('agent:a').locator('.pane-tab:not(.inactive) .pane-icon:visible').count(),1);
    assert.equal(await group('agent:a').locator('.pane-tab.inactive .pane-icon:visible').count(),0);
    // A pending submission remains the same intent across tab selection, and clears only after its real ACK.
    await input('a').fill('Pending across tabs');await input('a').press('Enter');await until(()=>posts.length===1);
    await tab('agent:b').click();await tab('agent:a').click();await input('a').press('Enter');await frames();assert.equal(posts.length,1);
    assert.equal(await input('a').inputValue(),'Pending across tabs');await posts[0].fulfill({status:202,json:{message_id:'submitted',status:'queued'}});await frames();assert.equal(await input('a').inputValue(),'');
    await input('a').fill('Refresh retained draft');
    const refreshAnchor = await page.evaluate(()=>{const ui=sessions.get('a');rememberPosition(ui);return {id:ui.anchor.node.dataset.entryId,offset:ui.anchor.offset};});
    await tab('agent:b').click();
    await page.reload();await page.waitForFunction(()=>sessions.size===2&&terminals.size===1);await frames();
    assert.deepEqual(await order('agent:a'),['terminal:t','agent:a','agent:b']);assert.equal(await tab('agent:b').getAttribute('aria-selected'),'true');
    await tab('agent:a').click();await frames();assert.equal(await input('a').inputValue(),'Refresh retained draft');
    const restored=await page.evaluate(id=>{const ui=sessions.get('a');return {follow:ui.follow,offset:ui.entries.get(id).node.getBoundingClientRect().top-ui.trace.getBoundingClientRect().top}},refreshAnchor.id);
    assert.equal(restored.follow,false);assert.ok(Math.abs(restored.offset-refreshAnchor.offset)<2,'inactive reading anchor survives refresh');
    await group('agent:a').locator('.subpane-add').click();await page.waitForFunction(()=>sessions.get('wren-1')?.group===sessions.get('b')?.group);await frames();
    assert.deepEqual(await order('agent:a'),['terminal:t','agent:a','agent:b','agent:wren-1']);assert.equal(await page.locator('.setup-form').count(),0);
    await page.locator('#add-agent').click();await page.waitForFunction(()=>sessions.has('wren-2'));await frames();assert.equal(await page.locator('.pane-group').count(),2);
    await page.locator('#add-terminal').click();await page.waitForFunction(()=>terminals.has('t2'));assert.equal(await page.locator('.pane-cwd:visible').count(),0);
    if(process.env.SCREENSHOT)await page.screenshot({path:process.env.SCREENSHOT.replace('.png','-normal.png')});
    // History remains accessible without exposing any creation form or directory chooser.
    await group('agent:a').locator('.subpane-add').click({button:'right'});await page.waitForSelector('.session-history[open] .session-card[data-session-id="old"]');
    assert.match(await page.locator('.session-history').innerText(),/Active sessions/);assert.match(await page.locator('.session-history').innerText(),/Inactive sessions/);
    if(process.env.SCREENSHOT)await page.screenshot({path:process.env.SCREENSHOT.replace('.png','-history.png')});
    assert.equal(await page.locator('.session-history input').count(),0);await page.locator('.session-history .session-card[data-session-id="old"]').click();await page.waitForFunction(()=>sessions.get('old')?.group===sessions.get('b')?.group);await frames();
    assert.ok((await order('agent:a')).includes('agent:old'));
    // Close the original group key while another tab remains: the group and other sessions survive.
    await tab('agent:a').click();await tab('agent:a').getByRole('button',{name:'Close a',exact:true}).click();await frames();
    assert.equal(await group('agent:a').count(),1);assert.ok((await order('agent:a')).includes('agent:b'));
    for(let i=0;i<7;i++){await group('agent:a').locator('.subpane-add').click();await page.waitForFunction(()=>!paneGroups.get('agent:a').plus.disabled);await frames();}
    await page.setViewportSize({width:390,height:850});await frames();
    const overflow=await group('agent:a').locator('.pane-tabs').evaluate(node=>({overflow:node.scrollWidth>node.clientWidth,bar:getComputedStyle(node).scrollbarWidth,plus:node.lastElementChild.className}));
    const visible = await group('agent:a').locator('.pane-tabs').evaluate(node=>{const active=node.querySelector('[aria-selected="true"]');return active.getBoundingClientRect().left>=node.getBoundingClientRect().left-1&&active.getBoundingClientRect().right<=node.getBoundingClientRect().right+1});
    assert.equal(visible,true,'resize keeps the active tab visible');
    assert.equal(overflow.overflow,true);assert.equal(overflow.bar,'none');assert.equal(overflow.plus,'subpane-add');
    if(process.env.SCREENSHOT){await page.screenshot({path:process.env.SCREENSHOT});}
    await group('agent:a').locator('.pane-tabs').evaluate(node=>{node.scrollLeft=0});await frames();
    assert.equal(await group('agent:a').locator('.pane-tabs').evaluate(node=>node.scrollLeft),0,'deliberate strip scrolling is not undone');
    const finalOrder=await order('agent:a');await page.reload();await page.waitForFunction(()=>sessions.size>5);await frames();assert.deepEqual(await order('agent:a'),finalOrder);
    assert.deepEqual(errors,[]);
    console.log('PASS: mixed tabs, stable selection/order, drag reorder, socket/draft/anchor continuity, real-ACK composer, direct creation, home contract, hidden cwd, history reopen, close, overflow and refresh');
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
