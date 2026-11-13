// Chrome with synthetic ClipboardEvent/File input and mocked HTTP/SSE. Does not alter OS clipboard.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const {chromium}=require(process.env.BOID_PLAYWRIGHT||'playwright-core');
const root=path.resolve(__dirname,'../src/web');
const png=Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==','base64');
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
 try{
  const page=await browser.newPage({viewport:{width:1100,height:800}}),errors=[],uploads=[],posts=[];let seq=0;
  const views=['a','b'].map(id=>({id,name:id,cwd:'/mock',status:'idle',pending:[],entries:[]}));
  const screens={revision:1,count:2,panes:{}};
  page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{window.EventSource=class{constructor(url){this.url=url;window.testEvents=this;setTimeout(()=>{this.onopen?.();window.fixtureSnapshot()},0)}close(){}}});
  const emit=async message=>page.evaluate(message=>testEvents.onmessage({data:JSON.stringify(message)}),{...message,seq:++seq});
  await page.exposeFunction('fixtureSnapshot',async()=>{const screen=await page.evaluate(()=>Number(new URL(testEvents.url,location.origin).searchParams.get('screen')));await emit({type:'snapshot',screens,catalog:['a','b'],sessions:screen===1?views:[],terminals:[]});});
  await page.route('**/*',route=>{
   const url=new URL(route.request().url());if(url.hostname!=='boid.test')return route.abort();
   if(url.pathname==='/api/config')return route.fulfill({json:{cwd:'/mock',agent_names:[]}});
   if(url.pathname==='/api/screens')return route.fulfill({json:screens});
   if(url.pathname.endsWith('/images')){assert.equal(route.request().headers()['content-type'],'image/png');assert.deepEqual(route.request().postDataBuffer(),png);uploads.push({route,path:url.pathname});return;}
   if(url.pathname.includes('/images/'))return route.fulfill({body:png,contentType:'image/png'});
   if(url.pathname.endsWith('/messages')){posts.push({route,path:url.pathname,body:route.request().postDataJSON()});return;}
   if(url.pathname.startsWith('/api/'))return route.fulfill({json:[]});
   const file=path.join(root,url.pathname==='/'?'index.html':url.pathname);return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
  });
  const frames=()=>page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
  const until=async check=>{for(let i=0;!check();i++){assert.ok(i<200,'request arrived');await new Promise(resolve=>setTimeout(resolve,10));}};
  const input=id=>page.locator(`.subpane[data-pane-key="agent:${id}"] .composer textarea`),chips=id=>page.locator(`.subpane[data-pane-key="agent:${id}"] .image-chip`);
  const paneError=()=>page.locator('.subpane[data-pane-key="agent:a"] > .error-banner');
  const paste=async(id='a',count=1,type='image/png',text='')=>input(id).evaluate((node,{bytes,count,type,text})=>{
   const data=new DataTransfer();for(let i=0;i<count;i++)data.items.add(new File([Uint8Array.from(bytes)],`clip${i}.png`,{type}));if(text)data.setData('text/plain',text);
   const event=new ClipboardEvent('paste',{bubbles:true,cancelable:true,clipboardData:data});node.dispatchEvent(event);return event.defaultPrevented;
  },{bytes:[...png],count,type,text});
  const finishUpload=async index=>{await uploads[index].route.fulfill({status:201,json:{id:`image-${index}`,mime_type:'image/png',size_bytes:png.length,width:1,height:1}});await frames();};
  const ack=async index=>{await posts[index].route.fulfill({status:202,json:{message_id:`m${index}`,status:'queued'}});await frames();};
  const remount=async()=>{await page.locator('#screens [data-screen="2"]').click();await page.waitForFunction(()=>activeScreen===2&&!sessions.has('a'));await page.locator('#screens [data-screen="1"]').click();await page.waitForFunction(()=>sessions.has('a')&&!sessions.get('a').restoringScreen);await frames();};
  await page.goto('http://boid.test');await page.waitForFunction(()=>sessions.size===2);
  await page.locator('.pane-tab[data-subpane-key="agent:b"]').dragTo(page.locator('.pane-tab[data-subpane-key="agent:a"]'),{targetPosition:{x:12,y:8}});
  await page.locator('.pane-tab[data-subpane-key="agent:a"]').click();
  // Ordinary paste remains native; synthetic events do not simulate the browser's default text insertion.
  const plain=await input('a').evaluate(node=>{const data=new DataTransfer();data.setData('text/plain','ordinary\ntext');const event=new ClipboardEvent('paste',{bubbles:true,cancelable:true,clipboardData:data});node.dispatchEvent(event);return event.defaultPrevented;});assert.equal(plain,false);
  await input('a').pressSequentially('native text');await input('a').press('ControlOrMeta+Z');assert.notEqual(await input('a').inputValue(),'native text','native text undo retained');await input('a').press('ControlOrMeta+Shift+Z');assert.equal(await input('a').inputValue(),'native text','native redo retained');
  await input('a').fill('Describe this\nwith detail');assert.equal(await paste(),true);await until(()=>uploads.length===1);
  assert.equal(await chips('a').count(),1);assert.equal(await chips('a').locator('span').innerText(),'[Image #1]');assert.equal(await chips('a').evaluate(el=>getComputedStyle(el).color),'rgb(120, 183, 255)');
  assert.equal(await input('a').inputValue(),'Describe this\nwith detail');assert.equal(await page.locator('.entry[data-kind=user]').count(),0);
  assert.notEqual(await input('a').evaluate(el=>getComputedStyle(el).color),'rgb(120, 183, 255)','only image markers are blue');
  if(process.env.SCREENSHOT)await page.screenshot({path:process.env.SCREENSHOT.replace('.png','-mixed.png')});
  await input('a').press('Enter');await frames();assert.equal(posts.length,0,'message waits for real upload');
  await input('a').fill('Newer draft while uploading');await page.locator('.pane-tab[data-subpane-key="agent:b"]').click();await finishUpload(0);await until(()=>posts.length===1);
  assert.deepEqual(posts[0].body,{text:'Describe this\nwith detail',images:['image-0']});assert.equal(posts[0].path,'/api/sessions/a/messages');
  await remount();await page.locator('.pane-tab[data-subpane-key="agent:a"]').click();assert.equal(await input('a').inputValue(),'Newer draft while uploading');assert.equal(await chips('a').count(),1);
  await ack(0);assert.equal(await input('a').inputValue(),'Newer draft while uploading');assert.equal(await chips('a').count(),1,'newer revision retains attached image');
  // Remove before next Enter changes payload; no local accepted bubble is ever added.
  await chips('a').getByRole('button',{name:'Remove image #1',exact:true}).click();await input('a').press('Enter');await until(()=>posts.length===2);assert.deepEqual(posts[1].body,{text:'Newer draft while uploading'});await ack(1);
  await paste('a',2);await until(()=>uploads.length===3);await input('a').press('Enter');await input('a').press('Enter');await finishUpload(2);await frames();assert.equal(posts.length,2);await finishUpload(1);await until(()=>posts.length===3);
  assert.deepEqual(posts[2].body,{text:'',images:['image-1','image-2']},'image-only ordered despite reversed upload completion');
  if(process.env.SCREENSHOT)await page.screenshot({path:process.env.SCREENSHOT});
  assert.equal(await chips('a').count(),2);await ack(2);assert.equal(await chips('a').count(),0);assert.equal(await input('a').inputValue(),'');
  const imageEntry={id:'server-image',kind:'user',text:'',images:[{id:'image-1',label:'Image #1'},{id:'image-2',label:'Image #2'}]};views[0].entries.push(imageEntry);await emit({type:'entry',session_id:'a',entry:imageEntry});await frames();
  assert.equal(await page.locator('.entry-images .image-marker').count(),2);await page.locator('.entry-images a').first().click();await page.waitForFunction(()=>document.querySelector('.image-preview img')?.complete);assert.match(await page.locator('.image-preview img').getAttribute('src'),/^blob:/);await page.getByRole('button',{name:'Close image preview',exact:true}).click();
  // Failed upload never becomes a text-only request. Explicit Retry resends the same captured image intent.
  await input('a').fill('Retain on upload failure');await paste();await until(()=>uploads.length===4);await input('a').press('Enter');await uploads[3].route.fulfill({status:400,json:{error:'Fixture invalid image'}});await frames();assert.equal(posts.length,3);assert.match(await paneError().innerText(),/Fixture invalid image/);assert.equal(await chips('a').count(),1);
  await paneError().getByRole('button',{name:'Retry',exact:true}).click();await until(()=>uploads.length===5);await finishUpload(4);await until(()=>posts.length===4);assert.deepEqual(posts[3].body,{text:'Retain on upload failure',images:['image-4']});await ack(3);assert.equal(await chips('a').count(),0);
  // Completed upload metadata survives refresh, no bytes/base64 stored; reload never sends automatically.
  await paste();await until(()=>uploads.length===6);await finishUpload(5);await input('a').fill('Refresh draft');await page.waitForTimeout(20);await page.reload();await page.waitForFunction(()=>sessions.size===2);await frames();assert.equal(await chips('a').count(),1);assert.equal(await input('a').inputValue(),'Refresh draft');assert.equal(posts.length,4);
  const storage=await page.evaluate(()=>sessionStorage.getItem(submissionStorageKey));assert.ok(!storage.includes('data:')&&!storage.includes('iVBOR'));await input('a').press('Enter');await until(()=>posts.length===5);assert.deepEqual(posts[4].body,{text:'Refresh draft',images:['image-5']});await ack(4);
  // Reload during upload keeps an explicit missing attachment, never silently drops it.
  await paste();await until(()=>uploads.length===7);await input('a').fill('Unfinished image');await page.waitForTimeout(20);await page.reload();await page.waitForFunction(()=>sessions.size===2);await frames();assert.equal(await chips('a').count(),1);assert.match(await paneError().innerText(),/interrupted by reload/);await input('a').press('Enter');await frames();assert.equal(posts.length,5);assert.match(await paneError().innerText(),/Paste the image again or remove it/);
  // Remove/drop retained submission explicitly; unsupported format adds no request or chip.
  await paneError().getByRole('button',{name:'Discard this submission',exact:true}).click();await chips('a').getByRole('button',{name:'Remove image #1',exact:true}).click();await paste('a',1,'image/gif');assert.equal(await chips('a').count(),0);assert.equal(uploads.length,7);
  assert.deepEqual(errors,[]);console.log('PASS: synthetic ClipboardEvent/File through real handler; blue chips, native text/undo, removal, ordered image-only and mixed send, delayed upload/ACK, FIFO dedup, pane/screen continuity, failure retry, metadata-only refresh and explicit missing-upload recovery; no OS clipboard or provider activity');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1});
