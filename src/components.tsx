import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Braces, ChevronDown, ChevronUp, ClipboardCopy, Eye, EyeOff, Search, Trash2 } from "lucide-react";
import flagFontUrl from "./assets/fonts/TwemojiCountryFlags.woff2?url";

// Windows has no flag emoji; this font draws them on every platform. It only
// covers regional-indicator letters, so other text keeps the system font.
const flagFace = document.createElement("style");
flagFace.textContent = `@font-face{font-family:"Twemoji Country Flags";unicode-range:U+1F1E6-1F1FF,U+1F3F4,U+E0062-E007F;src:url("${flagFontUrl}") format("woff2");font-display:swap}`;
document.head.appendChild(flagFace);

export function flagEmoji(code: string) {
  const upper = code.trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(upper)) return "";
  return String.fromCodePoint(...[...upper].map((letter) => 0x1f1e6 + letter.charCodeAt(0) - 65));
}

const regionNames = (() => {
  try {
    return new Intl.DisplayNames(["ru"], { type: "region" });
  } catch {
    return null;
  }
})();

export function countryName(code: string, fallback: string) {
  const upper = code.trim().toUpperCase();
  if (/^[A-Z]{2}$/.test(upper)) {
    try {
      return regionNames?.of(upper) || fallback || upper;
    } catch {
      return fallback || upper;
    }
  }
  return fallback;
}

export function Flag({ code, label }: { code: string; label?: string }) {
  const emoji = flagEmoji(code);
  if (!emoji) return null;
  return <span className="flag" role="img" aria-label={label || code.toUpperCase()}>{emoji}</span>;
}

export type RouteNode = { label: string; kind: "device" | "hop" | "exit" | "internet"; flag?: string };

/** The path traffic takes: this computer, SSH hops, the exit server, the internet. */
export function RouteRibbon({ nodes, state }: { nodes: RouteNode[]; state: "up" | "busy" | "down" | "error" }) {
  return (
    <ol className={`route route-${state}`} aria-label={`Маршрут: ${nodes.map((node) => node.label).join(", ")}`}>
      {nodes.map((node, index) => (
        <li key={`${node.label}-${index}`} className={`routeNode routeNode-${node.kind}`}>
          {index > 0 && <span className="routeLink" aria-hidden="true"><i /></span>}
          <span className="routeStop">
            <span className="routeDot" aria-hidden="true" />
            <span className="routeLabel">{node.label}</span>
            {node.flag && <Flag code={node.flag} />}
          </span>
        </li>
      ))}
    </ol>
  );
}

export type TrafficPoint = {
  ts: number;
  rx_bps: number;
  tx_bps: number;
  rx_total: number;
  tx_total: number;
  latency_ms: number | null;
};

export function formatBitRate(bytesPerSecond: number) {
  const bits = bytesPerSecond * 8;
  if (bits >= 1e9) return `${(bits / 1e9).toFixed(2)} Гбит/с`;
  if (bits >= 1e6) return `${(bits / 1e6).toFixed(bits >= 1e7 ? 0 : 1)} Мбит/с`;
  if (bits >= 1e3) return `${(bits / 1e3).toFixed(0)} Кбит/с`;
  return `${Math.round(bits)} бит/с`;
}

export function formatBytes(value: number) {
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(2)} ГБ`;
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} МБ`;
  if (value >= 1024) return `${(value / 1024).toFixed(0)} КБ`;
  return `${value} Б`;
}

function niceCeiling(value: number) {
  const exponent = 10 ** Math.floor(Math.log10(value));
  const fraction = value / exponent;
  const nice = fraction <= 1 ? 1 : fraction <= 2 ? 2 : fraction <= 5 ? 5 : 10;
  return nice * exponent;
}

