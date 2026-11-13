// Real wheel/keyboard regression with synthetic sessions; no live requests.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium}=require(process.env.BOID_PLAYWRIGHT||'playwright');
const root=process.env.BOID_WEB_ROOT||path.resolve(__dirname,'../src/web');
const url=process.env.BOID_SERVER_URL||'http://boid.test';
const pairs=id=>Array.from({length:300},(_,i)=>({id:`${id}-${i}`,kind:i%2?'tool_result':'tool_call',text:`exec_command (pair-${Math.floor(i/2)})\n`+JSON.stringify(i%2?{stdout:{text:'done'},exit_code:0}:{command:'true'})}));
const view=(id,entries)=>({id,name:id,cwd:'/tmp',status:'working',pending:[],entries,history:{start:0,end:entries.length,total:entries.length,seq:1}});
(async()=>{
 const browser=await chromium.launch({headless:true});
 try {
  const page=await browser.newPage({viewport:{width:1400,height:900}}),errors=[];let interrupts=0;
  page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
   window.EventSource=class{constructor(){window.testEvents=this;}close(){}};
   window.frameCosts=[];const raf=window.requestAnimationFrame;
   window.requestAnimationFrame=callback=>raf.call(window,time=>{const start=performance.now();callback(time);frameCosts.push(performance.now()-start);});
  });
  await page.route('**/*',route=>{
   const u=new URL(route.request().url());if(u.origin!==new URL(url).origin)return route.abort();
   if(u.pathname==='/api/config')return route.fulfill({json:{cwd:'/tmp',agent_names:[]}});
   if(u.pathname.endsWith('/interrupt')){interrupts++;return route.fulfill({status:202,json:{status:'interrupt_requested'}});}
   if(u.pathname.startsWith('/api/'))return route.fulfill({json:[]});
   if(process.env.BOID_SERVER_URL)return route.continue();
   const file=path.join(root,u.pathname==='/'?'index.html':u.pathname);
   return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
  });
  await page.goto(url);await page.waitForFunction(()=>window.testEvents);
  await page.evaluate(()=>{
   window.inspectPane=id=>{
    const ui=sessions.get(id),b=ui.trace.getBoundingClientRect(),before=ui.spacerBefore.getBoundingClientRect(),after=ui.spacerAfter.getBoundingClientRect();
    const visible=[...ui.mounted].filter(i=>{const r=i.node.getBoundingClientRect();return !i.node.hidden&&r.height>0&&r.bottom>b.top+10&&r.top<b.bottom-10;});
    return {top:ui.trace.scrollTop,follow:ui.follow,first:visible[0]?.id,last:visible.at(-1)?.id,mounted:ui.mounted.size,
     beforeGap:before.height?Math.max(0,before.bottom-b.top-10):0,afterGap:after.height?Math.max(0,b.bottom-10-after.top):0};
   };
  });
  await page.evaluate(v=>snapshot(v,[]),['a','b'].map(id=>view(id,pairs(id))));await page.waitForTimeout(350);
  const cold=await page.evaluate(()=>inspectPane('b'));
  if(process.env.BOID_COLD_SCREENSHOT)await page.screenshot({path:process.env.BOID_COLD_SCREENSHOT});
  const box=await page.locator('[data-pane-key="agent:b"] .trace').boundingBox();
  await page.mouse.move(box.x+box.width/2,box.y+box.height/2);
  for(const delta of [-500,-1000,-5000,-5000,500,5000,5000,10000,10000]){await page.mouse.wheel(0,delta);await page.waitForTimeout(150);}
  const steps=[];
  for(const delta of [-4,-4,-10]){
   const before=await page.evaluate(()=>inspectPane('b'));
   await page.mouse.wheel(0,delta);await page.waitForTimeout(150);
   const after=await page.evaluate(()=>inspectPane('b'));steps.push({delta,before,after});
  }
  const beforeEscape=await page.evaluate(()=>inspectPane('b'));
  await page.locator('[data-pane-key="agent:a"] .composer textarea').fill('draft');await page.keyboard.press('Escape');
  await page.waitForFunction(()=>sessions.get('a').interruptRequested&&!sessions.get('a').interrupting);
  await page.evaluate(()=>setStatus(sessions.get('a'),'stopped'));await page.waitForTimeout(150);
  const afterEscape=await page.evaluate(()=>inspectPane('b'));
  assert.equal(interrupts,1);
  // Focus remains on an expanded diff while wheel gestures cross its bottom.
  const diff=[...Array.from({length:120},(_,i)=>({id:`pre-${i}`,kind:'assistant',text:`Before ${i}`})),
   {id:'diff-call',kind:'tool_call',text:'exec_command (diff)\n{"command":"git diff"}'},
   {id:'diff-result',kind:'tool_result',text:'exec_command (diff)\n'+JSON.stringify({stdout:{text:Array.from({length:600},(_,i)=>'+ diff line '+i).join('\n')},exit_code:0})},
   ...Array.from({length:25},(_,i)=>({id:`post-${i}`,kind:'assistant',text:`After ${i}`}))];
  await page.evaluate(v=>{removeSession(sessions.get('b'));addSession(v);},view('b',diff));await page.waitForTimeout(150);
  await page.locator('[data-entry-id="diff-call"] summary').click();await page.waitForTimeout(150);
  const diffBox=await page.locator('[data-pane-key="agent:b"] .trace').boundingBox();
  await page.mouse.move(diffBox.x+diffBox.width/2,diffBox.y+diffBox.height/2);
  await page.mouse.wheel(0,-8000);await page.waitForTimeout(150);
  assert.equal(await page.locator('[data-entry-id="diff-result"]').isVisible(),true);
  for(let i=0;i<12;i++){await page.mouse.wheel(0,1000);await page.waitForTimeout(30);}
  await page.waitForTimeout(250);
  const diffBefore=await page.evaluate(()=>inspectPane('b'));
  await page.mouse.wheel(0,-4);await page.waitForTimeout(150);
  const diffAfter=await page.evaluate(()=>inspectPane('b'));
  // Collapse a tall result without moving the anchor or clamping scrollTop.
  const collapseEntries=[diff[120],diff[121],...Array.from({length:200},(_,i)=>({id:`tail-${i}`,kind:'assistant',text:`Tail ${i}`}))];
  await page.evaluate(v=>{removeSession(sessions.get('b'));addSession(v);},view('b',collapseEntries));
  await page.waitForTimeout(150);
  await page.evaluate(()=>{const ui=sessions.get('b');ui.follow=false;ui.trace.scrollTop=0;scheduleViewport(ui);});
  await page.waitForTimeout(150);
  await page.locator('[data-entry-id="diff-call"] summary').click();
  await page.evaluate(()=>scheduleViewport(sessions.get('b')));await page.waitForTimeout(200);
  const collapseBefore=await page.evaluate(()=>inspectPane('b'));
  await page.locator('[data-entry-id="diff-call"] summary').click();await page.waitForTimeout(200);
  const collapseAfter=await page.evaluate(()=>inspectPane('b'));
  // Refill must settle. No polling loop is added to an idle application.
  await page.waitForTimeout(350);const costs=await page.evaluate(()=>frameCosts.splice(0));
  await page.waitForTimeout(250);const idleFrames=await page.evaluate(()=>frameCosts.length);
  const sorted=costs.sort((a,b)=>a-b);
  const result={cold,steps,beforeEscape,afterEscape,diffBefore,diffAfter,collapseBefore,collapseAfter,idleFrames,frameCallbacks:costs.length,frameP95ms:sorted[Math.floor(sorted.length*.95)],frameMaxMs:sorted.at(-1),errors};
  console.log(JSON.stringify(result,null,2));
  if(process.env.BOID_SCREENSHOT)await page.screenshot({path:process.env.BOID_SCREENSHOT});
  if(process.env.BOID_EXPECT_FIXED!=='0'){
   for(const state of [cold,...steps.map(s=>s.after),afterEscape,diffAfter,collapseAfter]){
    assert.ok(state.first,'viewport must contain real entries');assert.ok(state.beforeGap<2&&state.afterGap<2,`spacer inside viewport: ${JSON.stringify(state)}`);
   }
   for(const {delta,before,after} of steps)assert.ok(Math.abs(after.top-before.top-delta)<2,`wheel ${delta} moved ${after.top-before.top}`);
   assert.ok(Math.abs(afterEscape.top-beforeEscape.top)<2,'other pane moved on Escape');
   assert.equal(afterEscape.first,beforeEscape.first);
   assert.equal(collapseAfter.top,collapseBefore.top,'collapse moved stable anchor');
   assert.ok(Math.abs(diffAfter.top-diffBefore.top+4)<2,'focused diff jumped on small upward wheel');
  }
  assert.ok(idleFrames<3,`viewport keeps scheduling while idle: ${idleFrames}`);
  assert.deepEqual(errors,[]);
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
