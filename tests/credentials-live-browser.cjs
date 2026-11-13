// Real isolated-server smoke. No prompts, OAuth, terminal creation, or provider requests.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium}=require(process.env.BOID_PLAYWRIGHT||'playwright-core');
assert.equal(process.env.BOID_REAL_SMOKE_ISOLATED,'1','Explicit isolated-server authorization required');
const origin=new URL(process.env.BOID_SMOKE_URL||'http://invalid');
assert.ok(['localhost','127.0.0.1','[::1]'].includes(origin.hostname),'Use isolated loopback/forwarded server only');
assert.ok(process.env.BOID_SMOKE_TOKEN&&process.env.BOID_SMOKE_HOME,'Synthetic server token and expected HOME required');
const expectedHome=process.env.BOID_SMOKE_HOME,forbidden=/\/(messages|sign-in|models|terminals)(?:[/?]|$)/;
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});let context;
 try{
  context=await browser.newContext({viewport:{width:1200,height:850},extraHTTPHeaders:{Authorization:`Bearer ${process.env.BOID_SMOKE_TOKEN}`}});
  const errors=[],blocked=[],writes=[];
  await context.route('**/*',route=>{const url=new URL(route.request().url());if(url.origin!==origin.origin||forbidden.test(url.pathname)||url.pathname==='/api/sessions/archived'||url.pathname.endsWith('/reopen')){blocked.push(url.pathname);return route.abort();}if(route.request().method()!=='GET')writes.push(url.pathname);return route.continue();});
  const raw=async(url,method='GET',data)=>{assert.ok(url.startsWith('/api/')&&!forbidden.test(url));return context.request.fetch(origin.origin+url,{method,...(data===undefined?{}:{data})});};
  const api=async(url,method='GET',data)=>{const r=await raw(url,method,data);assert.ok(r.ok(),`${method} ${url}: HTTP ${r.status()}`);return r.json();};
  const until=async(check,label)=>{for(let n=0;n<200;n++){const value=await check();if(value)return value;await new Promise(resolve=>setTimeout(resolve,100));}throw new Error(`Timeout: ${label}`);};
  const list=()=>api('/api/sessions?history=recent'),registry=()=>api('/api/credentials'),groups=()=>api('/api/pane-groups');
  assert.deepEqual(await list(),[],'Refuse populated instance');assert.deepEqual((await registry()).credentials,[],'Refuse existing credentials');assert.equal((await groups()).groups.length,0);
  const open=async()=>{const p=await context.newPage();p.on('pageerror',e=>errors.push(e.message));await p.goto(origin.origin);await p.waitForFunction(()=>catalogReady&&!document.querySelector('#open-credentials').disabled);return p;};
  const page=await open();await page.locator('#open-credentials').click();
  const empty=page.locator('.credential-workspace');await empty.getByRole('button',{name:'+ API key',exact:true}).click();await empty.getByLabel('Key',{exact:true}).fill('synthetic-key-never-valid-smoke-one');await empty.getByRole('button',{name:'Save',exact:true}).click();
  const firstRegistry=await until(async()=>{const r=await registry();return r.credentials.length===1&&r;},'automatic key creation');
  const keyOne=firstRegistry.credentials[0];assert.equal(keyOne.name,'openai-api-key-1');assert.equal(firstRegistry.default_credential_id,null);
  await page.locator('#add-agent').click();const first=await until(async()=>{const r=await list();return r.length===1&&r[0];},'first empty agent');
  assert.equal(first.cwd,expectedHome);assert.match(first.name,/^[a-z]+(?:-\d+)?$/);assert.equal(first.profile.credential_id,null);
  const groupOne=first.profile.pane_group_id;
  const pane=(p,id)=>p.locator(`.pane-group[data-pane-key="${id}"]`),tab=(p,id)=>p.locator(`.pane-tab[data-subpane-key="agent:${id}"]`),panel=(p,id)=>pane(p,id).locator('.credential-panel');
  await page.waitForFunction(id=>sessions.has(id),first.id);
  for(const count of [2,3]){await pane(page,groupOne).locator('.subpane-add').click();await until(async()=>(await groups()).groups.find(g=>g.id===groupOne)?.tabs.length===count,'background tab');}
  const initial=await list(),background=initial.find(row=>row.id!==first.id);assert.equal(initial.length,3);
  for(const row of initial){assert.equal(row.cwd,expectedHome);assert.match(row.name,/^[a-z]+(?:-\d+)?$/);assert.equal(row.profile.credential_id,null);}
  await page.locator('#add-agent').click();const fourth=await until(async()=>{const rows=await list();return rows.length===4&&rows.find(row=>!initial.some(old=>old.id===row.id));},'separate pane');const groupTwo=fourth.profile.pane_group_id;
  const added=await api('/api/credentials','POST',{type:'api_key',name:'',key:'synthetic-key-never-valid-smoke-two'}),keyTwo=added.registry.credentials.find(row=>row.id===added.id);assert.equal(keyTwo.name,'openai-api-key-2');
  await panel(page,groupTwo).getByRole('button',{name:`Edit ${keyTwo.name}`,exact:true}).click();await panel(page,groupTwo).getByLabel('Name',{exact:true}).fill('alpha-key');await panel(page,groupTwo).getByRole('button',{name:'Save',exact:true}).click();
  await until(async()=>(await registry()).credentials.some(row=>row.id===keyTwo.id&&row.name==='alpha-key'),'global key edit');
  const other=await open();await other.locator('#open-credentials').click();await page.waitForFunction(()=>sessions.size===4);await other.waitForFunction(()=>sessions.size===4);
  const staleRevision=(await groups()).revision;
  await panel(page,groupOne).locator(`tr[data-credential-id="${keyOne.id}"] .credential-type`).click();await until(async()=>(await groups()).groups.find(g=>g.id===groupOne)?.credential_id===keyOne.id,'group assignment');
  await other.waitForFunction(({g,k})=>serverPaneGroups.groups.find(row=>row.id===g)?.credential_id===k,{g:groupOne,k:keyOne.id});
  for(const row of initial)assert.equal((await api(`/api/sessions/${row.id}/history`)).profile.credential_id,keyOne.id);
  const unchanged=(await groups()).revision;await tab(page,first.id).click();await tab(other,background.id).click();assert.equal((await groups()).revision,unchanged,'selection cannot mutate auth/order');
  const stale=await raw(`/api/pane-groups/${encodeURIComponent(groupOne)}`,'PATCH',{credential_id:keyTwo.id,revision:staleRevision});assert.equal(stale.status(),409);assert.equal((await stale.json()).code,'pane_groups_changed');
  await panel(page,groupTwo).getByRole('button',{name:'Use alpha-key',exact:true}).click();await until(async()=>(await groups()).groups.find(g=>g.id===groupTwo)?.credential_id===keyTwo.id,'second group assignment');
  await tab(page,background.id).dragTo(tab(page,fourth.id),{targetPosition:{x:12,y:8}});await until(async()=>(await groups()).groups.find(g=>g.id===groupTwo)?.tabs.includes(`agent:${background.id}`),'server-owned regroup');
  await other.waitForFunction(({b,c})=>sessions.get(b)?.group===sessions.get(c)?.group,{b:background.id,c:fourth.id});assert.equal((await api(`/api/sessions/${background.id}/history`)).profile.credential_id,keyTwo.id);assert.equal((await list()).length,4);
  await tab(page,first.id).click();await tab(page,first.id).getByRole('button',{name:`Close ${first.name}`,exact:true}).click();await until(async()=>!(await list()).some(row=>row.id===first.id),'ended agent removed');await other.waitForFunction(id=>!sessions.has(id),first.id);
  assert.ok(!(await raw(`/api/sessions/${first.id}/history`)).ok(),'ended trace unavailable');assert.deepEqual(await api('/api/sessions/archived'),[],'legacy archive list exposes no ended records');
  const diagnostics=await api('/api/diagnostics');assert.ok(!diagnostics.sessions.some(row=>row.id===first.id));assert.equal(diagnostics.aggregate.recorded_requests,0,'no provider requests recorded');
  await panel(page,groupTwo).getByRole('button',{name:'Remove alpha-key',exact:true}).click();await until(async()=>(await groups()).groups.find(g=>g.id===groupTwo)?.credential_id===null,'delete unassigns group');
  for(const id of [background.id,fourth.id])assert.equal((await api(`/api/sessions/${id}/history`)).profile.credential_id,null);
  await page.reload();await page.waitForFunction(()=>sessions.size===3);await page.locator('#open-credentials').click();await page.waitForFunction(()=>registryLoaded);assert.equal(await pane(page,groupTwo).locator('.pane-tab').count(),2);
  await page.keyboard.press('Escape');await page.locator('#add-agent').click({button:'right'});await page.locator('.session-history[open] .session-card').first().waitFor();assert.equal(await page.locator(`.session-card[data-session-id="${first.id}"]`).count(),0);assert.equal(await page.locator('.session-history .session-card').count(),3);
  await page.getByRole('button',{name:'Close active agents',exact:true}).click();await page.locator('#open-credentials').click();
  if(process.env.BOID_SMOKE_SCREENSHOT){fs.mkdirSync(path.dirname(process.env.BOID_SMOKE_SCREENSHOT),{recursive:true});await page.screenshot({path:process.env.BOID_SMOKE_SCREENSHOT});}
  assert.deepEqual(blocked,[]);assert.deepEqual(errors,[]);assert.ok(writes.every(p=>!forbidden.test(p)));
  console.log(JSON.stringify({result:'PASS',scope:'Real isolated HTTP/SSE and two Chrome pages; no prompts/providers',activeAgents:3,endedAgent:first.id,automaticNames:true,home:expectedHome,canonicalGroups:true,hiddenTabCredentials:true,archiveAbsent:true,defaultFallback:false,recordedRequests:diagnostics.aggregate.recorded_requests,screenshot:process.env.BOID_SMOKE_SCREENSHOT||null},null,2));
 }finally{await context?.close();await browser.close();}
})().catch(error=>{console.error(error.message);process.exitCode=1});