function clock(ts: number) {
  return new Date(ts * 1000).toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

/** Download, upload and ping over the last minutes; fills its container. */
export function TrafficChart({ points }: { points: TrafficPoint[] }) {
  const boxRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 720, height: 220 });
  const [hover, setHover] = useState<number | null>(null);
  useLayoutEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setSize({ width: Math.max(320, Math.round(width)), height: Math.max(140, Math.round(height)) });
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, []);

  const visible = points.slice(-120);
  const { width, height } = size;
  const pad = { left: 82, right: 52, top: 12, bottom: 24 };
  const plotW = width - pad.left - pad.right;
  const plotH = height - pad.top - pad.bottom;
  const peakRate = Math.max(1, ...visible.map((p) => Math.max(p.rx_bps, p.tx_bps)));
  const rateMax = niceCeiling(peakRate * 8) / 8;
  const latencies = visible.map((p) => p.latency_ms ?? 0);
  const latencyMax = niceCeiling(Math.max(10, ...latencies));
  const x = (index: number) => pad.left + (visible.length < 2 ? 0 : (index / (visible.length - 1)) * plotW);
  const yRate = (value: number) => pad.top + (1 - value / rateMax) * plotH;
  const yPing = (value: number) => pad.top + (1 - value / latencyMax) * plotH;
  const line = (values: number[], y: (value: number) => number) =>
    values.map((value, index) => `${index ? "L" : "M"}${x(index).toFixed(1)},${y(value).toFixed(1)}`).join(" ");
  const area = (values: number[]) =>
    visible.length < 2 ? "" : `${line(values, yRate)} L${x(values.length - 1).toFixed(1)},${pad.top + plotH} L${pad.left},${pad.top + plotH} Z`;
  const rx = visible.map((p) => p.rx_bps);
  const tx = visible.map((p) => p.tx_bps);
  const ticks = [1, 0.5, 0];
  const timeTicks = visible.length > 2 ? [0, Math.floor((visible.length - 1) / 2), visible.length - 1] : [];
  const hovered = hover !== null ? visible[hover] : undefined;

  function onMove(event: React.MouseEvent<SVGSVGElement>) {
    if (visible.length < 2) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const position = ((event.clientX - rect.left) / rect.width) * width;
    const index = Math.round(((position - pad.left) / plotW) * (visible.length - 1));
    setHover(index >= 0 && index < visible.length ? index : null);
  }

  return (
    <div className="chart">
      <div className="chartHead">
        <span className="chartTitle">Скорость за последние минуты</span>
        <span className="chartLegend">
          <span><i className="swatch rx" />Скачивание</span>
          <span><i className="swatch tx" />Отдача</span>
          <span><i className="swatch ping" />Пинг</span>
        </span>
      </div>
      <div className="chartBox" ref={boxRef}>
        <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} onMouseMove={onMove} onMouseLeave={() => setHover(null)} role="img"
          aria-label="График скорости скачивания, отдачи и пинга">
          {ticks.map((fraction) => (
            <g key={fraction} className="grid">
              <line x1={pad.left} x2={width - pad.right} y1={yRate(rateMax * fraction)} y2={yRate(rateMax * fraction)} />
              <text x={pad.left - 8} y={yRate(rateMax * fraction) + 4} textAnchor="end">{formatBitRate(rateMax * fraction)}</text>
              <text className="pingAxis" x={width - pad.right + 8} y={yRate(rateMax * fraction) + 4}>{Math.round(latencyMax * fraction)} мс</text>
            </g>
          ))}
          {timeTicks.map((index, order) => (
            <text key={index} className="time" x={x(index)} y={height - 6} textAnchor={order === 0 ? "start" : order === 2 ? "end" : "middle"}>
              {clock(visible[index].ts)}
            </text>
          ))}
          {visible.length >= 2 ? (
            <>
              <path className="area rx" d={area(rx)} />
              <path className="area tx" d={area(tx)} />
              <path className="series rx" d={line(rx, yRate)} />
              <path className="series tx" d={line(tx, yRate)} />
              <path className="series ping" d={line(latencies, yPing)} />
            </>
          ) : (
            <text className="empty" x={width / 2} y={height / 2} textAnchor="middle">График появится после запуска туннеля</text>
          )}
          {hovered && hover !== null && (
            <g className="cursor">
              <line x1={x(hover)} x2={x(hover)} y1={pad.top} y2={pad.top + plotH} />
              <circle className="rx" cx={x(hover)} cy={yRate(hovered.rx_bps)} r={3.5} />
              <circle className="tx" cx={x(hover)} cy={yRate(hovered.tx_bps)} r={3.5} />
            </g>
          )}
        </svg>
        {hovered && hover !== null && (
          <div className="chartTip" style={{ left: Math.min(Math.max(x(hover) + 12, 8), width - 190) }}>
            <strong>{clock(hovered.ts)}</strong>
            <span><i className="swatch rx" />{formatBitRate(hovered.rx_bps)}</span>
            <span><i className="swatch tx" />{formatBitRate(hovered.tx_bps)}</span>
            <span><i className="swatch ping" />{hovered.latency_ms ?? "—"} мс</span>
          </div>
        )}
      </div>
    </div>
  );
}

