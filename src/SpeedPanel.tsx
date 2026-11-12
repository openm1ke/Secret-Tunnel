import React, { useEffect, useMemo, useState } from "react";
import { CircleStop, Eye, EyeOff, Play, Trash2 } from "lucide-react";
import { Flag, RouteNode, RouteRibbon, countryName } from "./components";

export type SpeedResults = {
  ping_ms?: number | null;
  ping_min_ms?: number | null;
  jitter_ms?: number | null;
  loaded_ping_ms?: number | null;
  dns_ms?: number | null;
  dns_loss?: number | null;
  download_single_mbps?: number | null;
  download_mbps?: number | null;
  upload_mbps?: number | null;
  requests_per_second?: number | null;
  request_ms?: number | null;
  mixed_download_mbps?: number | null;
  mixed_request_ms?: number | null;
  download_source?: string | null;
  server_rtt_ms?: number | null;
  bytes_down: number;
  bytes_up: number;
};
export type SpeedRun = {
  id: string; kind: string; started_at: number; finished_at: number; server: string; route: string[]; country: string;
  mode?: string; exit_ip?: string; exit_country?: string; edge?: string; provider?: string;
  results: SpeedResults; error: string; cancelled: boolean;
};
export type SpeedLive = {
  running: boolean; kind: string; mode: string; provider: string; exit_ip: string; exit_country: string; edge: string;
  steps: string[]; step: number; progress: number;
  down_mbps: number; up_mbps: number; ping_ms: number | null; timeline: [number, number, number][];
  results: SpeedResults; last: SpeedRun | null; error: string;
};

export const emptyLive: SpeedLive = { running: false, kind: "", mode: "", provider: "", exit_ip: "", exit_country: "", edge: "", steps: [], step: 0, progress: 0, down_mbps: 0, up_mbps: 0, ping_ms: null, timeline: [], results: { bytes_down: 0, bytes_up: 0 }, last: null, error: "" };

const kinds: { id: string; title: string; text: string; time: string; traffic: string }[] = [
  { id: "quick", title: "Быстрый", text: "Пинг, загрузка и отдача в один поток.", time: "≈ 15 с", traffic: "до 150 МБ" },
  { id: "full", title: "Полный", text: "Все проверки по очереди: пинг, UDP, один и шесть потоков, отдача, короткие запросы.", time: "≈ 50 с", traffic: "до 400 МБ" },
  { id: "bandwidth", title: "Ширина канала", text: "8 потоков вниз и 4 вверх, как торрент или облачная синхронизация. Пинг под нагрузкой.", time: "≈ 25 с", traffic: "до 400 МБ" },
  { id: "browsing", title: "Как браузер", text: "60 коротких запросов, каждый по новому соединению, и DNS. Отвечает, насколько быстро открываются страницы.", time: "≈ 15 с", traffic: "≈ 5 МБ" },
  { id: "mixed", title: "Смешанный", text: "Две длинные загрузки, поток коротких запросов и пинг одновременно: несколько программ сразу.", time: "≈ 20 с", traffic: "до 200 МБ" },
  { id: "latency", title: "Задержка и UDP", text: "30 пингов по открытому соединению и 30 DNS-запросов по UDP: звонки и игры.", time: "≈ 15 с", traffic: "< 1 МБ" },
  { id: "mlab", title: "M-Lab NDT7", text: "Методика M-Lab: по 10 секунд загрузки и отдачи одним соединением до ближайшего сервера. M-Lab публикует результаты вместе с IP-адресом в открытом наборе данных.", time: "≈ 25 с", traffic: "до 300 МБ" },
];
const providers = [
  { id: "cloudflare", title: "Cloudflare", hint: "speed.cloudflare.com: узел Cloudflare рядом с точкой выхода" },
  { id: "librespeed", title: "LibreSpeed", hint: "Открытые серверы LibreSpeed: выбирается тот, что быстрее отвечает на этом пути" },
];
const isDirect = (run: { mode?: string }) => run.mode === "direct";
const kindTitle = (id: string) => kinds.find((kind) => kind.id === id)?.title ?? id;

