// UI integration test with a mocked Tauri transport; backend semantics are tested in Rust/C.
const {chromium}=require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const path=require('path');
const fs=require('fs');
let browser;
(async()=>{
 const apps=[{id:'discord:discord',name:'Discord',executable:'Discord.exe',patterns:['Discord.exe'],source:'fixture'}];
 const discord=apps.find(app=>app.id==='discord:discord');
 const cfg={selected_server_id:'test',selected_proxy_id:'local',servers:[{id:'test',name:'Server',host:'server.test',port:22,user:'user',key_path:'key',compression:false,strict_host_key_checking:'yes',connect_timeout:8,server_alive_interval:15,server_alive_count_max:3}],proxies:[{id:'local',name:'Local',host:'127.0.0.1',port:1080}],settings:{ipinfo_token:'',ssh_engine:'embedded',hide_sensitive:true,verify_proxy_on_start:true,auto_reconnect:false,reconnect_delay_seconds:3,start_tunnel_on_launch:false,clear_logs_on_start:false,redirect_cli_path:'component.exe',redirect_processes:discord.patterns,domain_rules:[]}};
 let captures=false;
 browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_EXECUTABLE ? {executablePath:process.env.PLAYWRIGHT_EXECUTABLE} : {})});
 const page=await browser.newPage({viewport:{width:1120,height:900}});
 const errors=[];page.on('pageerror',error=>errors.push(error.message));
 await page.exposeFunction('mockInvoke',(command,args)=>{
  switch(command){
   case 'get_config':return cfg;
   case 'tunnel_status':return {state:'running',state_emoji:'🟢',app_version:'test',ssh_engine:'embedded',server_name:'Server',proxy_host:'127.0.0.1',proxy_port:1080,rx_total:0,tx_total:0,ip:'',country_code:'',country_name:'',country_flag:'',last_error:''};
   case 'traffic_history':case 'get_logs':return [];
   case 'redirect_status':return {running:captures,starting:false,platform:'windows',requires_administrator:true,supports_system_ssh:false,supports_domains:true,setup_message:'',pid:null,administrator:true,component_available:true,last_error:''};
   case 'scan_installed_applications':return apps;
   case 'check_domain_rule': { if(args.pattern.includes('(?=')||args.pattern.includes('[')&&!args.pattern.includes(']'))throw Error('Invalid regex'); const re=new RegExp('^(?:'+args.pattern+')$','i');return !!args.domain&&re.test(args.domain.replace(/\.$/,'')); }
   case 'save_settings':cfg.settings=args.settings;return cfg;
   case 'start_redirect':captures=true;return 'Capture started';
   case 'stop_redirect':captures=false;return 'Capture stopped';
   default:throw Error(command);
  }
 });
 await page.addInitScript(()=>{window.__TAURI_INTERNALS__={invoke:(command,args)=>window.mockInvoke(command,args)};});
 await page.goto('http://127.0.0.1:1420/');

 await page.getByRole('button',{name:'Домены',exact:true}).click();
 await page.getByRole('button',{name:'+ Добавить правило',exact:true}).click();
 const pattern=page.getByRole('textbox',{name:'Регулярное выражение 1',exact:true});
 await pattern.fill('[');
 await page.getByText('Error: Invalid regex',{exact:true}).waitFor();
 if(!await page.getByRole('button',{name:'Сохранить правила',exact:true}).isDisabled())throw Error('Invalid regex can be saved');
 await pattern.fill(String.raw`(?:.*\.)?example\.com`);
 await page.getByRole('textbox',{name:'Проверить домен',exact:true}).fill('cdn.example.com');
 await page.getByText('Правило 1: В туннель',{exact:true}).waitFor();
 await page.getByRole('combobox',{name:'Маршрут 1',exact:true}).selectOption('direct');
 await page.getByText('Правило 1: Напрямую',{exact:true}).waitFor();
 await page.getByRole('button',{name:'+ Добавить правило',exact:true}).click();
 await page.getByRole('textbox',{name:'Регулярное выражение 2',exact:true}).fill('.*');
 await page.getByText('Правило 1: Напрямую',{exact:true}).waitFor();
 await page.getByRole('button',{name:'Поднять правило 2',exact:true}).click();
 await page.getByText('Правило 1: В туннель',{exact:true}).waitFor();
 await page.getByRole('checkbox',{name:'Включить правило 1',exact:true}).uncheck();
 await page.getByText('Правило 2: Напрямую',{exact:true}).waitFor();
 await page.getByRole('button',{name:'Сохранить правила',exact:true}).click();
 await page.getByText('Правила доменов сохранены. Включите перехват для применения',{exact:true}).waitFor();
 if(cfg.settings.domain_rules.length!==2||cfg.settings.domain_rules[0].enabled||cfg.settings.domain_rules[1].action!=='direct')throw Error('Save mismatch');
 await page.getByRole('button',{name:'Включить перехват',exact:true}).click();
 await page.getByRole('button',{name:'Сохранить и применить',exact:true}).waitFor();
 await page.getByRole('checkbox',{name:'Включить правило 1',exact:true}).check();
 await page.getByText('Правило 1: В туннель',{exact:true}).waitFor();
 await page.getByRole('button',{name:'Сохранить и применить',exact:true}).click();
 await page.getByText('Capture started',{exact:true}).waitFor();
 if(!captures)throw Error('Apply did not restart capture');
 await page.getByText('Правило 1: В туннель',{exact:true}).waitFor();
 const output=path.join(__dirname,'../output/playwright');fs.mkdirSync(output,{recursive:true});
 await page.screenshot({path:path.join(output,'domains-rules.png'),fullPage:true});
 await page.setViewportSize({width:1120,height:780});
 await page.screenshot({path:path.join(process.env.TEMP||'/tmp','secret-tunnel-domains-780.png'),fullPage:true});
 if(errors.length)throw Error(errors.join(';'));
 console.log('PASS: domains UI invalid regex, test host, action, priority, disabled rule, save, start, apply; Tauri transport mocked');
 await browser.close();
})().catch(async error=>{console.error(error);if(browser)await browser.close();process.exitCode=1;});
