// Real isolated server + Chromium virtual authenticator. No provider calls or real credentials.
const assert = require('node:assert/strict');
const {execFileSync} = require('node:child_process');
const {chromium} = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const url = process.env.BOID_TEST_URL || 'http://localhost:17736';
const token = process.env.BOID_TEST_TOKEN || 'isolated-passkey-test-token-only';
assert.equal(new URL(url).hostname, 'localhost');
assert.notEqual(new URL(url).port, '7734', 'Never run this against production');
async function admin(path, method='GET') {
  const r = await fetch(url+path, {method, headers:{Authorization:`Bearer ${token}`}});
  assert.ok(r.ok, `Admin ${path}: ${r.status}`); return r.json();
}
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
 try {
 const context=await browser.newContext();const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const cdp=await context.newCDPSession(page);await cdp.send('WebAuthn.enable');
 const {authenticatorId}=await cdp.send('WebAuthn.addVirtualAuthenticator',{options:{protocol:'ctap2',transport:'internal',hasResidentKey:true,hasUserVerification:true,isUserVerified:true,automaticPresenceSimulation:true}});
 const call=(path,method='POST',body)=>page.evaluate(async({path,method,body})=>{
  const r=await fetch(path,{method,headers:body?{'Content-Type':'application/json'}:{},body:body?JSON.stringify(body):undefined});return {status:r.status,body:await r.json().catch(()=>null)};
 },{path,method,body});
 const finishRequests=[];page.on('request',r=>{if(r.url().endsWith('/finish'))finishRequests.push({path:new URL(r.url()).pathname,body:r.postDataJSON()});});
 // No public "claim first owner" path, and no browser-authorized enrollment minting.
 await page.goto(url,{waitUntil:'domcontentloaded'});await page.locator('#login').waitFor({state:'visible'});
 assert.equal((await call('/auth/passkey/enroll')).status,401);
 assert.equal((await call('/auth/passkey/register/start','POST',{capability:'invalid'})).status,401);
 const wrong=await fetch(url+'/auth/passkey/sign-in/start',{method:'POST',headers:{Origin:'http://localhost:9999'}});assert.equal(wrong.status,403);
 // Enroll using only a capability minted over the administrator channel.
 const enrollment=await admin('/auth/passkey/enroll','POST');
 await page.goto(enrollment.url,{waitUntil:'domcontentloaded'});await page.locator('#passkey-enroll').waitFor({state:'visible'});
 assert.equal(await page.evaluate(()=>location.hash),'');
 assert.equal(await page.locator('#token').isVisible(),false);
 await page.screenshot({path:process.env.BOID_PASSKEY_SCREENSHOT || '/tmp/boid-passkey-enrollment.png'});
 await page.locator('#passkey-enroll').click();await page.locator('#login').waitFor({state:'hidden'});
 assert.equal((await call('/api/config','GET')).status,200);
 let cookies=await context.cookies();const session=cookies.find(c=>c.name==='boid_browser');assert.ok(session?.httpOnly);assert.equal(session.sameSite,'Strict');assert.ok(!cookies.some(c=>c.name==='boid_token'));
 assert.equal((await call('/api/login')).status,403);
 const registration=finishRequests.at(-1);assert.equal((await call(registration.path,'POST',registration.body)).status,401);
 assert.equal((await call('/auth/passkey/register/start','POST',{capability:new URL(enrollment.url).hash.slice('#enroll='.length)})).status,401);
 await page.reload({waitUntil:'domcontentloaded'});await page.locator('#login').waitFor({state:'hidden'});
 // A fresh session is issued after a real signed assertion; no token cookie is introduced.
 await call('/auth/passkey/logout');await page.reload({waitUntil:'domcontentloaded'});await page.locator('#passkey-sign-in').waitFor({state:'visible'});
 await page.locator('#passkey-sign-in').click();await page.locator('#login').waitFor({state:'hidden'});
 if(process.env.BOID_TEST_RESTART) {
   execFileSync(process.env.BOID_TEST_RESTART,[],{stdio:'pipe'});
   await page.reload({waitUntil:'domcontentloaded'});await page.locator('#login').waitFor({state:'hidden'});
   assert.equal((await call('/api/config','GET')).status,200);
 }
 const assertion=finishRequests.at(-1);assert.equal((await call(assertion.path,'POST',assertion.body)).status,401);
 // Correctly signed credentials from an old challenge cannot satisfy a new one.
 await call('/auth/passkey/sign-in/start');assert.equal((await call(assertion.path,'POST',assertion.body)).status,401);
 // Collect a new real assertion, then alter each security-relevant input.
 async function assertionBody() {
   const started=await call('/auth/passkey/sign-in/start');assert.equal(started.status,200);
   return page.evaluate(async options=>passkeyResponse(await navigator.credentials.get(passkeyOptions(options,false))),started.body);
 }
 for(const alteration of ['origin','challenge','signature']) {
   const body=await assertionBody();
   if(alteration==='signature') body.response.signature=Buffer.from('invalid signature').toString('base64url');
   else {const data=JSON.parse(Buffer.from(body.response.clientDataJSON,'base64url'));data[alteration]=alteration==='origin'?'http://evil.example':'wrong-challenge';body.response.clientDataJSON=Buffer.from(JSON.stringify(data)).toString('base64url');}
   assert.equal((await call('/auth/passkey/sign-in/finish','POST',body)).status,401,alteration);
 }
 // Authenticator response without user verification must be rejected.
 await cdp.send('WebAuthn.setResponseOverrideBits',{authenticatorId,isBadUV:true});
 const unverified=await assertionBody();assert.equal((await call('/auth/passkey/sign-in/finish','POST',unverified)).status,401);
 await cdp.send('WebAuthn.setResponseOverrideBits',{authenticatorId});
 // Keep an SSE and terminal WebSocket open, then revoke this browser session.
 const terminal=await call('/api/terminals','POST',{cwd:'/tmp',cols:80,rows:24});assert.equal(terminal.status,200);
 await page.evaluate(async id=>{
  window.transportEvents=[];
  await new Promise((resolve,reject)=>{window.revocationSocket=new WebSocket(`${location.protocol==='https:'?'wss':'ws'}://${location.host}/api/terminals/${id}/ws`);revocationSocket.onopen=resolve;revocationSocket.onerror=reject;revocationSocket.onclose=()=>transportEvents.push('ws-close');});
  window.revocationEvents=new EventSource('/api/events?history=recent');revocationEvents.onerror=()=>{transportEvents.push('sse-close');revocationEvents.close();};
 },terminal.body.id);
 const access=await admin('/auth/passkey/access');assert.equal(access.credentials.length,1);assert.equal(access.sessions.length,1);
 await admin('/auth/passkey/access/'+access.sessions[0].id,'DELETE');
 await page.waitForFunction(()=>transportEvents.includes('ws-close')&&transportEvents.includes('sse-close'));
 assert.equal((await call('/api/config','GET')).status,401);
 // Credential revocation also invalidates already-issued challenges.
 await page.reload({waitUntil:'domcontentloaded'});await page.locator('#passkey-sign-in').waitFor({state:'visible'});
 const pending=await assertionBody();
 await admin('/auth/passkey/access/'+access.credentials[0].id,'DELETE');
 assert.equal((await call('/auth/passkey/sign-in/finish','POST',pending)).status,401);
 const after=await admin('/auth/passkey/access');assert.equal(after.credentials.length,0);assert.equal(after.sessions.length,0);
 // Shared token fallback still works; it cannot be used by a cross-origin browser.
 await page.goto(url+'/#token='+token,{waitUntil:'domcontentloaded'});await page.locator('#login').waitFor({state:'hidden'});
 assert.equal((await call('/api/config','GET')).status,200);
 assert.deepEqual(errors,[]);
 console.log('PASS: enrollment, sign-in, replay/origin/challenge/signature/UV rejection, cookies, live transport revocation, in-flight credential revocation, token fallback');
 }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
