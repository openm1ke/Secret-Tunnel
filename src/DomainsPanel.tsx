import { t, translateMessage, formatLocale, getLanguage } from "./i18n";
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
    <div className="panelTitle">{t("Домены ")}<span className="domainState">{p.running?t("Перехват включён"):t("Перехват выключен")}</span></div>
    <p>{t("Первое включённое совпадение сверху определяет маршрут. Домены имеют приоритет перед выбранными приложениями. Служебные соединения туннеля всегда идут напрямую.")}</p>
    <div className="domainTest">
      <label>{t("Проверить домен")}<input placeholder="cdn.example.com" value={domain} onChange={e=>setDomain(e.target.value)} /></label>
      <div role="status">{checking?t("Проверяю…"):!domain.trim()?t("Введите домен, чтобы проверить правила"):invalid?t("Исправьте ошибки в правилах или тестовом домене"):winner?t("Правило {0}: {1}", p.rules.indexOf(winner)+1, winner.action==="tunnel"?t("В туннель"):t("Напрямую")):t("Нет совпадений: действует выбор приложений")}</div>
    </div>
    <p className="domainHelp">{t("Regex применяется ко всему домену без учёта регистра. Пример: ")}<code>{String.raw`(?:.*\.)?example\.com`}</code> {t(" — домен и поддомены. Lookaround и обратные ссылки не поддерживаются. Вводите IDN в punycode.")}</p>
    <div className="domainList">
      {!p.rules.length&&<div className="notice">{t("Правил пока нет. Добавьте домен и выберите его маршрут.")}</div>}
      {p.rules.map((r,index)=>{
        const result=results[r.id]; const stats=p.stats?.rules.find(s=>s.id===r.id);
        return <article className={`domainRule${!r.enabled?" disabledRule":""}`} key={r.id}>
          <div className="domainRuleTop">
            <label className="domainToggle"><input type="checkbox" checked={r.enabled} onChange={e=>edit(r.id,{enabled:e.target.checked})} aria-label={t("Включить правило {0}", index+1)} />{index+1}</label>
            <label className="domainPattern">{t("Регулярное выражение")}<input aria-label={t("Регулярное выражение {0}", index+1)} value={r.pattern} maxLength={1024} placeholder={String.raw`(?:.*\.)?example\.com`} onChange={e=>edit(r.id,{pattern:e.target.value})} /></label>
            <label>{t("Маршрут")}<select aria-label={t("Маршрут {0}", index+1)} value={r.action} onChange={e=>edit(r.id,{action:e.target.value as Rule["action"]})}><option value="tunnel">{t("В туннель")}</option><option value="direct">{t("Напрямую")}</option></select></label>
            <div className="domainOrder"><button title={t("Выше")} aria-label={t("Поднять правило {0}", index+1)} disabled={index===0} onClick={()=>move(index,-1)}>↑</button><button title={t("Ниже")} aria-label={t("Опустить правило {0}", index+1)} disabled={index===p.rules.length-1} onClick={()=>move(index,1)}>↓</button><button title={t("Удалить")} aria-label={t("Удалить правило {0}", index+1)} onClick={()=>p.onChange(p.rules.filter(x=>x.id!==r.id))}>×</button></div>
          </div>
          <div className={result&&!result.valid?"domainError":"domainValidation"}>{checking?t("Проверяю…"):result?.error||(!domain.trim()?t("Синтаксис верен"):result?.match?t("Домен совпадает"):t("Домен не совпадает"))}</div>
          <div className="domainCounters">{t("Совпадений: ")}{stats?.observed??0} {t(" · В туннель: ")}{stats?.tunnel??0} {t(" · Напрямую: ")}{stats?.direct??0}</div>
        </article>;
      })}
    </div>
    <div className="actions left domainActions">
      <button disabled={p.rules.length>=128||p.busy} onClick={()=>p.onChange([...p.rules,{id:crypto.randomUUID(),pattern:"",enabled:true,action:"tunnel"}])}>{t("+ Добавить правило")}</button>
      <button className="primary" disabled={p.busy||checking||invalid} onClick={p.onSave}>{p.running?t("Сохранить и применить"):t("Сохранить правила")}</button>
      {!p.running?<button disabled={p.busy||checking||invalid||!p.canStart} onClick={p.onStart}>{t("Включить перехват")}</button>:<button disabled={p.busy} onClick={p.onStop}>{t("Выключить перехват")}</button>}
    </div>
    <p className="domainHelp">{t("Событий сеанса: ")}{p.stats?.events??0} {t(" · Домен неизвестен: ")}{p.stats?.unknown??0} {t(" · Потеряно строк журнала: ")}{p.stats?.dropped_output_lines??0}</p>
    <details className="domainDetails"><summary>{t("О счётчиках и определении доменов")}</summary><p className="domainHelp">{t("Показаны счётчики последнего применённого набора правил; они сбрасываются при запуске перехвата. Это решения для новых TCP/UDP-соединений с устранением повторов, не HTTP-запросы и не подтверждение доставки. Перекрывающиеся правила могут видеть одно совпадение; маршрут считается только победителю. При перегрузке журнала часть событий может не попасть в счётчики.")}</p>
    <p className="domainHelp">{t("Домен определяется из обычного DNS. DoH/DoT и общий IP нескольких сайтов ограничивают точность. Если домен неизвестен, действует выбор приложений. После применения переподключите приложения. DNS-кэш браузера может потребовать перезапуска браузера; для доменных правил отключите в нём защищённый DNS.")}</p></details>
  </section>;
}