type LogKind = "event" | "connection" | "error" | "service";
type Connection = { route: "tunnel" | "direct" | "block"; who: string; host: string; address: string; port: string; protocol: string };
type LogLine = { time: string; text: string; details: string; level: "error" | "warn" | "info"; kind: LogKind; connection?: Connection };

// Judged on the message only, never on JSON diagnostics: "timeout_ms" or
// "error_code": null are field names, not failures.
const errorPattern = /\b(error|errors|failed|failure|rejected|denied|refused|timed out|unreachable)\b|ошибк|не удалось|не найден|AGENT_[A-Z_]+|broken pipe|reset by peer|panicked/i;
const warnPattern = /\b(warn|warning|reconnect|reconnecting|retry|retrying)\b|переподключ|повтор/i;
/** Periodic and per-connection internals: useful for diagnosis, noise otherwise. */
const servicePattern = /^(SSH heartbeat|SSH TCP|SOCKS: local fake-IP|SOCKS destination|Transport v3 ready|Агент |Android: |Traffic sampler|Redirect macOS: (ProxyBridge CLI|Extension:|DNS proxy|Network configuration|Stopping))/;
const connectionPattern = /^Соединение (через туннель|напрямую|заблокировано): (.+?) → (.+?)(?::(\d+))? (TCP|UDP)$/;
/** Engine lines said in plain words. */
const plainWords: [RegExp, string | ((m: RegExpExecArray) => string)][] = [
  [/^Redirect macOS: Redirecting traffic\..*$/, "Перехват включён: правила применены"],
  [/^Redirect macOS: Rules cleared, tunnel stopped\.$/, "Перехват выключен: правила сняты"],
  [/^Redirect macOS: Profile ".*": \d+ proxies, (\d+) rules applied$/, (m) => `Перехват: применено правил — ${m[1]}`],
  [/^Tunnel is running, ssh pid=.*$/, "Туннель запущен"],
  [/^Tunnel stopped$/, "Туннель остановлен"],
  [/^Proxy verification OK, public IP hidden$/, "Прокси проверен: выход работает"],
  [/^TCP reachability OK$/, "Сервер доступен по TCP"],
  [/^SSH key authentication OK$/, "Ключ SSH принят сервером"],
  [/^Testing SSH key authentication\.\.\.$/, "Проверяем ключ SSH…"],
];

const ipv4 = /\b\d{1,3}(?:\.\d{1,3}){3}\b/g;
const ipv6 = /\b[0-9a-f]{1,4}(?::[0-9a-f]{0,4}){2,7}\b/gi;
/** IP addresses replaced, for screenshots and shared logs. Clock times are kept. */
export function maskAddresses(text: string) {
  return text
    .replace(ipv4, "•••.•••.•••.•••")
    .replace(ipv6, (match) => (match.includes("::") || match.split(":").length >= 5 ? "••••::••••" : match));
}

function parseConnection(text: string): Connection | undefined {
  const match = connectionPattern.exec(text);
  if (!match) return undefined;
  const route = match[1] === "через туннель" ? "tunnel" : match[1] === "напрямую" ? "direct" : "block";
  const named = /^(.+) \(([^()]+)\)$/.exec(match[3]);
  return { route, who: match[2], host: named ? named[1] : "", address: named ? named[2] : match[3], port: match[4] || "", protocol: match[5] };
}

/** `[epoch] message` lines from the backend. A trailing JSON object, or
 * further lines, are details: the periodic transport diagnostics. */
export function parseLogs(raw: string[]): LogLine[] {
  return raw.map((entry) => {
    const [first, ...rest] = entry.split("\n");
    const match = /^\[(\d{9,12})\]\s?(.*)$/.exec(first);
    const time = match ? clock(Number(match[1])) : "";
    let text = match ? match[2] : first;
    let details = rest.join("\n");
    const json = /^(.*?)\s+([[{].*[\]}])$/.exec(text);
    if (json && json[1]) {
      text = json[1];
      details = details ? `${json[2]}\n${details}` : json[2];
    }
    const connection = parseConnection(text);
    if (connection) {
      return { time, text, details, kind: "connection", level: connection.route === "block" ? "warn" : "info", connection };
    }
    for (const [pattern, words] of plainWords) {
      const plain = pattern.exec(text);
      if (plain) {
        text = typeof words === "string" ? words : words(plain);
        break;
      }
    }
    const level = errorPattern.test(text) ? "error" : warnPattern.test(text) ? "warn" : "info";
    const kind: LogKind = level === "error" ? "error" : servicePattern.test(text) ? "service" : "event";
    return { time, text, details, level, kind };
  });
}