const speedTicks = [0, 5, 10, 25, 50, 100, 250, 500, 1000];
const pingTicks = [0, 25, 50, 100, 200, 400, 800];
/** Position 0..1 on a scale whose ticks are evenly spaced. */
function position(value: number, ticks: number[]) {
  if (value <= ticks[0]) return 0;
  for (let i = 1; i < ticks.length; i++) {
    if (value <= ticks[i]) return (i - 1 + (value - ticks[i - 1]) / (ticks[i] - ticks[i - 1])) / (ticks.length - 1);
  }
  return 1;
}

const SWEEP = 240;
function polar(cx: number, cy: number, r: number, fraction: number) {
  const angle = ((-SWEEP / 2 + fraction * SWEEP) * Math.PI) / 180;
  return [cx + r * Math.sin(angle), cy - r * Math.cos(angle)];
}
function arc(cx: number, cy: number, r: number, from: number, to: number) {
  const [x1, y1] = polar(cx, cy, r, from);
  const [x2, y2] = polar(cx, cy, r, to);
  const large = (to - from) * SWEEP > 180 ? 1 : 0;
  return `M ${x1} ${y1} A ${r} ${r} 0 ${large} 1 ${x2} ${y2}`;
}

function formatMbps(value: number | null | undefined) {
  if (value === null || value === undefined) return "—";
  return value >= 100 ? value.toFixed(0) : value.toFixed(1);
}
function formatMs(value: number | null | undefined) {
  return value === null || value === undefined ? "—" : `${Math.round(value)} мс`;
}

/** A speedometer: the arc fills to the value, ticks on a 0–1000 Mbit/s scale. */
function Gauge({ label, value, tone, active }: { label: string; value: number; tone: "rx" | "tx"; active: boolean }) {
  const fraction = position(value, speedTicks);
  return (
    <figure className={`gauge ${tone}${active ? " active" : ""}`}>
      <svg viewBox="0 0 200 168" role="img" aria-label={`${label}: ${formatMbps(value)} Мбит/с`}>
        <path className="track" d={arc(100, 100, 80, 0, 1)} />
        <path className="value" d={arc(100, 100, 80, 0, 1)} pathLength={1} strokeDasharray={`${Math.max(fraction, 0.001)} 1`} />
        {speedTicks.map((tick, index) => {
          const f = index / (speedTicks.length - 1);
          const [x1, y1] = polar(100, 100, 92, f);
          const [x2, y2] = polar(100, 100, 98, f);
          const [tx, ty] = polar(100, 100, 108, f);
          return (
            <g key={tick} className="tick">
              <line x1={x1} y1={y1} x2={x2} y2={y2} />
              <text x={tx} y={ty + 3}>{tick >= 1000 ? "1к" : tick}</text>
            </g>
          );
        })}
        <line className="needle" x1={100} y1={100} x2={100} y2={38} style={{ transform: `rotate(${-SWEEP / 2 + fraction * SWEEP}deg)` }} />
        <circle className="hub" cx={100} cy={100} r={5} />
        <text className="reading" x={100} y={142}>{value > 0 ? formatMbps(value) : "—"}</text>
        <text className="unit" x={100} y={158}>Мбит/с</text>
      </svg>
      <figcaption>{label}</figcaption>
    </figure>
  );
}

function PingMeter({ label, value, hint }: { label: string; value: number | null | undefined; hint: string }) {
  const fraction = value === null || value === undefined ? 0 : position(value, pingTicks);
  const level = value === null || value === undefined ? "" : value < 60 ? "good" : value < 150 ? "fair" : "poor";
  return (
    <div className={`pingMeter ${level}`} title={hint}>
      <span>{label}</span>
      <strong>{formatMs(value)}</strong>
      <i aria-hidden="true"><b style={{ width: `${Math.max(fraction * 100, value ? 3 : 0)}%` }} /></i>
    </div>
  );
}

function Timeline({ points }: { points: [number, number, number][] }) {
  if (points.length < 2) return <div className="speedTimeline empty" />;
  const start = points[0][0];
  const end = points[points.length - 1][0];
  const top = Math.max(10, ...points.map(([, down, up]) => Math.max(down, up)));
  const path = (index: 1 | 2) => points.map((point, i) => `${i ? "L" : "M"} ${((point[0] - start) / Math.max(end - start, 1)) * 600} ${60 - (point[index] / top) * 56}`).join(" ");
  return (
    <svg className="speedTimeline" viewBox="0 0 600 62" preserveAspectRatio="none" aria-hidden="true">
      <path className="rx" d={path(1)} />
      <path className="tx" d={path(2)} />
    </svg>
  );
}

