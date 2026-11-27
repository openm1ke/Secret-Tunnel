import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, ArrowUpRight, Globe, RefreshCcw } from "lucide-react";
import { Flag, countryName } from "./components";
import { openExternal } from "./AboutPanel";

type Offer = {
  id: string; provider: string; country: string; city: string; plan: string;
  price_minor: number; currency: string; cpu: number; ram_mb: number; disk_gb: number;
  port_mbps: number | null; conditions: string; verified_at: string; update_error: string | null;
  source_url: string; order_url: string; test_source_url: string; test_note: string; test_host?: string | null; price_note?: string | null; company_country?: string;
};
type Measurement = { id: string; median_ms: number | null; min_ms: number | null; max_ms: number | null; successes: number; attempts: number; family: string; error: string | null };
type Response = { catalog?: { offers: Offer[] }; source?: string; warning?: string; error?: string; results?: Measurement[] };

export default function HostingCatalog({ onClose, onPurchased, overlay = false, tunnelActive = false }: { onClose?: () => void; onPurchased: () => void; overlay?: boolean; tunnelActive?: boolean }) {
  const [offers, setOffers] = useState<Offer[]>([]), [results, setResults] = useState<Record<string, Measurement>>({});
  const [region, setRegion] = useState("all"), [sort, setSort] = useState(false);
  const [busy, setBusy] = useState(""), [message, setMessage] = useState(""), [measuredAt, setMeasuredAt] = useState("");
  const generation = useRef(0), dialog = useRef<HTMLDivElement>(null), wasBusy = useRef(false);
  const job = useRef("");
  const operation = useRef("");
  const request = async (action: string, ids: string[] = []) => JSON.parse(await invoke<string>("hosting_request", { request: JSON.stringify({ action, ids, job: job.current }) })) as Response;
  const cancel = (reason = "Замер отменён.") => {
    generation.current++; setBusy(""); wasBusy.current = false; operation.current = ""; setMessage(reason);
    void request("cancel").catch(() => undefined);
  };
  async function run(action: string) {
    if (wasBusy.current) return;
    wasBusy.current = true; operation.current = action; setBusy(action); setMessage(""); const token = ++generation.current;
    if (action === "probe") { job.current = crypto.randomUUID(); setResults({}); setMeasuredAt(""); }
    try {
      const response = await request(action, visible.map(o => o.id));
      if (token !== generation.current) return;
      if (response.catalog) { setOffers(response.catalog.offers); setResults({}); setMeasuredAt(""); }
      if (response.results) { setResults(Object.fromEntries(response.results.map(r => [r.id, r]))); setMeasuredAt(new Date().toLocaleTimeString("ru")); }
      setMessage(response.error ?? response.warning ?? (response.source === "bundled" ? "Встроенная копия. Дата проверки указана у каждого тарифа." : response.source === "cache" ? "Сохранённая копия каталога." : response.source === "network" ? "Каталог обновлён из сети." : ""));
    } catch (e) { if (token === generation.current) setMessage(String(e)); }
    finally { if (token === generation.current) { wasBusy.current = false; operation.current = ""; setBusy(""); } }
  }
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    let mounted = true;
    dialog.current?.focus();
    void (async () => { await run("load"); if (mounted) await run("refresh"); })();
    const network = () => { cancel("Сеть изменилась. Повторите замер."); setResults({}); setMeasuredAt(""); };
    const hidden = () => { if (document.hidden && wasBusy.current) cancel("Приложение свёрнуто. Повторите замер."); };
    window.addEventListener("offline", network); window.addEventListener("online", network); document.addEventListener("visibilitychange", hidden);
    return () => { mounted = false; generation.current++; wasBusy.current = false; void request("cancel").catch(() => undefined); previous?.focus(); window.removeEventListener("offline", network); window.removeEventListener("online", network); document.removeEventListener("visibilitychange", hidden); };
  }, []);
  useEffect(() => { if (tunnelActive && operation.current === "probe") cancel("Туннель включён. Замер отменён."); }, [tunnelActive]);
  const visible = offers.filter(o => region === "all" || o.country === region)
    .sort((a, b) => sort ? (results[a.id]?.median_ms ?? Infinity) - (results[b.id]?.median_ms ?? Infinity) : 0);
  const countries = [...new Set(offers.map(o => o.country))].sort();
  const content = <section className="catalogPage" role={overlay ? "dialog" : undefined} aria-modal={overlay ? true : undefined} aria-labelledby="hosting-title" tabIndex={-1} ref={dialog}
    onKeyDown={e => {
      if (e.key === "Escape" && overlay) onClose?.();
      if (e.key === "Tab" && overlay) {
        const items = [...(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), summary') ?? [])];
        if (e.shiftKey && (document.activeElement === items[0] || document.activeElement === dialog.current)) { e.preventDefault(); items[items.length - 1]?.focus(); }
        else if (!e.shiftKey && document.activeElement === items[items.length - 1]) { e.preventDefault(); items[0]?.focus(); }
      }
    }}>
    {overlay && <button className="ghost catalogBack" onClick={onClose}><ArrowLeft size={16}/>Назад к настройке</button>}
    <header className="catalogHeader">
      <div><span className="catalogEyebrow">ВЫБОР СВОЕГО VPS</span><h1 id="hosting-title">Каталог серверов</h1>
        <p>Сравните локации и задержку. Выберите хостера и настройте SSH после покупки.</p></div>
      <span className="catalogCount"><Globe size={17}/>Провайдеры: {offers.length} · страны: {countries.length}</span>
    </header>
    <div className="catalogToolbar">
      <div className="catalogFilters" role="group" aria-label="Страна сервера">
        <button aria-pressed={region === "all"} disabled={busy === "probe"} onClick={() => setRegion("all")}>Все страны</button>
        {countries.map(code => <button key={code} aria-pressed={region === code} disabled={busy === "probe"} onClick={() => setRegion(code)}><Flag code={code}/>{countryName(code, code)}</button>)}
      </div>
      <div className="catalogActions">
        <button className="primary" disabled={!!busy && busy !== "probe" || !offers.length} onClick={() => busy === "probe" ? cancel() : void run("probe")}>{busy === "probe" ? "Отменить замер" : "Проверить задержку"}</button>
        <button className="catalogRefresh" aria-label="Обновить каталог" title="Обновить каталог" disabled={!!busy} onClick={() => void run("refresh")}><RefreshCcw size={17}/></button>
      </div>
    </div>
    <div className="catalogStatus" role="status" aria-live="polite">{busy ? busy === "probe" ? "Измеряем задержку…" : "Обновляем данные…" : message}</div>
    {measuredAt && <label className="catalogSort"><input type="checkbox" checked={sort} onChange={e => setSort(e.target.checked)}/>Сначала с меньшей задержкой · замер в {measuredAt}</label>}
    <div className="catalogCards">{visible.map(o => {
      const r = results[o.id];
      const price = (o.price_minor / 100).toLocaleString("ru", { maximumFractionDigits: 2 });
      const currency = ({ RUB: "₽", EUR: "€", USD: "$" })[o.currency] ?? o.currency;
      const speed = o.port_mbps ? o.port_mbps % 1000 === 0 ? `${o.port_mbps / 1000} Гбит/с` : `${o.port_mbps} Мбит/с` : "Не указана";
      return <article className="catalogCard" key={o.id}>
        <div className="catalogIdentity"><span className="catalogFlag" aria-hidden="true"><Flag code={o.country}/></span><div><h2>{o.provider}</h2><p>{countryName(o.country, o.country)} · {o.city}</p></div></div>
        <div className="catalogMetrics">
          <div><strong>{price} <span>{currency}</span></strong><small>в месяц</small></div>
          <div className={`catalogLatency ${!o.test_host || r && r.median_ms === null ? "unavailable" : ""}`}><strong>{!o.test_host ? "Нет узла" : r ? r.median_ms !== null ? <>{r.median_ms.toFixed(0)} <span>мс</span></> : "Нет замера" : "—"}</strong><small>{o.test_host ? "задержка TCP" : "замер недоступен"}</small></div>
        </div>
        <p className="catalogPriceNote">{o.price_note ?? "Условия цены — у хостера"}</p>
        <div className="catalogPort"><span>Скорость порта</span><strong>{o.port_mbps ? "до " : ""}{speed}</strong></div>
        <p className="catalogHardware">{o.cpu} vCPU <i/> {o.ram_mb / 1024} ГБ RAM <i/> {o.disk_gb} ГБ</p>
        <button className="catalogVisit" onClick={() => void openExternal(o.order_url)}>Сайт хостера<ArrowUpRight size={17}/></button>
        <details className="catalogDetails"><summary>Подробнее</summary>
          <div><b>{o.plan}</b><p>{o.conditions}</p>
            {r && <p>{r.median_ms !== null ? `TCP · ${r.min_ms?.toFixed(0)}–${r.max_ms?.toFixed(0)} мс · ${r.successes}/${r.attempts} ответов · ${r.family}` : r.error}</p>}
            <p>{o.test_note}</p><p>Проверено: {o.verified_at.slice(0,10)}</p>
            {o.company_country && o.company_country !== "Не уточнено" && <p>Страна компании: {o.company_country}</p>}
            <button className="ghost" onClick={() => void openExternal(o.source_url)}>Источник цены ↗</button><button className="ghost" onClick={() => void openExternal(o.test_source_url)}>Тестовый узел ↗</button>
          </div>
        </details>
        {o.update_error && <small className="catalogWarning">Цена требует обновления</small>}
      </article>;
    })}</div>
    <footer className="catalogFooter"><details className="catalogDetails"><summary>Как сравнивать серверы</summary><p>Флаг — страна сервера. Задержка — медиана пяти TCP-подключений из текущей сети, не ICMP-пинг. Другой VPN влияет на результат. Скорость порта заявлена хостером и не гарантирует скорость SSH. Тестовый узел и будущий VPS могут различаться. Цены в исходной валюте; итоговая сумма — у хостера.</p></details>
      <button onClick={onPurchased}>Сервер уже куплен — настроить SSH</button></footer>
  </section>;
  return overlay ? <div className="catalogOverlay">{content}</div> : content;
}
