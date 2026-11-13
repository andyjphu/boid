// Compatibility entry points into the credential view; isolated mocked Chrome, no provider calls.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { chromium } = require(process.env.BOID_PLAYWRIGHT || 'playwright-core');
const root = path.resolve(__dirname, '../src/web');
(async () => {
  const browser = await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
  try {
    const page=await browser.newPage(), errors=[], actions=[];
    const profile={billing_route:'api_key',model:'first',reasoning_effort:'medium'};
    const groups={revision:1,groups:[{id:'agent:agent',screen:1,tabs:['agent:agent'],credential_id:'key',credential_type:'api_key'}]};
    const registry={revision:1,credentials:[{id:'key',type:'api_key',name:'openai-api-key-1',connected:true,paused:false,plan_enabled:false},{id:'sub',type:'chatgpt',name:'subscription-1',connected:true,paused:false,plan_enabled:true}]};
    page.on('pageerror',error=>errors.push(error.message));
    await page.addInitScript(()=>{window.EventSource=class{constructor(){window.testEvents=this}close(){}}});
    await page.route('**/*',async route=>{
      const url=new URL(route.request().url()), method=route.request().method();
      if(url.hostname!=='boid.test')return route.abort();
      if(method!=='GET')actions.push({path:url.pathname,body:route.request().postDataJSON()});
      if(url.pathname==='/api/config')return route.fulfill({json:{cwd:'/tmp',agent_names:[]}});
      if(url.pathname==='/api/credentials')return route.fulfill({json:registry});
      if(url.pathname==='/api/pane-groups')return route.fulfill({json:groups});
      if(url.pathname.startsWith('/api/pane-groups/')){groups.groups[0].credential_id=route.request().postDataJSON().credential_id;groups.revision++;return route.fulfill({json:groups});}
      if(url.pathname==='/api/sessions'&&method==='POST')return route.fulfill({status:400,json:{error:'Use /login to connect an API key or ChatGPT subscription.'}});
      if(url.pathname==='/api/sessions/agent/model'){profile.model=route.request().postDataJSON().model;return route.fulfill({json:{profile}});}
      if(url.pathname.endsWith('/models'))return route.fulfill({json:{models:[{slug:'second',display_name:'Second model'},{slug:'first',display_name:'First model'}]}});
      if(url.pathname.endsWith('/history'))return route.fulfill({json:{id:'agent',profile}});
      if(url.pathname.startsWith('/api/'))return route.fulfill({json:[]});
      const file=path.join(root,url.pathname==='/'?'index.html':url.pathname);
      return route.fulfill({body:fs.readFileSync(file),contentType:file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
    });
    await page.goto('http://boid.test');await page.waitForFunction(()=>window.testEvents);
    await page.evaluate(({groups,profile})=>{receivePaneGroups(groups,false);snapshot([{id:'agent',name:'canary',cwd:'/tmp',status:'idle',entries:[],pending:[],profile}],[])},{groups,profile});
    const composer=page.locator('.subpane[data-pane-key="agent:agent"] .composer textarea');
    await composer.fill('/lo');await composer.press('Tab');assert.equal(await composer.inputValue(),'/login');await composer.press('Enter');
    await page.getByRole('button',{name:'Use subscription-1',exact:true}).click();
    await page.waitForFunction(()=>serverPaneGroups.groups[0].credential_id==='sub');
    assert.deepEqual(actions[0],{path:'/api/pane-groups/agent%3Aagent',body:{credential_id:'sub',revision:1}});
    assert.equal(profile.model,'first','credential choice does not change model');
    assert.equal(await page.locator('#provider-login,.billing-route').count(),0);
    await page.keyboard.press('Escape');assert.equal(await page.locator('.credential-overlay').count(),0);
    await composer.fill('/model');await composer.press('Enter');await page.getByRole('option',{name:'Second model',exact:true}).click();
    await page.waitForFunction(()=>sessions.get('agent').view.profile.model==='second');
    assert.equal(groups.groups[0].credential_id,'sub','model choice does not change credential');
    await page.locator('#add-agent').click();await page.getByRole('button',{name:'Connect provider',exact:true}).click();
    assert.equal(await page.locator('.credential-panel').count(),1);assert.equal(await page.locator('.setup-form').count(),0);
    assert.ok(actions.every(action=>!action.path.endsWith('/messages')&&!action.path.endsWith('/interrupt')&&!action.path.startsWith('/api/accounts')));
    assert.deepEqual(errors,[]);
    console.log('PASS: /login and creation-error Connect provider open canonical credentials, group selection preserves model, /model preserves credential, Escape does not interrupt, no obsolete account selector or setup form');
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1});
