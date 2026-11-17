const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium}=require(process.env.BOID_PLAYWRIGHT||'playwright-core');
const root=path.resolve(__dirname,'../src/web');
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
 try {
  const page=await browser.newPage({viewport:{width:1200,height:850}}),errors=[];let seq=0;
  page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{window.EventSource=class{constructor(){window.eventsFixture=this;}close(){}};});
  await page.route('**/*',route=>{
   const u=new URL(route.request().url());if(u.hostname!=='boid.test')return route.abort();
   if(u.pathname==='/api/config')return route.fulfill({json:{cwd:'/tmp',agent_names:[]}});
   if(u.pathname.startsWith('/api/'))return route.fulfill({json:[]});
   const f=path.join(root,u.pathname==='/'?'index.html':u.pathname);return route.fulfill({body:fs.readFileSync(f),contentType:f.endsWith('.js')?'text/javascript':f.endsWith('.css')?'text/css':'text/html'});
  });
  await page.goto('http://boid.test');await page.waitForFunction(()=>window.eventsFixture?.onmessage);
  const emit=message=>page.evaluate(message=>eventsFixture.onmessage({data:JSON.stringify(message)}),{...message,seq:++seq});
  const views=['captain','peer'].map(id=>({id,name:id,cwd:'/tmp',status:'idle',pending:[],entries:[],tab_color:null}));
  await emit({type:'snapshot',sessions:views,terminals:[]});
  const color=id=>page.evaluate(id=>getComputedStyle(sessions.get(id).tab).backgroundColor,id);
  const shimmer=id=>page.evaluate(id=>{const s=getComputedStyle(sessions.get(id).title);return{animation:s.animationName,color:s.color,background:s.backgroundImage};},id);
  await page.evaluate(()=>{moveLocalSubpane(sessions.get('peer'),sessions.get('captain').group);selectSubpane(sessions.get('captain'));});
  await emit({type:'status',session_id:'peer',status:'working',working_since_ms:Date.now()});
  assert.equal(await page.evaluate(()=>sessions.get('peer').inactive),true);
  assert.equal((await shimmer('peer')).animation,'working-shimmer');
  assert.equal((await shimmer('peer')).color,'rgba(0, 0, 0, 0)','inactive grey label must expose the shimmer gradient');
  await emit({type:'tab_color',session_id:'peer',tab_color:'cyan'});assert.equal(await color('peer'),'rgb(113, 242, 242)');
  assert.equal((await shimmer('peer')).color,'rgba(0, 0, 0, 0)');
  await emit({type:'profile',session_id:'peer',profile:{service_tier:'priority'}});assert.equal(await color('peer'),'rgb(255, 69, 58)');
  await emit({type:'tab_color',session_id:'peer',tab_color:'violet'});assert.equal(await color('peer'),'rgb(255, 69, 58)');
  assert.equal((await shimmer('peer')).animation,'working-shimmer');
  await emit({type:'profile',session_id:'peer',profile:{service_tier:'default'}});assert.equal(await color('peer'),'rgb(177, 113, 242)');
  await emit({type:'status',session_id:'peer',status:'idle'});assert.equal((await shimmer('peer')).animation,'none');
  await emit({type:'tab_color',session_id:'peer',tab_color:null});assert.equal(await color('peer'),'rgb(24, 24, 24)');
  // A reconnect snapshot must restore the server's color even if an SSE update was missed.
  views[1].tab_color='green';await emit({type:'snapshot',sessions:views,terminals:[]});assert.equal(await color('peer'),'rgb(113, 242, 113)');
  assert.equal(await page.evaluate(()=>Object.keys(agentTabColors).length),12);
  await page.emulateMedia({reducedMotion:'reduce'});
  await emit({type:'status',session_id:'peer',status:'working',working_since_ms:Date.now()});
  assert.equal((await shimmer('peer')).animation,'none');assert.notEqual((await shimmer('peer')).color,'rgba(0, 0, 0, 0)');
  assert.deepEqual(errors,[]);
  console.log('PASS: 12-color palette, live/reconnect updates, hidden working shimmer, team colors, fast precedence/restoration, grey reset, reduced-motion readability.');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
