// Run against terminal_latency_fixture only. Uses an isolated real shell and PTY.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const {chromium} = require(process.env.BOID_PLAYWRIGHT || 'playwright');
const url = process.env.BOID_TEST_URL || 'http://127.0.0.1:17836';
(async () => {
  const browser = await chromium.launch({headless:true});
  let page;
  try {
    page = await browser.newPage({viewport:{width:1400,height:900}});
    const errors=[];page.on('pageerror',error=>errors.push(error.message));
    const token=fs.readFileSync(process.env.BOID_FIXTURE_TOKEN_FILE,'utf8').trim();
    await page.context().addCookies([{name:'boid_fixture_token',value:token,url}]);
    await page.goto(url);
    await page.waitForFunction(()=>typeof terminals!=='undefined' && catalogReady);
    assert.equal((await (await page.request.get(url+'/api/server')).json()).fixture,true);
    // Capture the actual xterm instance without changing its rendering path.
    await page.evaluate(()=>{
      const Original=window.Terminal;
      window.Terminal=class extends Original {
        constructor(options){super(options);window.testTerminal=this;}
      };
    });
    await page.locator('#add-terminal').click();
    await page.locator('.terminal-host').click();
    await page.waitForFunction(()=>window.testTerminal && !testTerminal.options.disableStdin);
    const textarea=page.locator('.terminal-host .xterm-helper-textarea');
    await textarea.focus();
    await page.keyboard.press('Control+c');
    await page.waitForTimeout(500);
    await page.evaluate(()=>{
      window.currentTerminalLine=()=>{const b=testTerminal.buffer.active;return b.getLine(b.baseY+b.cursorY)?.translateToString(true)||'';};
      window.lastRenderedLine='';
      testTerminal.onRender(()=>{window.lastRenderedLine=currentTerminalLine();});
      window.terminalText=()=>{
        const b=testTerminal.buffer.active;let result='';
        for(let i=Math.max(0,b.length-30);i<b.length;i++)result+=b.getLine(i)?.translateToString(true)+'\n';
        return result;
      };
    });
    // Use a plain shell so completion plugins do not contaminate PTY latency.
    await page.keyboard.type("exec env PS1='fixture> ' /bin/sh -i");
    await page.keyboard.press('Enter');
    await page.waitForFunction(()=>currentTerminalLine().trim()==='fixture>');
    const measure=async loaded=>{
      const beforeWrites=loaded?(await (await page.request.post(url+'/end-load')).json()).writes:0;
      if(loaded)await page.request.post(url+'/load');
      const samples=[];
      let typed='';
      for(let i=0;i<20;i++){
        const char=String.fromCharCode(97+i);typed+=char;
        const start=performance.now();
        await page.keyboard.type(char);
        try { await page.waitForFunction(text=>currentTerminalLine().includes(text) && lastRenderedLine.includes(text),typed,{timeout:10000,polling:'raf'}); }
        catch(error) { console.error(await page.evaluate(()=>({line:currentTerminalLine(),tail:terminalText().slice(-500),disabled:testTerminal.options.disableStdin,focused:document.activeElement?.className})));throw error; }
        samples.push(performance.now()-start);
        await page.waitForTimeout(30);
      }
      await page.keyboard.press('Control+c');
      await page.waitForTimeout(200);
      // Output must reach the actual renderer, and scrolling remains interactive.
      const prefix=loaded?'LOAD_':'IDLE_';
      const command=`python3 -u -c 'import time; [(print("${prefix}%04d"%i),time.sleep(.002)) for i in range(1000)]'`;
      await page.keyboard.type(command);
      await page.evaluate(end=>{
        window.outputParsed=[];window.outputRendered=[];window.outputEndRendered=false;
        window.outputSubscription=testTerminal.onWriteParsed(()=>outputParsed.push(performance.now()));
        window.renderSubscription=testTerminal.onRender(({start,end:rowEnd})=>{
          outputRendered.push(performance.now());
          const b=testTerminal.buffer.active;
          for(let row=start;row<=rowEnd;row++)if(b.getLine(b.viewportY+row)?.translateToString(true).includes(end))outputEndRendered=true;
        });
      },prefix+'0999');
      const outputStart=performance.now();
      await page.keyboard.press('Enter');
      await page.waitForFunction(()=>outputEndRendered,null,{timeout:20000});
      const outputMs=performance.now()-outputStart;
      const outputGaps=await page.evaluate(()=>{outputSubscription.dispose();renderSubscription.dispose();return {
        parsed:outputParsed.slice(1).map((t,i)=>t-outputParsed[i]),
        rendered:outputRendered.slice(1).map((t,i)=>t-outputRendered[i])
      };});
      await page.locator('.terminal-host').hover();
      const before=await page.evaluate(()=>testTerminal.buffer.active.viewportY);
      await page.mouse.wheel(0,-250);
      await page.waitForFunction(y=>testTerminal.buffer.active.viewportY<y,before);
      await page.evaluate(()=>testTerminal.scrollToBottom());
      const finished=loaded?await (await page.request.post(url+'/end-load')).json():{};
      const sorted=[...samples].sort((a,b)=>a-b);
      return {loaded,echo_ms:samples,median_ms:sorted[10],max_ms:sorted.at(-1),output_ms:outputMs,output_parse_max_gap_ms:Math.max(0,...outputGaps.parsed),output_render_max_gap_ms:Math.max(0,...outputGaps.rendered),output_updates:outputGaps.rendered.length+1,...finished,writes_in_run:(finished.writes||0)-beforeWrites};
    };
    const result={idle:await measure(false),loaded:await measure(true),errors};
    assert.ok(result.loaded.writes>0,'load must perform real durable writes');
    assert.deepEqual(errors,[]);
    if(process.env.BOID_EXPECT_FAST==='1'){
      assert.ok(result.loaded.max_ms<350,`echo stalled: ${result.loaded.max_ms}ms`);
      assert.ok(result.loaded.output_ms<6000,`output stalled: ${result.loaded.output_ms}ms`);
    }
    console.log(JSON.stringify(result,null,2));
    if(process.env.BOID_RESULT)fs.writeFileSync(process.env.BOID_RESULT,JSON.stringify(result,null,2));
  }finally{
    if(page){
      await page.request.post(url+'/end-load').catch(()=>{});
      const response=await page.request.get(url+'/api/terminals').catch(()=>null);
      if(response?.ok())for(const terminal of await response.json())await page.request.delete(url+'/api/terminals/'+terminal.id).catch(()=>{});
    }
    await browser.close();
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
