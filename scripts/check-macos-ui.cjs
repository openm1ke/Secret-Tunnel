// Routing flow check with mocked Tauri IPC; no system extension is activated.
const {chromium} = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
let browser;
(async () => {
  const apps = [{id:'chat',name:'Example Chat',executable:'/Applications/Example Chat.app',
    patterns:['org.example.Chat','org.example.Chat.helper'],source:'macos'}];
  const imported = {id:'custom',name:'Custom Browser',executable:'/Applications/Custom Browser.app',
    patterns:['org.example.Browser'],source:'macos'};
  const cfg = {selected_server_id:'test',selected_proxy_id:'local',
    servers:[{id:'test',name:'Test SSH',host:'server.test',port:22,user:'user',key_path:'~/.ssh/test',compression:false,strict_host_key_checking:'yes',connect_timeout:8,server_alive_interval:15,server_alive_count_max:3}],
    proxies:[{id:'local',name:'Local',host:'127.0.0.1',port:1080}],
    settings:{redirect_cli_path:'/Applications/ProxyBridge.app/Contents/MacOS/ProxyBridge',redirect_processes:[],domain_rules:[],
      ipinfo_token:'',ssh_engine:'system',hide_sensitive:true,verify_proxy_on_start:true,auto_reconnect:false,reconnect_delay_seconds:3,start_tunnel_on_launch:false,clear_logs_on_start:false}};
  const routing = {running:false,starting:false,platform:'macos',requires_administrator:false,
    supports_system_ssh:true,supports_domains:false,setup_message:'',pid:null,administrator:false,component_available:true,last_error:''};
  let starts = 0, stops = 0;
  let tunnelPhase = 'running', tunnelStops = 0;
  const errors = [];
  browser = await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_EXECUTABLE ? {executablePath:process.env.PLAYWRIGHT_EXECUTABLE} : {})});
  const page = await browser.newPage({viewport:{width:1120,height:780}});
  page.on('pageerror',error => errors.push(error.message));
  await page.exposeFunction('mockInvoke',(command,args) => {
    switch(command) {
      case 'get_config': return cfg;
      case 'tunnel_status': return {state:tunnelPhase,state_emoji:'🟢',app_version:'0.2.3',ssh_engine:'system',
        server_name:'Test SSH',proxy_name:'Local',proxy_host:'127.0.0.1',proxy_port:1080,rx_total:0,tx_total:0,
        ip:'',country_code:'',country_name:'',country_flag:'',last_error:''};
      case 'traffic_history': case 'get_logs': return [];
      case 'redirect_status': return {...routing};
      case 'scan_installed_applications': return apps;
      case 'plugin:dialog|open': return [imported.executable];
      case 'resolve_application_paths': assert.deepEqual(args.paths,[imported.executable]); return [imported];
      case 'save_settings': cfg.settings = args.settings; return cfg;
      case 'start_redirect': starts++; routing.starting = true; return 'Waiting for macOS';
      case 'stop_redirect': stops++; routing.running = false; routing.starting = false; return 'Stopped';
      case 'stop_tunnel': tunnelStops++; tunnelPhase = 'stopped'; return 'Stop requested';
      default: throw Error(command);
    }
  });
  await page.addInitScript(() => {window.__TAURI_INTERNALS__={invoke:(command,args) => window.mockInvoke(command,args)};});
  await page.goto(process.env.TEST_URL || 'http://127.0.0.1:1420/');
  await page.getByRole('button',{name:'Приложения',exact:true}).click();
  const start = page.getByRole('button',{name:'Включить перехват',exact:true});
  await page.getByText('Example Chat',{exact:true}).waitFor();
  assert(await start.isDisabled(),'empty selection must not start');
  assert.equal(await page.getByText('Для перехвата запустите Secret Tunnel от имени администратора.',{exact:true}).count(),0);
  await page.locator('.appTile').filter({hasText:'Example Chat'}).getByRole('checkbox').check();
  await page.getByRole('button',{name:'Сохранить выбор',exact:true}).click();
  await page.getByText('Выбор приложений сохранён',{exact:true}).waitFor();
  assert.deepEqual(cfg.settings.redirect_processes,apps[0].patterns,'include exact helper IDs');
  assert(await start.isEnabled(),'macOS + system SSH does not require administrator mode');
  await start.click();
  await page.getByText('Ждём разрешения macOS',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Отменить',exact:true}).click();
  await page.getByText('Перехват выключен',{exact:true}).waitFor();
  assert.equal(stops,1);
  await page.getByRole('button',{name:'Добавить .app',exact:true}).click();
  await page.getByText('Custom Browser',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Сохранить выбор',exact:true}).click();
  await page.getByText('Выбор приложений сохранён',{exact:true}).waitFor();
  assert.deepEqual([...cfg.settings.redirect_processes].sort(),[...apps[0].patterns,...imported.patterns].sort());
  await start.click();
  await page.getByText('Ждём разрешения macOS',{exact:true}).waitFor();
  routing.starting = false; routing.running = true;
  routing.setup_message = 'Системное расширение перенаправляет новые соединения выбранных приложений';
  await page.getByText('Перехват включён',{exact:false}).waitFor();
  const actionsVisible = () => page.locator('.appsControl').evaluate(el => {
    const box = el.getBoundingClientRect(); return box.top >= 0 && box.bottom <= window.innerHeight;
  });
  assert(await actionsVisible(),'routing actions must remain visible at 1120x780');
  const out = path.resolve(__dirname,'../output/playwright'); fs.mkdirSync(out,{recursive:true});
  await page.screenshot({path:path.join(out,'macos-applications.png'),fullPage:true});
  await page.setViewportSize({width:860,height:720});
  assert(await actionsVisible(),'routing actions must remain visible at the minimum window size');
  await page.screenshot({path:path.join(out,'macos-applications-860.png'),fullPage:true});
  await page.setViewportSize({width:1120,height:780});
  await page.getByRole('button',{name:'Выключить',exact:true}).click();
  await page.getByText('Перехват выключен',{exact:true}).waitFor();
  routing.component_available = false;
  await page.getByRole('link',{name:'ProxyBridge 4.1.2 или новее',exact:true}).waitFor();
  assert(await start.isDisabled(),'missing component must not start');
  assert.equal(starts,2);
  await page.getByRole('button',{name:'Домены',exact:true}).click();
  await page.getByText('Доменные правила применяются только в Windows.',{exact:false}).waitFor();
  assert(await page.getByRole('button',{name:'Включить перехват',exact:true}).isDisabled());
  await page.getByRole('button',{name:'Подключение',exact:true}).click();
  const connect = page.getByRole('button',{name:'Подключить',exact:true});
  const disconnect = page.getByRole('button',{name:'Отключить',exact:true});
  for (const [phase,label] of [['starting','Подключаемся'],['reconnecting','Переподключаемся'],['stopping','Отключаемся']]) {
    tunnelPhase = phase;
    await page.locator('.sideState').filter({hasText:label}).waitFor();
    assert.equal(await connect.count(),0,`${phase} must not offer a duplicate Connect`);
    assert(await disconnect.isEnabled(),`${phase} must allow Disconnect`);
  }
  tunnelPhase = 'starting';
  await page.locator('.sideState').filter({hasText:'Подключаемся'}).waitFor();
  await disconnect.click();
  await connect.waitFor();
  await page.waitForFunction(() => ![...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Подключить').disabled);
  assert.equal(tunnelStops,1,'Stop during startup reaches the backend');
  assert.equal(await disconnect.count(),0);
  assert.deepEqual(errors,[]);
  await browser.close();
  console.log('PASS: macOS app/helper selection, .app import, routing lifecycle, missing component, domains unavailable, startup/reconnect/stop UI cancellation; Tauri IPC mocked');
})().catch(async error => {console.error(error); if(browser) await browser.close(); process.exitCode=1;});