const LOG_HEIGHT_KEY = "secret-tunnel.log-height";
const LOG_KINDS_KEY = "secret-tunnel.log-kinds";
const kindLabels: [LogKind, string, string][] = [
  ["event", "События", "Подключение, перехват, переподключения"],
  ["connection", "Соединения", "Какая программа куда подключилась и каким путём"],
  ["error", "Ошибки", "Сбои и отказы"],
  ["service", "Служебные", "Проверки связи, статистика TCP и другие внутренние записи"],
];
const routeLabels = { tunnel: "туннель", direct: "напрямую", block: "блок" } as const;

function readKinds(): Set<LogKind> {
  try {
    const saved = JSON.parse(localStorage.getItem(LOG_KINDS_KEY) || "null");
    if (Array.isArray(saved)) return new Set(saved);
  } catch {
    /* per-viewer convenience only */
  }
  return new Set<LogKind>(["event", "connection", "error"]);
}

function ConnectionText({ connection, hide }: { connection: Connection; hide: boolean }) {
  const dns = connection.port === "53";
  const address = hide ? maskAddresses(connection.address) : connection.address;
  return (
    <span className="flow">
      <em className={`via ${connection.route}`}>{routeLabels[connection.route]}</em>
      <strong>{connection.who}</strong>
      <span className="arrow">→</span>
      {dns ? <span>DNS-запрос{connection.host ? ` ${connection.host}` : ""}</span>
        : connection.host ? <span>{connection.host}{!hide && <small> {address}</small>}</span>
        : <span>{address}</span>}
      {!dns && connection.port && <small>:{connection.port}</small>}
      <small className="proto">{connection.protocol}</small>
    </span>
  );
}

/** What a line says, as shown: addresses masked when asked. */
function lineText(line: LogLine, hide: boolean) {
  if (line.connection) {
    const c = line.connection;
    const target = c.port === "53" ? "DNS-запрос" : c.host || (hide ? maskAddresses(c.address) : c.address);
    return `${routeLabels[c.route]}: ${c.who} → ${target}${c.port && c.port !== "53" ? `:${c.port}` : ""} ${c.protocol}`;
  }
  return hide ? maskAddresses(line.text) : line.text;
}

