// Isolated history/window integration; all provider and application requests are mocked.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {chromium} = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
const entries = Array.from({length:1200},(_,i)=>({id:`e${i}`,kind:'assistant',text:`Message ${i}: `+'text '.repeat(60)}));
let seq=100;
function view(start=1120,end=1200) {return {id:'a',name:'canary',cwd:'/tmp',status:'idle',pending:[],communications:[],entries:entries.slice(start,end),history:{start,end,total:entries.length,seq}};}
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
 try {
 const page=await browser.newPage({viewport:{width:1100,height:800}});const errors=[];page.on('pageerror',e=>errors.push(e.message));
 let held=null, holdNext=false;const requests=[];
 await page.addInitScript(()=>{window.EventSource=class{constructor(url){window.testEvents=this;window.eventUrl=url;}close(){}};});
 await page.route('**/*',async route=>{
  const u=new URL(route.request().url());if(u.hostname!=='boid.test')return route.abort();
  if(u.pathname==='/api/config')return route.fulfill({json:{cwd:'/tmp',next_agent_name:'swift',agent_names:['canary','swift']}});
  if(u.pathname==='/api/sessions/a/history'){
   requests.push(u.search);let end=entries.length,start=Math.max(0,end-80);
   if(u.searchParams.has('before')){end=+u.searchParams.get('before');start=Math.max(0,end-80);}
   if(u.searchParams.has('after')){start=+u.searchParams.get('after');end=Math.min(entries.length,start+80);}
   if(u.searchParams.has('around')){const index=entries.findIndex(e=>e.id===u.searchParams.get('around'));start=Math.max(0,index-40);end=Math.min(entries.length,start+80);}
   if(u.searchParams.has('entry')){start=entries.findIndex(e=>e.id===u.searchParams.get('entry'));end=start+1;}
   const result=view(start,end);
   if(holdNext){holdNext=false;held={route,result};return;}
   return route.fulfill({json:result});
  }
  if(u.pathname.startsWith('/api/'))return route.fulfill({json:[]});
  const file=path.join(root,u.pathname==='/'?'index.html':u.pathname);
  return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
 });
 await page.goto('http://boid.test');await page.waitForFunction(()=>window.testEvents);
 const frames=()=>page.evaluate(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))));
 await page.evaluate(v=>snapshot([v],[]),view());await frames();
 assert.equal(await page.evaluate(()=>eventUrl),'/api/events?history=recent');
 assert.equal(await page.evaluate(()=>sessions.get('a').entries.size),80);
 assert.ok(await page.evaluate(()=>sessions.get('a').mounted.size<=100));
 // Load earlier history and preserve the exact visible anchor.
 const anchored=await page.evaluate(()=>{const ui=sessions.get('a');ui.follow=false;scrollTrace(ui,ui.trace.scrollTop-100);rememberPosition(ui);return {id:ui.anchor.node.dataset.entryId,offset:ui.anchor.offset};});
 await page.evaluate(()=>loadHistory(sessions.get('a'),'before'));await frames();
 const preserved=await page.evaluate(id=>{const ui=sessions.get('a');return ui.entries.get(id).node.getBoundingClientRect().top-ui.trace.getBoundingClientRect().top;},anchored.id);
 assert.ok(Math.abs(preserved-anchored.offset)<2,`prepend moved anchor by ${preserved-anchored.offset}`);
 assert.equal(await page.evaluate(()=>sessions.get('a').history.start),1040);
 // Peer jumps hydrate only the target's surrounding page and mount its destination.
 await page.evaluate(()=>locateCommunication(sessions.get('a'),{sender_id:'a',recipient_id:'b',sender_entry:'e12',id:'peer'},'a'));await frames();
 assert.equal(await page.evaluate(()=>document.activeElement.dataset.entryId),'e12');
 assert.ok(requests.some(q=>q.includes('around=e12')));
 // Live events beyond an unloaded gap must not make old history appear contiguous.
 await page.evaluate(()=>testEvents.onmessage({data:JSON.stringify({type:'entry',seq:101,session_id:'a',entry_index:1200,entry:{id:'e1200',kind:'assistant',text:'new'}})}));
 assert.equal(await page.evaluate(()=>sessions.get('a').history.total),1201);
 assert.ok(await page.evaluate(()=>sessions.get('a').history.end<500));
 entries.push({id:'e1200',kind:'assistant',text:'new'});seq=101;
 await page.evaluate(()=>loadHistory(sessions.get('a'),'latest'));await frames();
 assert.equal(await page.evaluate(()=>sessions.get('a').entries.get('e1200').text),'new');
 // Fetch captures an old tail, then a delta arrives before HTTP completion.
 holdNext=true;
 await page.evaluate(()=>{window.pendingHistory=loadHistory(sessions.get('a'),'latest');});
 for(let tries=0;!held&&tries<200;tries++)await new Promise(r=>setTimeout(r,5));assert.ok(held,'history request must start');
 await page.evaluate(()=>testEvents.onmessage({data:JSON.stringify({type:'delta',seq:102,session_id:'a',entry_id:'e1200',text:' + delta'})}));
 await held.route.fulfill({json:held.result});held=null;await page.evaluate(()=>pendingHistory);await frames();
 assert.equal(await page.evaluate(()=>sessions.get('a').entries.get('e1200').text),'new + delta');
 // Page can also include a delta before that SSE event arrives: do not apply twice.
 entries.at(-1).text='new + delta';seq=102;
 await page.evaluate(()=>loadHistory(sessions.get('a'),'latest'));await frames();
 await page.evaluate(()=>testEvents.onmessage({data:JSON.stringify({type:'delta',seq:102,session_id:'a',entry_id:'e1200',text:' + delta'})}));
 assert.equal(await page.evaluate(()=>sessions.get('a').entries.get('e1200').text),'new + delta');
 // A restarted server has a lower sequence; the snapshot resets entry watermarks.
 seq=2;entries.at(-1).text='restored';
 await page.evaluate(v=>testEvents.onmessage({data:JSON.stringify({type:'snapshot',seq:2,sessions:[v],terminals:[]})}),view(1121,1201));await frames();
 await page.evaluate(()=>testEvents.onmessage({data:JSON.stringify({type:'delta',seq:3,session_id:'a',entry_id:'e1200',text:' live'})}));await frames();
 assert.equal(await page.evaluate(()=>sessions.get('a').entries.get('e1200').text),'restored live');
 // Selected text remains attached as new entries arrive.
 await page.evaluate(()=>{const ui=sessions.get('a'),item=ui.entries.get('e1200');const range=document.createRange();range.selectNodeContents(item.body);getSelection().removeAllRanges();getSelection().addRange(range);});
 const selected=await page.evaluate(()=>getSelection().toString());
 await page.evaluate(()=>{const ui=sessions.get('a');for(let i=1201;i<1401;i++)applyTranscriptEvent(ui,{type:'entry',seq:i,entry_index:i,entry:{id:`e${i}`,kind:'assistant',text:'stream'}});});await frames();
 assert.equal(await page.evaluate(()=>getSelection().toString()),selected);
 await page.evaluate(()=>getSelection().removeAllRanges());await frames();
 // Large previews load their complete content explicitly.
 await page.evaluate(()=>{sessions.get('a').follow=true;});
 const full='large '.repeat(5000);entries[1199].text=full;seq=2000;
 await page.evaluate(v=>snapshot([v],[]),{...view(1121,1201),entries:view(1121,1201).entries.map(e=>e.id==='e1199'?{...e,text:'large preview',text_complete:false}:e)});await frames();
 await page.locator('[data-entry-id="e1199"] button').click();await frames();
 assert.equal(await page.evaluate(()=>sessions.get('a').entries.get('e1199').text),full);
 // Independent review: reading viewport must survive reconnect at the same entry/offset.
 await page.evaluate(()=>getSelection().removeAllRanges());
 await page.evaluate(()=>{const ui=sessions.get('a');ui.follow=true;});
 await page.evaluate(()=>loadHistory(sessions.get('a'),'latest'));await frames();
 const beforeReconnect=await page.evaluate(()=>{const ui=sessions.get('a');ui.follow=false;scrollTrace(ui,ui.trace.scrollTop-100);rememberPosition(ui);return {id:ui.anchor.node.dataset.entryId,offset:ui.anchor.offset,top:ui.trace.scrollTop};});
 await page.evaluate(v=>testEvents.onmessage({data:JSON.stringify({type:'snapshot',seq:v.history.seq,sessions:[v],terminals:[]})}),view(1121,1201));
 await page.waitForTimeout(150);await frames();
 const afterReconnect=await page.evaluate(id=>{const ui=sessions.get('a'),item=ui.entries.get(id);return {mounted:item?.node.isConnected,offset:item?.node.getBoundingClientRect().top-ui.trace.getBoundingClientRect().top,top:ui.trace.scrollTop,anchor:ui.anchor?.node.dataset.entryId};},beforeReconnect.id);
 assert.ok(afterReconnect.mounted); assert.ok(Math.abs(beforeReconnect.offset-afterReconnect.offset)<2, `reconnect moved ${afterReconnect.offset-beforeReconnect.offset}px`);
 // Independent review: selected old page + Latest must not hide missing middle pages.
 await page.evaluate(()=>locateCommunication(sessions.get('a'),{sender_id:'a',recipient_id:'b',sender_entry:'e12',id:'peer'},'a'));await frames();
 await page.evaluate(()=>{const item=sessions.get('a').entries.get('e12');const range=document.createRange();range.selectNodeContents(item.body);getSelection().removeAllRanges();getSelection().addRange(range);});
 await page.evaluate(()=>loadHistory(sessions.get('a'),'latest'));await frames();
 assert.equal(await page.evaluate(()=>sessions.get('a').history.start),0);
 assert.ok(await page.evaluate(()=>sessions.get('a').history.end<500));
 assert.equal(await page.evaluate(()=>sessions.get('a').historyAfter.hidden),false);
 await page.evaluate(()=>getSelection().removeAllRanges());await page.waitForTimeout(100);await frames();
 assert.equal(await page.evaluate(()=>sessions.get('a').history.end),1201);
 assert.ok(await page.evaluate(()=>sessions.get('a').history.start>1000));
 // The full-content watermark also rejects an SSE delta already included by the GET.
 await page.evaluate(()=>applyTranscriptEvent(sessions.get('a'), {type:'delta',seq:2000,entry_id:'e1199',text:'duplicate'}));
 assert.equal(await page.evaluate(()=>sessions.get('a').entries.get('e1199').text),full);
 // Scrolling gestures during streaming keep the established follow/reading boundary.
 await page.evaluate(()=>{getSelection().removeAllRanges();const ui=sessions.get('a');ui.follow=true;followLatest(ui);});await frames();
 const box=await page.locator('[data-pane-key="agent:a"] .trace').boundingBox();
 await page.mouse.move(box.x+box.width/2,box.y+box.height/2);
 await page.evaluate(()=>{window.streamTimer=setInterval(()=>putEntry(sessions.get('a'),{id:'e1200',text:'.'},true),40);});
 try {
  await page.mouse.wheel(0,1000);await page.waitForTimeout(80);
  for(const amount of [1,4,10]) {
   const before=await page.evaluate(()=>sessions.get('a').trace.scrollTop);
   await page.mouse.wheel(0,-amount);await page.waitForTimeout(80);
   const after=await page.evaluate(()=>({top:sessions.get('a').trace.scrollTop,follow:sessions.get('a').follow}));
   assert.equal(after.follow,false);assert.ok(after.top<before-.5);
  }
  await page.mouse.wheel(0,500);await page.waitForTimeout(80);
  assert.equal(await page.evaluate(()=>sessions.get('a').follow),true);
 }finally {await page.evaluate(()=>clearInterval(streamTimer));}
 // A result remains understandable when its cached call is outside the mounted window.
 entries[1125]={id:'tool-call',kind:'tool_call',text:'exec_command (pair)\n{"command":"echo hello"}'};
 entries[1195]={id:'tool-result',kind:'tool_result',text:'exec_command (pair)\n{"stdout":{"text":"hello\\nworld\\nmore\\nlast"},"exit_code":0}'};
 entries[1199].text='short again';seq=4000;
 await page.evaluate(()=>{sessions.get('a').follow=true;});
 await page.evaluate(v=>snapshot([v],[]),view(1121,1201));await frames();
 assert.equal(await page.evaluate(()=>sessions.get('a').entries.get('tool-call').node.isConnected),false);
 assert.ok(await page.locator('[data-entry-id="tool-result"]').textContent().then(t=>t.includes('hello')));
 await page.locator('[data-entry-id="tool-result"] summary').click();await frames();
 assert.equal(await page.locator('[data-entry-id="tool-result"] details').getAttribute('open'),'');
 // A delta for an unloaded entry must be replayed after its older page arrives.
 holdNext=true;
 await page.evaluate(()=>{window.pendingHistory=loadHistory(sessions.get('a'),'before');});
 for(let tries=0;!held&&tries<200;tries++)await new Promise(r=>setTimeout(r,5));assert.ok(held);
 await page.evaluate(()=>testEvents.onmessage({data:JSON.stringify({type:'delta',seq:4001,session_id:'a',entry_id:'e1100',text:' racing delta'})}));
 await held.route.fulfill({json:held.result});held=null;await page.evaluate(()=>pendingHistory);await frames();
 assert.ok(await page.evaluate(()=>sessions.get('a').entries.get('e1100').text.endsWith(' racing delta')));
 // Selection begun during an automatic reconnect request survives its response.
 const selectedAnchor=await page.evaluate(()=>{const ui=sessions.get('a');ui.follow=false;rememberPosition(ui);return ui.anchor.node.dataset.entryId;});
 holdNext=true;
 await page.evaluate(id=>{window.pendingRestore=loadHistory(sessions.get('a'),'restore',id);},selectedAnchor);
 for(let tries=0;!held&&tries<200;tries++)await new Promise(r=>setTimeout(r,5));assert.ok(held);
 const duringSelection=await page.evaluate(id=>{const item=sessions.get('a').entries.get(id);const r=document.createRange();r.selectNodeContents(item.body);getSelection().removeAllRanges();getSelection().addRange(r);return getSelection().toString();},selectedAnchor);
 await held.route.fulfill({json:held.result});held=null;await page.evaluate(()=>pendingRestore);await frames();
 assert.equal(await page.evaluate(()=>getSelection().toString()),duringSelection);
 assert.ok(await page.evaluate(()=>sessions.get('a').deferredNavigation));
 await page.evaluate(()=>getSelection().removeAllRanges());await page.waitForTimeout(100);await frames();
 // A late peer-jump response must not reopen a pane closed during the request.
 holdNext=true;
 await page.evaluate(()=>{window.pendingJump=locateCommunication(sessions.get('a'),{sender_id:'a',recipient_id:'b',sender_entry:'e20',id:'peer'},'a');});
 for(let tries=0;!held&&tries<200;tries++)await new Promise(r=>setTimeout(r,5));assert.ok(held);
 await page.evaluate(()=>removeSession(sessions.get('a')));
 await held.route.fulfill({json:held.result});held=null;await page.evaluate(()=>pendingJump);await frames();
 assert.equal(await page.evaluate(()=>sessions.has('a')),false);
 assert.deepEqual(errors,[]);
 console.log('PASS: recent snapshot, bounded DOM, prepend anchor, peer jump, unloaded gap, fetch/SSE races, restart sequence, selection, full content');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exit(1)});