function when(epoch: number) {
  const date = new Date(epoch * 1000);
  return `${date.toLocaleDateString("ru-RU", { day: "numeric", month: "short" })}, ${date.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" })}`;
}

const MODE_KEY = "secret-tunnel.speed-mode";
const PROVIDER_KEY = "secret-tunnel.speed-provider";
function remembered(key: string, fallback: string) {
  try {
    return localStorage.getItem(key) || fallback;
  } catch {
    return fallback;
  }
}
function remember(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* per-viewer convenience only */
  }
}

function maskIp(ip: string) {
  return ip.includes(":") ? "••••::••••" : "•••.•••.•••.•••";
}

/** The servers a tunnel run went through: one server, or a chain. */
function chainOf(run: { route: string[]; server: string }) {
  return run.route.length ? run.route : run.server ? [run.server] : [];
}

function PathCell({ run }: { run: SpeedRun }) {
  if (isDirect(run)) return <span className="pathBadge direct">без туннеля</span>;
  const chain = chainOf(run);
  return (
    <span className="pathCell">
      <span className="chain">{chain.map((hop, index) => <React.Fragment key={index}>{index > 0 && <i>→</i>}<b>{hop}</b></React.Fragment>)}</span>
      <span className={chain.length > 1 ? "pathBadge chainBadge" : "pathBadge"}>{chain.length > 1 ? `цепочка · ${chain.length} звена` : "1 сервер"}</span>
    </span>
  );
}

function ExitCell({ run, hide }: { run: SpeedRun; hide: boolean }) {
  const country = run.exit_country || run.country;
  if (!country && !run.exit_ip) return <>—</>;
  return (
    <span className="exitCell" title={run.exit_ip ? `IP выхода: ${hide ? maskIp(run.exit_ip) : run.exit_ip}` : undefined}>
      {country && <Flag code={country} label={countryName(country, "")} />} {countryName(country, country)}
      {run.exit_ip && <small>{hide ? maskIp(run.exit_ip) : run.exit_ip}</small>}
    </span>
  );
}

const downOf = (results: SpeedResults) => results.download_mbps ?? results.mixed_download_mbps ?? results.download_single_mbps ?? null;
function delta(tunnel: number | null | undefined, direct: number | null | undefined, lowerIsBetter = false) {
  if (!tunnel || !direct) return null;
  const change = ((tunnel - direct) / direct) * 100;
  const good = lowerIsBetter ? change <= 0 : change >= 0;
  return <em className={good ? "better" : "worse"}>{change > 0 ? "+" : ""}{change.toFixed(0)} %</em>;
}