export function LogDock({ logs, busy, hideAddresses, onClear, onCopied }: { logs: string[]; busy: boolean; hideAddresses: boolean; onClear: () => void; onCopied: (text: string) => void }) {
  const [height, setHeight] = useState(() => {
    try {
      return Number(localStorage.getItem(LOG_HEIGHT_KEY)) || 220;
    } catch {
      return 220;
    }
  });
  const [collapsed, setCollapsed] = useState(false);
  const [filter, setFilter] = useState("");
  const [kinds, setKinds] = useState<Set<LogKind>>(readKinds);
  const [showDetails, setShowDetails] = useState(false);
  const [hide, setHide] = useState(hideAddresses);
  const [follow, setFollow] = useState(true);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => setHide(hideAddresses), [hideAddresses]);
  const lines = useMemo(() => parseLogs(logs), [logs]);
  const counts = useMemo(() => {
    const result: Record<LogKind, number> = { event: 0, connection: 0, error: 0, service: 0 };
    lines.forEach((line) => { result[line.kind] += 1; });
    return result;
  }, [lines]);
  // Repeats in a row collapse into one entry with a count.
  const shown = useMemo(() => {
    const needle = filter.toLowerCase();
    const result: { line: LogLine; text: string; repeat: number }[] = [];
    for (const line of lines) {
      if (!kinds.has(line.kind)) continue;
      const text = lineText(line, hide);
      if (needle && !`${text}\n${line.details}`.toLowerCase().includes(needle)) continue;
      const last = result[result.length - 1];
      if (last && last.text === text && last.line.kind === line.kind) {
        last.repeat += 1;
        last.line = line;
      } else {
        result.push({ line, text, repeat: 1 });
      }
    }
    return result;
  }, [lines, kinds, filter, hide]);

  useEffect(() => {
    try {
      localStorage.setItem(LOG_HEIGHT_KEY, String(Math.round(height)));
    } catch {
      /* per-viewer convenience only */
    }
  }, [height]);
  useEffect(() => {
    try {
      localStorage.setItem(LOG_KINDS_KEY, JSON.stringify([...kinds]));
    } catch {
      /* per-viewer convenience only */
    }
  }, [kinds]);
  useEffect(() => {
    const list = listRef.current;
    if (list && follow) list.scrollTop = list.scrollHeight;
  }, [shown.length, follow, collapsed]);

  function toggleKind(kind: LogKind) {
    setKinds((current) => {
      const next = new Set(current);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      return next;
    });
  }

  function startResize(event: React.PointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const startY = event.clientY;
    const startHeight = height;
    const move = (moveEvent: PointerEvent) => {
      const next = startHeight + (startY - moveEvent.clientY);
      setHeight(Math.min(Math.max(next, 120), Math.round(window.innerHeight * 0.75)));
      setCollapsed(false);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function onScroll() {
    const list = listRef.current;
    if (!list) return;
    setFollow(list.scrollHeight - list.scrollTop - list.clientHeight < 24);
  }

  return (
    <section className={collapsed ? "logDock collapsed" : "logDock"} style={collapsed ? undefined : { height }}>
      <div className="logResize" onPointerDown={startResize} role="separator" aria-orientation="horizontal"
        aria-label="Изменить высоту журнала" title="Потяните, чтобы изменить высоту" />
      <div className="logBar">
        <button className="logTitle" onClick={() => setCollapsed((value) => !value)} aria-expanded={!collapsed}>
          {collapsed ? <ChevronUp size={15} /> : <ChevronDown size={15} />}
          Журнал
          {counts.error > 0 && <span className="count bad">{counts.error} ошиб.</span>}
        </button>
        {!collapsed && (
          <>
            <div className="logKinds" role="group" aria-label="Что показывать">
              {kindLabels.map(([kind, label, hint]) => (
                <button key={kind} className={kinds.has(kind) ? `kind ${kind} on` : `kind ${kind}`} aria-pressed={kinds.has(kind)} title={hint} onClick={() => toggleKind(kind)}>
                  {label}<span>{counts[kind]}</span>
                </button>
              ))}
            </div>
            <label className="logSearch">
              <Search size={14} />
              <input value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Поиск: программа, сайт, текст" aria-label="Поиск в журнале" />
            </label>
            <button className={hide ? "ghost icon on" : "ghost icon"} aria-pressed={hide} title={hide ? "Показать IP-адреса" : "Скрыть IP-адреса"} aria-label={hide ? "Показать IP-адреса" : "Скрыть IP-адреса"} onClick={() => setHide((value) => !value)}>
              {hide ? <EyeOff size={15} /> : <Eye size={15} />}
            </button>
            <button className={showDetails ? "ghost icon on" : "ghost icon"} aria-pressed={showDetails} title={showDetails ? "Скрыть подробности" : "Показать подробности (JSON диагностики)"} aria-label="Подробности" onClick={() => setShowDetails((value) => !value)}>
              <Braces size={15} />
            </button>
            <button className="ghost icon" title="Скопировать показанные записи" aria-label="Скопировать журнал" onClick={() => onCopied(shown.map(({ line, text, repeat }) => `${line.time} ${text}${repeat > 1 ? ` ×${repeat}` : ""}${showDetails && line.details ? `\n${hide ? maskAddresses(line.details) : line.details}` : ""}`).join("\n"))}>
              <ClipboardCopy size={15} />
            </button>
            <button className="ghost icon" title="Очистить журнал" aria-label="Очистить журнал" disabled={busy} onClick={onClear}><Trash2 size={15} /></button>
          </>
        )}
      </div>
      {!collapsed && (
        <div className="logList" ref={listRef} onScroll={onScroll} role="log" aria-live="off">
          {shown.length === 0 && <p className="logEmpty">{lines.length ? "Нет записей с такими фильтрами" : "Журнал пуст. Записи появятся после запуска туннеля."}</p>}
          {shown.map(({ line, text, repeat }, index) => (
            <div key={index} className={`logLine ${line.level} ${line.kind}`}>
              <time>{line.time}</time>
              <div className="logText">
                {line.connection ? <ConnectionText connection={line.connection} hide={hide} /> : <span>{text}</span>}
                {repeat > 1 && <b className="repeat" title="Повторилось подряд">×{repeat}</b>}
                {showDetails && line.details && <code>{hide ? maskAddresses(line.details) : line.details}</code>}
              </div>
            </div>
          ))}
        </div>
      )}
      {!collapsed && !follow && (
        <button className="logFollow" onClick={() => { setFollow(true); const list = listRef.current; if (list) list.scrollTop = list.scrollHeight; }}>
          К последним записям
        </button>
      )}
    </section>
  );
}
