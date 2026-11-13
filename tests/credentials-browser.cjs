// Two isolated Chrome windows; registry, groups, OAuth and sockets are mocked. No real credentials/providers.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
(async () => {
  const browser = await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
  try {
    const context=await browser.newContext({viewport:{width:1200,height:800}}), pages=[], errors=[], mutations=[];
    const agents=['a','b','c'].map(id=>({id,name:id,cwd:'/home/mock',status:'working',pending:[],entries:[{id:`${id}-1`,kind:'assistant',text:'Work continues while credential settings are visible.'}]}));
    const terminals=[{id:'t',name:'#1',cwd:'/tmp',running:true}];
    const screens={revision:1,count:1,panes:{}};
    const registry={revision:1,default_credential_id:'z',credentials:[
      {id:'z',type:'api_key',name:'zebra-key',connected:true,paused:false,plan_enabled:false},
      {id:'s',type:'chatgpt',name:'alpha-subscription',connected:true,paused:false,plan_enabled:true},
      {id:'k',type:'api_key',name:'beta-key',connected:true,paused:false,plan_enabled:false},
    ]};
    const groups={revision:1,groups:[
      {id:'agent:a',screen:1,tabs:['agent:a','agent:b'],credential_id:'z',credential_type:'api_key'},
      {id:'agent:c',screen:1,tabs:['agent:c'],credential_id:'s',credential_type:'chatgpt'},
      {id:'terminal:t',screen:1,tabs:['terminal:t'],credential_id:null,credential_type:null},
    ]};
    let sequence=0, releaseSelection=null, holdSelection=false, forceConflict=false, keyNumber=0;
    const frames=p=>p.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    const emit=async message=>{const data={...message,seq:++sequence};for(const p of pages)await p.evaluate(message=>window.testEvents?.onmessage?.({data:JSON.stringify(message)}),data);};
    const snapshot=async page=>{await page.evaluate(message=>testEvents.onmessage({data:JSON.stringify(message)}),{type:'snapshot',seq:++sequence,screens,pane_groups:groups,catalog:agents.map(a=>a.id),sessions:agents,terminals});};
    await context.exposeBinding('subscribeFixture',async({page})=>snapshot(page));
    await context.addInitScript(()=>{
      window.EventSource=class{constructor(url){this.url=url;window.testEvents=this;setTimeout(()=>{this.onopen?.();window.subscribeFixture()},0)}close(){}};
      window.testSockets=[];window.WebSocket=class{static OPEN=1;constructor(){this.readyState=1;testSockets.push(this)}close(){this.readyState=3}send(){}};
      window.testPopups=[];window.open=()=>{const popup={location:{href:''},close(){this.closed=true}};testPopups.push(popup);return popup;};
    });
    await context.route('**/*',async route=>{
      const url=new URL(route.request().url()), method=route.request().method();
      if(url.hostname!=='boid.test')return route.abort();
      const body=method==='GET'?null:route.request().postDataJSON();
      if(method!=='GET')mutations.push({path:url.pathname,method,...(body?{body:{...body,key:body.key?'[synthetic secret]':undefined}}:{})});
      if(url.pathname==='/api/config')return route.fulfill({json:{cwd:'/tmp',agent_names:[]}});
      if(url.pathname==='/api/screens')return route.fulfill({json:screens});
      if(url.pathname==='/api/pane-groups'&&method==='GET')return route.fulfill({json:groups});
      if(url.pathname==='/api/credentials'&&method==='GET')return route.fulfill({json:registry});
      if(url.pathname==='/api/credentials/sign-in')return route.fulfill({json:{start_url:'https://oauth.invalid/fixture'}});
      if(url.pathname==='/api/credentials'&&method==='POST'){
        assert.equal(body.key,'fixture-secret');const id=`new-${++keyNumber}`;
        registry.credentials.push({id,type:'api_key',name:body.name||`openai-api-key-${keyNumber}`,connected:true,paused:false,plan_enabled:false});registry.revision++;
        await emit({type:'credentials',registry});return route.fulfill({json:{id,registry}});
      }
      if(url.pathname.startsWith('/api/credentials/')){
        const id=decodeURIComponent(url.pathname.split('/').at(-1));const row=registry.credentials.find(row=>row.id===id);
        if(method==='PATCH'){if(body.key)assert.equal(body.key,'fixture-secret');Object.assign(row,body.name?{name:body.name}:{});}
        if(method==='DELETE'){
          registry.credentials=registry.credentials.filter(row=>row.id!==id);
          for(const group of groups.groups)if(group.credential_id===id){group.credential_id=null;group.credential_type=null;}
          groups.revision++;await emit({type:'pane_groups',pane_groups:groups});
        }
        registry.revision++;await emit({type:'credentials',registry});return route.fulfill({json:{registry}});
      }
      if(url.pathname==='/api/pane-groups/move'){
        assert.equal(body.revision,groups.revision);const source=groups.groups.find(group=>group.tabs.includes(body.pane));
        source.tabs=source.tabs.filter(key=>key!==body.pane);const target=groups.groups.find(group=>group.id===body.target_group_id);
        const index=target.tabs.indexOf(body.before);target.tabs.splice(index<0?target.tabs.length:index,0,body.pane);
        groups.groups=groups.groups.filter(group=>group.tabs.length);groups.revision++;
        await emit({type:'pane_groups',pane_groups:groups});return route.fulfill({json:groups});
      }
      if(url.pathname.startsWith('/api/pane-groups/')&&method==='PATCH'){
        if(holdSelection)await new Promise(resolve=>{releaseSelection=resolve});
        if(forceConflict){forceConflict=false;return route.fulfill({status:409,json:{error:'Pane groups changed. Refresh and retry.'}});}
        assert.equal(body.revision,groups.revision);const group=groups.groups.find(group=>group.id===decodeURIComponent(url.pathname.split('/').at(-1)));
        group.credential_id=body.credential_id;group.credential_type=registry.credentials.find(row=>row.id===body.credential_id)?.type||null;groups.revision++;
        await emit({type:'pane_groups',pane_groups:groups});return route.fulfill({json:groups});
      }
      if(url.pathname==='/api/sessions')return route.fulfill({json:agents});
      if(url.pathname.endsWith('/history'))return route.fulfill({json:agents.find(a=>url.pathname.includes(`/${a.id}/`))});
      if(url.pathname.startsWith('/api/'))return route.fulfill({json:[]});
      const file=path.join(root,url.pathname==='/'?'index.html':url.pathname);
      return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
    });
    const makePage=async()=>{const p=await context.newPage();pages.push(p);p.on('pageerror',error=>errors.push(error.message));await p.goto('http://boid.test');await p.waitForFunction(()=>sessions.size===3&&paneGroups.size===3);await frames(p);return p;};
    const page=await makePage(), other=await makePage();
    const panel=(p,id)=>p.locator(`.pane-group[data-pane-key="${id}"] .credential-panel`);
    const tab=(p,id)=>p.locator(`.pane-tab[data-subpane-key="agent:${id}"]`);
    const selected=(p,id)=>panel(p,id).locator('tr[aria-selected=true]').getAttribute('data-credential-id');
    const until=async check=>{for(let i=0;!check();i++){assert.ok(i<200);await new Promise(resolve=>setTimeout(resolve,10));}};
    await page.evaluate(()=>{window.captureCount=0;window.originalCapture=captureAgentState;captureAgentState=(...args)=>{captureCount++;return originalCapture(...args)};window.retainedB=sessions.get('b');});
    await page.locator('#open-credentials').click();await other.locator('#open-credentials').click();await frames(page);
    assert.equal(await page.locator('.credential-panel').count(),3);assert.equal(await page.evaluate(()=>captureCount),0,'lock toggle does not capture transcript state');
    assert.deepEqual(await panel(page,'agent:a').locator('tr').evaluateAll(rows=>rows.map(row=>row.dataset.credentialId)),['s','k','z']);
    assert.equal(await selected(page,'agent:a'),'z');assert.equal(await selected(other,'agent:a'),'z');
    assert.ok(await panel(page,'agent:a').locator('tr').evaluateAll(rows=>rows.every(row=>row.cells.length===4)));
    const beforeSelection=mutations.length;await tab(page,'b').click();await tab(other,'a').click();await frames(page);
    assert.equal(mutations.length,beforeSelection,'active tab selection makes no auth mutation');assert.equal(await selected(page,'agent:a'),'z');
    assert.equal(await page.evaluate(()=>sessions.get('b')===retainedB&&sessions.get('a').view.status==='working'&&sessions.get('b').view.status==='working'),true);
    holdSelection=true;await panel(page,'agent:a').locator('tr[data-credential-id=k] .credential-type').click();await until(()=>releaseSelection);
    assert.equal(await selected(page,'agent:a'),'z','unaccepted credential selection stays unchanged');assert.equal(await selected(other,'agent:a'),'z');
    holdSelection=false;releaseSelection();await page.waitForFunction(()=>serverPaneGroups.groups.find(g=>g.id==='agent:a').credential_id==='k');await frames(other);
    assert.equal(await selected(other,'agent:a'),'k');assert.equal(await page.evaluate(()=>captureCount),0,'credential confirmation does not capture transcripts');
    if(process.env.SCREENSHOT)await page.screenshot({path:process.env.SCREENSHOT});
    // API key creation/editing propagates globally but does not silently choose it.
    await panel(page,'agent:a').getByRole('button',{name:'+ API key',exact:true}).click();
    await panel(page,'agent:a').getByLabel('Name',{exact:true}).fill('delta-key');await panel(page,'agent:a').getByLabel('Key',{exact:true}).fill('fixture-secret');
    if(process.env.SCREENSHOT)await page.screenshot({path:process.env.SCREENSHOT.replace('.png','-edit.png')});
    await panel(page,'agent:a').getByRole('button',{name:'Save',exact:true}).click();await other.waitForFunction(()=>credentialRegistry.credentials.some(row=>row.name==='delta-key'));
    assert.equal(await panel(other,'agent:c').getByRole('button',{name:'Use delta-key',exact:true}).count(),1);assert.equal(await selected(page,'agent:a'),'k');
    await panel(page,'agent:a').getByRole('button',{name:'Edit beta-key',exact:true}).click();await panel(page,'agent:a').getByLabel('Name',{exact:true}).fill('aardvark-key');
    registry.credentials.find(row=>row.id==='z').name='zebra-renamed';registry.revision++;await emit({type:'credentials',registry});await frames(page);
    assert.equal(await panel(page,'agent:a').getByLabel('Name',{exact:true}).inputValue(),'aardvark-key','remote registry update preserves open editor');
    await panel(page,'agent:a').getByRole('button',{name:'Save',exact:true}).click();await other.waitForFunction(()=>credentialRegistry.credentials.find(row=>row.id==='k').name==='aardvark-key');
    assert.equal(await panel(other,'agent:a').locator('.credential-label strong').innerText(),'aardvark-key');
    await panel(page,'agent:c').getByRole('button',{name:'Edit alpha-subscription',exact:true}).click();await until(()=>mutations.some(m=>m.path==='/api/credentials/sign-in'));
    const reauth=mutations.find(m=>m.path==='/api/credentials/sign-in');assert.equal(reauth.body.credential_id,'s');assert.equal(reauth.body.consent,false);
    await panel(page,'agent:c').getByRole('button',{name:'+ Subscription',exact:true}).click();await until(()=>mutations.filter(m=>m.path==='/api/credentials/sign-in').length===2);
    assert.equal(mutations.filter(m=>m.path==='/api/credentials/sign-in')[1].body.consent,true);
    registry.credentials.find(row=>row.id==='z').name='delta-key';registry.revision++;await emit({type:'credentials',registry});
    assert.equal(await panel(page,'agent:a').getByRole('button',{name:'Use delta-key · z',exact:true}).count(),1);
    assert.equal(await panel(page,'agent:a').getByRole('button',{name:'Use delta-key · new-1',exact:true}).count(),1);
    registry.credentials.find(row=>row.id==='z').name='zebra-key';registry.revision++;await emit({type:'credentials',registry});
    // Moving a background agent is server-confirmed and adopts destination credentials in both windows.
    await tab(other,'b').click();
    await tab(page,'b').dragTo(tab(page,'c'),{targetPosition:{x:2,y:8}});
    await other.waitForFunction(()=>sessions.get('b').group===sessions.get('c').group);await frames(page);
    assert.deepEqual(await page.evaluate(()=>sessions.get('c').group.record.tabs),['agent:b','agent:c']);
    assert.equal(await selected(page,'agent:c'),'s');assert.equal(await selected(other,'agent:c'),'s');
    assert.equal(await page.evaluate(()=>sessions.size===3&&sessions.get('b')===retainedB),true);
    assert.equal(await other.evaluate(()=>sessions.get('c').group.record.active),'agent:c','remote regroup preserves local active tab');
    assert.equal(await other.evaluate(()=>selected?.hidden||false),false,'remote regroup never leaves keyboard target in a hidden pane');
    const authMutations=mutations.filter(m=>m.method==='PATCH'&&m.path.startsWith('/api/pane-groups/')).length;
    await tab(page,'c').click();await tab(page,'b').click();await frames(page);
    assert.equal(mutations.filter(m=>m.method==='PATCH'&&m.path.startsWith('/api/pane-groups/')).length,authMutations);
    forceConflict=true;await panel(page,'agent:c').getByRole('button',{name:'Use delta-key',exact:true}).click();
    await page.waitForFunction(()=>[...credentialPanels.values()].some(view=>view.overlay.textContent.includes('Pane groups changed')));
    assert.equal(await selected(page,'agent:c'),'s','conflict cannot display rejected selection');
    await panel(page,'agent:a').getByRole('button',{name:'Remove aardvark-key',exact:true}).click();await other.waitForFunction(()=>!credentialRegistry.credentials.some(row=>row.id==='k'));await frames(other);
    assert.equal(await panel(other,'agent:a').locator('tr[aria-selected=true]').count(),0,'deleted credential has no fallback');
    assert.match(await panel(other,'agent:a').locator('.credential-label').innerText(),/No credential selected/);
    assert.equal(await page.evaluate(()=>JSON.stringify({...sessionStorage,...localStorage}).includes('fixture-secret')),false);
    assert.equal(await page.evaluate(()=>sessions.size),3);assert.equal(await other.evaluate(()=>sessions.size),3);
    assert.ok(mutations.every(m=>!m.path.endsWith('/messages')&&!m.path.endsWith('/interrupt')&&!(m.method==='DELETE'&&m.path.startsWith('/api/sessions'))),'credential controls do not prompt, interrupt or end agents');
    await page.keyboard.press('Escape');assert.equal(await page.locator('.credential-overlay').count(),0);
    await page.locator('#open-credentials').click();await frames(page);assert.equal(await selected(page,'agent:c'),'s');
    await page.reload();await page.waitForFunction(()=>sessions.size===3&&serverPaneGroups);await frames(page);
    assert.deepEqual(await page.evaluate(()=>sessions.get('c').group.record.tabs),['agent:b','agent:c'],'refresh uses canonical membership');
    await page.locator('#open-credentials').click();await page.waitForFunction(()=>registryLoaded);
    registry.credentials.find(row=>row.id==='z').name='reconnected-key';registry.revision++;await snapshot(page);
    await page.waitForFunction(()=>credentialRegistry.credentials.some(row=>row.name==='reconnected-key'));
    await page.locator('#open-credentials').click();
    // Empty workspace still offers registry creation without a failed agent-creation request.
    agents.length=0;terminals.length=0;groups.groups=[];groups.revision++;await snapshot(page);await frames(page);
    await page.locator('#open-credentials').click();await page.waitForSelector('.credential-workspace .credential-panel');
    await page.waitForFunction(()=>registryLoaded);const emptyPanel=page.locator('.credential-workspace .credential-panel');assert.equal(await emptyPanel.locator('tr').count(),3);
    await emptyPanel.getByRole('button',{name:'+ API key',exact:true}).click();await emptyPanel.getByLabel('Key',{exact:true}).fill('fixture-secret');await emptyPanel.getByRole('button',{name:'Save',exact:true}).click();
    await page.waitForFunction(()=>credentialRegistry.credentials.length===4);
    await emptyPanel.locator('.credential-editor').waitFor({state:'detached'});
    if(process.env.SCREENSHOT)await page.screenshot({path:process.env.SCREENSHOT.replace('.png','-empty.png')});
    assert.ok(!mutations.some(m=>m.path==='/api/sessions'&&m.method==='POST'));
    await page.evaluate(()=>{diagnosticsData={sessions:[{id:'idle',name:'Idle',archived:false},{id:'ended',name:'Ended',archived:true}]};renderDiagnostics();});
    assert.deepEqual(await page.locator('#diagnostics-scope option').evaluateAll(options=>options.map(option=>option.value)),['','idle'],'stale archived diagnostics rows remain excluded');
    assert.deepEqual(errors,[]);
    console.log('PASS: shared credential registry, four-column panels, server-confirmed group choice, delayed/rejected changes, global CRUD, reauth consent, background continuity, canonical two-window groups, no auth on tab selection, empty workspace, no secret storage or provider calls');
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1});