/** The latest tunnel run beside the latest run without it, same service when possible. */
function Comparison({ history }: { history: SpeedRun[] }) {
  const latestTunnel = [...history].reverse().find((run) => !isDirect(run) && run.kind !== "mlab");
  if (!latestTunnel) return null;
  const service = (run: SpeedRun) => (run.provider || "").split(" · ")[0];
  const directRuns = [...history].reverse().filter((run) => isDirect(run) && run.kind !== "mlab");
  const latestDirect = directRuns.find((run) => service(run) === service(latestTunnel)) ?? directRuns[0];
  if (!latestDirect) return null;
  const rows: [string, number | null | undefined, number | null | undefined, (v: number) => string, boolean][] = [
    ["Скачивание", downOf(latestTunnel.results), downOf(latestDirect.results), (v) => `${formatMbps(v)} Мбит/с`, false],
    ["Отдача", latestTunnel.results.upload_mbps, latestDirect.results.upload_mbps, (v) => `${formatMbps(v)} Мбит/с`, false],
    ["Пинг", latestTunnel.results.ping_ms, latestDirect.results.ping_ms, (v) => formatMs(v), true],
    ["Под нагрузкой", latestTunnel.results.loaded_ping_ms, latestDirect.results.loaded_ping_ms, (v) => formatMs(v), true],
  ];
  return (
    <section className="panel compare">
      <header className="panelHead">
        <h3>Туннель и прямое соединение</h3>
        <span className="chip">{service(latestTunnel)}{service(latestDirect) !== service(latestTunnel) ? ` / ${service(latestDirect)}` : ""}</span>
      </header>
      <table className="statsTable">
        <thead><tr><th /><th className="num">Через туннель<small>{when(latestTunnel.started_at)}</small></th><th className="num">Без туннеля<small>{when(latestDirect.started_at)}</small></th><th className="num">Разница</th></tr></thead>
        <tbody>
          {rows.filter(([, a, b]) => a || b).map(([label, tunnel, direct, format, lower]) => (
            <tr key={label}><td>{label}</td><td className="num">{tunnel ? format(tunnel) : "—"}</td><td className="num">{direct ? format(direct) : "—"}</td><td className="num">{delta(tunnel, direct, lower) ?? "—"}</td></tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

export default function SpeedPanel({ connected, live, history, serverPing, tunnelRoute, tunnelServer, tunnelCountry, hideAddresses, onStart, onCancel, onClear }: {
  connected: boolean; live: SpeedLive; history: SpeedRun[]; serverPing: number | null;
  tunnelRoute: string[]; tunnelServer: string; tunnelCountry: string; hideAddresses: boolean;
  onStart: (kind: string, direct: boolean, provider: string) => void; onCancel: () => void; onClear: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [mode, setMode] = useState(() => remembered(MODE_KEY, "tunnel"));
  const [provider, setProvider] = useState(() => remembered(PROVIDER_KEY, "cloudflare"));
  const [hide, setHide] = useState(hideAddresses);
  useEffect(() => setHide(hideAddresses), [hideAddresses]);
  const direct = mode === "direct";
  const current: SpeedRun | undefined = live.running
    ? { id: "", kind: live.kind, started_at: 0, finished_at: 0, server: tunnelServer, route: tunnelRoute, country: tunnelCountry, mode: live.mode, exit_ip: live.exit_ip, exit_country: live.exit_country, edge: live.edge, provider: live.provider, results: live.results, error: "", cancelled: false }
    : live.last && isDirect(live.last) === direct ? live.last : [...history].reverse().find((run) => isDirect(run) === direct);
  const shown = current?.results;
  const best = useMemo(() => ({ down: shown ? downOf(shown) : null, up: shown?.upload_mbps ?? null }), [shown]);
  const runs = [...history].reverse();
  const top = Math.max(1, ...history.map((run) => downOf(run.results) ?? 0));
  const downloading = live.running && live.down_mbps > 0;
  const uploading = live.running && live.up_mbps > 0;
  // The path being measured, or the one the next run takes.
  const pathDirect = live.running ? live.mode === "direct" : direct;
  const hops = pathDirect ? [] : tunnelRoute.length ? tunnelRoute : tunnelServer ? [tunnelServer] : [];
  const exitCountry = current?.exit_country || (!pathDirect ? tunnelCountry : "");
  const nodes: RouteNode[] = [
    { label: "Этот Mac", kind: "device" },
    ...(pathDirect ? [{ label: "Текущее соединение", kind: "hop" as const }] : hops.map((hop) => ({ label: hop, kind: "hop" as const }))),
    { label: "Интернет", kind: "internet", flag: exitCountry || undefined },
  ];
  const canRun = () => !live.running && (direct || connected);

  return (
    <section className="speedPage">
      <header className="panelHead">
        <div>
          <h2>Скорость</h2>
          <p className="panelIntro">Замеры идут через публичные тестеры скорости: Cloudflare, LibreSpeed или M-Lab. Сравните туннель с прямым соединением этого компьютера одним и тем же сервисом.</p>
        </div>
        <div className="speedControls">
          <div className="segmented" role="group" aria-label="Путь замера">
            <button className={!direct ? "on" : ""} disabled={live.running} onClick={() => { setMode("tunnel"); remember(MODE_KEY, "tunnel"); }}>Через туннель</button>
            <button className={direct ? "on" : ""} disabled={live.running} onClick={() => { setMode("direct"); remember(MODE_KEY, "direct"); }}>Без туннеля</button>
          </div>
          <div className="segmented" role="group" aria-label="Сервис замера">
            {providers.map((item) => (
              <button key={item.id} title={item.hint} className={provider === item.id ? "on" : ""} disabled={live.running} onClick={() => { setProvider(item.id); remember(PROVIDER_KEY, item.id); }}>{item.title}</button>
            ))}
          </div>
          {live.running && <button className="danger" onClick={onCancel}><CircleStop size={15} />Остановить</button>}
        </div>
      </header>

      <div className="panel speedHero">
        <div className="speedPath">
          <RouteRibbon nodes={nodes} state={live.running ? "busy" : pathDirect || connected ? "up" : "down"} />
          <div className="exitLine">
            {current?.exit_ip || current?.exit_country ? (
              <>
                <span>Выход: {exitCountry && <Flag code={exitCountry} label={countryName(exitCountry, "")} />} <b>{countryName(exitCountry, exitCountry || "—")}</b></span>
                {current?.exit_ip && (
                  <span className="exitIp">IP <code>{hide ? maskIp(current.exit_ip) : current.exit_ip}</code>
                    <button type="button" className="tinyButton" title={hide ? "Показать IP" : "Скрыть IP"} aria-label={hide ? "Показать IP" : "Скрыть IP"} onClick={() => setHide((value) => !value)}>{hide ? <Eye size={13} /> : <EyeOff size={13} />}</button>
                  </span>
                )}
                {current?.provider && <span>Сервис: {current.provider}</span>}
                {!live.running && current && <span className="muted">замер {when(current.started_at)}</span>}
              </>
            ) : <span className="muted">Адрес и страна выхода появятся при первом замере.</span>}
          </div>
        </div>
        <div className="gauges">
          <Gauge label="Скачивание" value={live.running ? live.down_mbps : best.down ?? 0} tone="rx" active={downloading} />
          <Gauge label="Отдача" value={live.running ? live.up_mbps : best.up ?? 0} tone="tx" active={uploading} />
        </div>
        <div className="pings">
          <PingMeter label="Пинг" value={live.running && live.ping_ms !== null && !shown?.ping_ms ? live.ping_ms : shown?.ping_ms} hint="Медиана задержки запроса по уже открытому соединению: от этого компьютера до сервиса и обратно." />
          <PingMeter label="Под нагрузкой" value={shown?.loaded_ping_ms} hint="Задержка, пока канал загружен потоками. Большая разница с обычным пингом значит, что очередь раздувается." />
          <PingMeter label="UDP · DNS" value={shown?.dns_ms} hint="Ответ DNS по UDP. Без туннеля его может перехватить другой VPN (Clash отвечает сам за доли миллисекунды)." />
          {shown?.server_rtt_ms ? <PingMeter label="M-Lab: от выхода" value={shown.server_rtt_ms} hint="Задержка, которую видит сервер M-Lab до своего собеседника. Через туннель это сервер-выход, а не этот компьютер." />
            : <PingMeter label="До сервера" value={serverPing} hint="Проверка связи SSH до вашего сервера, без интернета за ним." />}
          <dl className="pingFacts">
            <div><dt>Джиттер</dt><dd>{formatMs(shown?.jitter_ms)}</dd></div>
            <div><dt>Потери UDP</dt><dd>{shown?.dns_loss === null || shown?.dns_loss === undefined ? "—" : `${Math.round(shown.dns_loss * 100)} %`}</dd></div>
          </dl>
        </div>
        {(live.running || live.steps.length > 0) && (
          <div className="speedSteps">
            <ol>
              {live.steps.map((step, index) => (
                <li key={step} className={index < live.step || (!live.running && !live.error) ? "done" : index === live.step && live.running ? "current" : ""}>{step}</li>
              ))}
            </ol>
            <div className="speedProgress" aria-hidden="true"><i style={{ width: `${live.running ? ((live.step + live.progress) / Math.max(live.steps.length, 1)) * 100 : 100}%` }} /></div>
            <Timeline points={live.timeline} />
          </div>
        )}
        {live.error && <p className="notice bad">{live.error}</p>}
      </div>

      <div className="speedKinds">
        {kinds.map((kind) => (
          <article key={kind.id} className={live.running && live.kind === kind.id ? "speedKind running" : "speedKind"}>
            <h3>{kind.title}</h3>
            <p>{kind.text}</p>
            <footer>
              <span>{kind.time} · {kind.traffic}</span>
              <button className="primary" disabled={!canRun()} title={!direct && !connected ? "Подключите туннель или выберите «Без туннеля»" : undefined} onClick={() => onStart(kind.id, direct, provider)}><Play size={14} />{direct ? "Без туннеля" : "Запустить"}</button>
            </footer>
          </article>
        ))}
      </div>

      {shown && (
        <div className="metrics speedResults">
          <div className="metric"><span>Загрузка, 1 поток</span><strong className="rx">{formatMbps(shown.download_single_mbps)}</strong></div>
          <div className="metric"><span>Загрузка, много потоков</span><strong className="rx">{formatMbps(shown.download_mbps)}</strong></div>
          <div className="metric"><span>Отдача</span><strong className="tx">{formatMbps(shown.upload_mbps)}</strong></div>
          <div className="metric"><span>Короткие запросы</span><strong>{shown.requests_per_second ? `${shown.requests_per_second.toFixed(1)}/с · ${formatMs(shown.request_ms)}` : "—"}</strong></div>
          <div className="metric"><span>Смешанная: загрузка</span><strong className="rx">{formatMbps(shown.mixed_download_mbps)}</strong></div>
          <div className="metric"><span>Смешанная: запрос</span><strong>{formatMs(shown.mixed_request_ms)}</strong></div>
        </div>
      )}
      {shown?.download_source && <p className="fieldNote">Загрузка измерена с запасного источника — {shown.download_source}.</p>}

      <Comparison history={history} />

      <section className="panel">
        <header className="panelHead">
          <h3>История замеров</h3>
          {history.length > 0 && (confirming ? (
            <span className="confirmRow"><span>Удалить историю?</span>
              <button className="danger" onClick={() => { setConfirming(false); onClear(); }}><Trash2 size={14} />Удалить</button>
              <button className="ghost" onClick={() => setConfirming(false)}>Отмена</button></span>
          ) : <button className="ghost" onClick={() => setConfirming(true)}><Trash2 size={14} />Очистить</button>)}
        </header>
        {runs.length === 0 ? <p className="emptyHint">Замеров ещё не было. Выберите тест выше.</p> : (
          <div className="tableScroll">
            <table className="statsTable speedTable">
              <thead><tr><th>Когда</th><th>Тест</th><th>Путь</th><th>Выход</th><th>Сервис</th><th className="num">↓ Мбит/с</th><th className="num">↑ Мбит/с</th><th className="num">Пинг</th><th className="num">Под нагрузкой</th><th className="num">UDP</th><th className="num">Запросы</th></tr></thead>
              <tbody>
                {runs.map((run) => {
                  const down = downOf(run.results);
                  return (
                    <tr key={run.id} className={isDirect(run) ? "directRun" : undefined} title={[run.error, run.cancelled ? "Остановлен" : "", run.results.download_source ? `Загрузка: ${run.results.download_source}` : ""].filter(Boolean).join(" · ")}>
                      <td>{when(run.started_at)}</td>
                      <td>{kindTitle(run.kind)}{run.cancelled && <small> · прерван</small>}</td>
                      <td><PathCell run={run} /></td>
                      <td><ExitCell run={run} hide={hide} /></td>
                      <td className="serviceCell">{(run.provider || "Cloudflare").replace(/^LibreSpeed · /, "LibreSpeed: ").replace(/^M-Lab NDT7 · /, "M-Lab: ")}</td>
                      <td className="num rx"><span className="barCell"><i style={{ width: `${((down ?? 0) / top) * 100}%` }} />{formatMbps(down)}</span></td>
                      <td className="num tx">{formatMbps(run.results.upload_mbps)}</td>
                      <td className="num">{formatMs(run.results.ping_ms ?? run.results.server_rtt_ms)}</td>
                      <td className="num">{formatMs(run.results.loaded_ping_ms)}</td>
                      <td className="num">{formatMs(run.results.dns_ms)}</td>
                      <td className="num">{run.results.requests_per_second ? `${run.results.requests_per_second.toFixed(1)}/с` : "—"}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </section>
  );
}
