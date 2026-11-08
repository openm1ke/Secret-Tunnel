import {useEffect, useState} from "react";
import {invoke} from "@tauri-apps/api/core";

type Rule = {id:string; pattern:string; enabled:boolean; action:"tunnel"|"direct"};
type Stats = {events:number; unknown:number; dropped_output_lines?:number; rules:{id:string; observed:number; tunnel:number; direct:number}[]};
type Props = {rules:Rule[]; stats?:Stats; running:boolean; busy:boolean; canStart:boolean;
  onChange:(rules:Rule[])=>void; onSave:()=>void; onStart:()=>void; onStop:()=>void};

export default function DomainsPanel(p:Props) {
  const [domain,setDomain]=useState("");
  const [results,setResults]=useState<Record<string,{valid:boolean; match:boolean; error?:string}>>({});
  const [checking,setChecking]=useState(false);
  const validationKey=JSON.stringify(p.rules.map(r=>[r.id,r.pattern]));
  useEffect(()=>{
    let cancelled=false;
    setChecking(true);
    const timer=setTimeout(async()=>{
      const entries=await Promise.all(p.rules.map(async r=>{
        try {const match=await invoke<boolean>("check_domain_rule",{pattern:r.pattern,domain}); return [r.id,{valid:true,match}] as const;}
        catch(error) {return [r.id,{valid:false,match:false,error:String(error)}] as const;}
      }));
      if(!cancelled){setResults(Object.fromEntries(entries));setChecking(false);}
    },250);
    return ()=>{cancelled=true;clearTimeout(timer);};
  },[validationKey,domain]);
  const edit=(id:string,patch:Partial<Rule>)=>p.onChange(p.rules.map(r=>r.id===id?{...r,...patch}:r));
  const move=(index:number,delta:number)=>{const next=[...p.rules];[next[index],next[index+delta]]=[next[index+delta],next[index]];p.onChange(next);};
  const winner=domain.trim()&&!checking?p.rules.find(r=>r.enabled&&results[r.id]?.match):undefined;
  const invalid=p.rules.some(r=>!results[r.id]?.valid);
  return <section className="panel narrow applicationsPanel domainPanel">
    <div className="panelTitle">Домены <span className="domainState">{p.running?"Перехват включён":"Перехват выключен"}</span></div>
    <p>Первое включённое совпадение сверху определяет маршрут. Домены имеют приоритет перед выбранными приложениями. Служебные соединения туннеля всегда идут напрямую.</p>
    <div className="domainTest">
      <label>Проверить домен<input placeholder="cdn.example.com" value={domain} onChange={e=>setDomain(e.target.value)} /></label>
      <div role="status">{checking?"Проверяю…":!domain.trim()?"Введите домен, чтобы проверить правила":invalid?"Исправьте ошибки в правилах или тестовом домене":winner?`Правило ${p.rules.indexOf(winner)+1}: ${winner.action==="tunnel"?"В туннель":"Напрямую"}`:"Нет совпадений: действует выбор приложений"}</div>
    </div>
    <p className="domainHelp">Regex применяется ко всему домену без учёта регистра. Пример: <code>{String.raw`(?:.*\.)?example\.com`}</code> — домен и поддомены. Lookaround и обратные ссылки не поддерживаются. Вводите IDN в punycode.</p>
    <div className="domainList">
      {!p.rules.length&&<div className="notice">Правил пока нет. Добавьте домен и выберите его маршрут.</div>}
      {p.rules.map((r,index)=>{
        const result=results[r.id]; const stats=p.stats?.rules.find(s=>s.id===r.id);
        return <article className={`domainRule${!r.enabled?" disabledRule":""}`} key={r.id}>
          <div className="domainRuleTop">
            <label className="domainToggle"><input type="checkbox" checked={r.enabled} onChange={e=>edit(r.id,{enabled:e.target.checked})} aria-label={`Включить правило ${index+1}`} />{index+1}</label>
            <label className="domainPattern">Регулярное выражение<input aria-label={`Регулярное выражение ${index+1}`} value={r.pattern} maxLength={1024} placeholder={String.raw`(?:.*\.)?example\.com`} onChange={e=>edit(r.id,{pattern:e.target.value})} /></label>
            <label>Маршрут<select aria-label={`Маршрут ${index+1}`} value={r.action} onChange={e=>edit(r.id,{action:e.target.value as Rule["action"]})}><option value="tunnel">В туннель</option><option value="direct">Напрямую</option></select></label>
            <div className="domainOrder"><button title="Выше" aria-label={`Поднять правило ${index+1}`} disabled={index===0} onClick={()=>move(index,-1)}>↑</button><button title="Ниже" aria-label={`Опустить правило ${index+1}`} disabled={index===p.rules.length-1} onClick={()=>move(index,1)}>↓</button><button title="Удалить" aria-label={`Удалить правило ${index+1}`} onClick={()=>p.onChange(p.rules.filter(x=>x.id!==r.id))}>×</button></div>
          </div>
          <div className={result&&!result.valid?"domainError":"domainValidation"}>{checking?"Проверяю…":result?.error||(!domain.trim()?"Синтаксис верен":result?.match?"Домен совпадает":"Домен не совпадает")}</div>
          <div className="domainCounters">Совпадений: {stats?.observed??0} · В туннель: {stats?.tunnel??0} · Напрямую: {stats?.direct??0}</div>
        </article>;
      })}
    </div>
    <div className="actions left domainActions">
      <button disabled={p.rules.length>=128||p.busy} onClick={()=>p.onChange([...p.rules,{id:crypto.randomUUID(),pattern:"",enabled:true,action:"tunnel"}])}>+ Добавить правило</button>
      <button className="primary" disabled={p.busy||checking||invalid} onClick={p.onSave}>{p.running?"Сохранить и применить":"Сохранить правила"}</button>
      {!p.running?<button disabled={p.busy||checking||invalid||!p.canStart} onClick={p.onStart}>Включить перехват</button>:<button disabled={p.busy} onClick={p.onStop}>Выключить перехват</button>}
    </div>
    <p className="domainHelp">Событий сеанса: {p.stats?.events??0} · Домен неизвестен: {p.stats?.unknown??0} · Потеряно строк журнала: {p.stats?.dropped_output_lines??0}</p>
    <details className="domainDetails"><summary>О счётчиках и определении доменов</summary><p className="domainHelp">Показаны счётчики последнего применённого набора правил; они сбрасываются при запуске перехвата. Это решения для новых TCP/UDP-соединений с устранением повторов, не HTTP-запросы и не подтверждение доставки. Перекрывающиеся правила могут видеть одно совпадение; маршрут считается только победителю. При перегрузке журнала часть событий может не попасть в счётчики.</p>
    <p className="domainHelp">Домен определяется из обычного DNS. DoH/DoT и общий IP нескольких сайтов ограничивают точность. Если домен неизвестен, действует выбор приложений. После применения переподключите приложения. DNS-кэш браузера может потребовать перезапуска браузера; для доменных правил отключите в нём защищённый DNS.</p></details>
  </section>;
}
