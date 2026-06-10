import React, { useEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CircleStop,
  Check,
  Copy,
  Eye,
  EyeOff,
  FileText,
  FolderOpen,
  HelpCircle,
  Play,
  Plus,
  RefreshCcw,
  Save,
  Server,
  Settings,
  Shield,
  SlidersHorizontal,
  Terminal,
  Trash2,
} from "lucide-react";
import appIconUrl from "./assets/app-icon.png";
import "./styles.css";

type ServerProfile = {
  id: string;
  name: string;
  host: string;
  port: number;
  user: string;
  key_path: string;
  compression: boolean;
  strict_host_key_checking: string;
  connect_timeout: number;
  server_alive_interval: number;
  server_alive_count_max: number;
};

type ProxyProfile = {
  id: string;
  name: string;
  host: string;
  port: number;
};

type AppSettings = {
  ipinfo_token: string;
  ssh_engine: string;
  hide_sensitive: boolean;
  verify_proxy_on_start: boolean;
  auto_reconnect: boolean;
  reconnect_delay_seconds: number;
  start_tunnel_on_launch: boolean;
  clear_logs_on_start: boolean;
};

type AppConfig = {
  selected_server_id: string;
  selected_proxy_id: string;
  servers: ServerProfile[];
  proxies: ProxyProfile[];
  settings: AppSettings;
};

type TunnelStatus = {
  state: string;
  state_emoji: string;
  app_version: string;
  ssh_engine: string;
  server_id: string;
  server_name: string;
  proxy_id: string;
  proxy_name: string;
  proxy_host: string;
  proxy_port: number;
  pid: number | null;
  started_at: number | null;
  ip: string;
  country_code: string;
  country_name: string;
  country_flag: string;
  rx_total: number;
  tx_total: number;
  last_error: string;
};

type TrafficPoint = {
  ts: number;
  rx_bps: number;
  tx_bps: number;
  rx_total: number;
  tx_total: number;
  latency_ms: number | null;
};

type Tab = "connect" | "servers" | "proxies" | "settings";
type BusyAction = "connect" | "disconnect" | "restart" | "save" | "delete" | "test" | "logs" | null;
type CopyField = "proxy-ip" | "server-host" | "ipinfo-token" | null;

const emptyServer: ServerProfile = {
  id: "",
  name: "",
  host: "",
  port: 22,
  user: "",
  key_path: "~/.ssh/id_ed25519",
  compression: false,
  strict_host_key_checking: "yes",
  connect_timeout: 8,
  server_alive_interval: 15,
  server_alive_count_max: 3,
};

const emptyProxy: ProxyProfile = {
  id: "",
  name: "",
  host: "127.0.0.1",
  port: 1080,
};

const emptyConfig: AppConfig = {
  selected_server_id: "",
  selected_proxy_id: "",
  servers: [],
  proxies: [],
  settings: {
    ipinfo_token: "",
    ssh_engine: "system",
    hide_sensitive: true,
    verify_proxy_on_start: true,
    auto_reconnect: false,
    reconnect_delay_seconds: 3,
    start_tunnel_on_launch: false,
    clear_logs_on_start: false,
  },
};

const defaultSettings: AppSettings = {
  ipinfo_token: "",
  ssh_engine: "system",
  hide_sensitive: true,
  verify_proxy_on_start: true,
  auto_reconnect: false,
  reconnect_delay_seconds: 3,
  start_tunnel_on_launch: false,
  clear_logs_on_start: false,
};

const emptyStatus: TunnelStatus = {
  state: "stopped",
  state_emoji: "🔴",
  app_version: "0.1.0",
  ssh_engine: "system",
  server_id: "",
  server_name: "",
  proxy_id: "",
  proxy_name: "",
  proxy_host: "127.0.0.1",
  proxy_port: 1080,
  pid: null,
  started_at: null,
  ip: "",
  country_code: "",
  country_name: "",
  country_flag: "",
  rx_total: 0,
  tx_total: 0,
  last_error: "",
};

function SettingHint({ text }: { text: string }) {
  return (
    <span className="settingHint" tabIndex={0} aria-label={text}>
      <HelpCircle size={15} />
      <span className="tooltip" role="tooltip">{text}</span>
    </span>
  );
}

function SettingLabel({ children, hint }: { children: React.ReactNode; hint: string }) {
  return (
    <span className="settingLabel">
      <span>{children}</span>
      <SettingHint text={hint} />
    </span>
  );
}

function makeId(name: string) {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function stateLabel(state: string) {
  if (state === "running") return "Работает";
  if (state === "unknown") return "Неизвестно";
  return "Остановлен";
}

function engineLabel(engine: string) {
  if (engine === "embedded") return "Embedded Rust SSH";
  return "System OpenSSH";
}

function selectedServer(config: AppConfig) {
  return (
    config.servers.find((server) => server.id === config.selected_server_id) ??
    config.servers[0] ??
    emptyServer
  );
}

function selectedProxy(config: AppConfig) {
  return (
    config.proxies.find((proxy) => proxy.id === config.selected_proxy_id) ??
    config.proxies[0] ??
    emptyProxy
  );
}

function formatBytes(value: number) {
  if (value >= 1024 * 1024 * 1024) return `${(value / 1024 / 1024 / 1024).toFixed(2)} GB`;
  if (value >= 1024 * 1024) return `${(value / 1024 / 1024).toFixed(2)} MB`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${value} B`;
}

function formatRate(value: number) {
  return `${formatBytes(value)}/s`;
}

function formatLatency(value: number | null | undefined) {
  if (value === null || value === undefined) return "-";
  return `${value} ms`;
}

function formatStartedAt(value: number | null) {
  if (!value) return "-";
  return new Date(value * 1000).toLocaleString();
}

function formatChartTime(value: number) {
  return new Date(value * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function maskValue(value: string) {
  if (!value) return "-";
  return "••••••••";
}

function TrafficChart({ points }: { points: TrafficPoint[] }) {
  const width = 720;
  const height = 200;
  const chart = { left: 44, right: 4, top: 22, bottom: 30 };
  const visible = points.slice(-60);
  const max = Math.max(1, ...visible.map((point) => Math.max(point.rx_bps, point.tx_bps)));
  const latencyMax = Math.max(1, ...visible.map((point) => point.latency_ms ?? 0));
  const latestLatency = [...visible].reverse().find((point) => point.latency_ms !== null && point.latency_ms !== undefined)?.latency_ms;
  const yTicks = [max, Math.round(max / 2), 0];
  const xTicks =
    visible.length > 2
      ? [visible[0], visible[Math.floor(visible.length / 2)], visible[visible.length - 1]]
      : visible;

  function xFor(index: number) {
    if (visible.length < 2) return chart.left;
    return chart.left + (index / (visible.length - 1)) * (width - chart.left - chart.right);
  }

  function yFor(value: number) {
    return chart.top + (1 - value / max) * (height - chart.top - chart.bottom);
  }

  function yForLatency(value: number | null | undefined) {
    return chart.top + (1 - (value ?? 0) / latencyMax) * (height - chart.top - chart.bottom);
  }

  function pathFor(key: "rx_bps" | "tx_bps") {
    if (visible.length < 2) return "";
    return visible
      .map((point, index) => {
        const x = xFor(index);
        const y = yFor(point[key]);
        return `${index === 0 ? "M" : "L"} ${x.toFixed(1)} ${y.toFixed(1)}`;
      })
      .join(" ");
  }

  function latencyPath() {
    if (visible.length < 2) return "";
    return visible
      .map((point, index) => {
        const x = xFor(index);
        const y = yForLatency(point.latency_ms);
        return `${index === 0 ? "M" : "L"} ${x.toFixed(1)} ${y.toFixed(1)}`;
      })
      .join(" ");
  }

  return (
    <div className="trafficChart">
      <svg viewBox={`0 0 ${width} ${height}`} aria-label="Traffic chart">
        <text className="axisTitle" x={chart.left} y={13}>Скорость</text>
        <g className="gridLines">
          {yTicks.map((tick, index) => {
            const y = yFor(tick);
            return (
              <g key={`${tick}-${index}`}>
                <line x1={chart.left} y1={y} x2={width - chart.right} y2={y} />
                <text x={chart.left - 10} y={y + 4} textAnchor="end">
                  {formatRate(tick)}
                </text>
              </g>
            );
          })}
          {xTicks.map((point) => {
            const index = visible.indexOf(point);
            const x = xFor(index);
            return (
              <g key={point.ts}>
                <line x1={x} y1={chart.top} x2={x} y2={height - chart.bottom} />
                <text x={x} y={height - 12} textAnchor={index === 0 ? "start" : index === visible.length - 1 ? "end" : "middle"}>
                  {formatChartTime(point.ts)}
                </text>
              </g>
            );
          })}
        </g>
        <line className="axisLine" x1={chart.left} y1={height - chart.bottom} x2={width - chart.right} y2={height - chart.bottom} />
        <path className="rx" d={pathFor("rx_bps")} />
        <path className="tx" d={pathFor("tx_bps")} />
        <path className="ping" d={latencyPath()} />
        {visible.length < 2 && (
          <text className="emptyChart" x={width / 2} y={height / 2} textAnchor="middle">
            Данные появятся после запуска туннеля
          </text>
        )}
      </svg>
      <div className="chartLegend">
        <span><i className="rxMark" /> Получено</span>
        <span><i className="txMark" /> Отправлено</span>
        <span><i className="pingMark" /> Ping {formatLatency(latestLatency)}</span>
      </div>
    </div>
  );
}

function App() {
  const [activeTab, setActiveTab] = useState<Tab>("connect");
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [status, setStatus] = useState<TunnelStatus>(emptyStatus);
  const [serverDraft, setServerDraft] = useState<ServerProfile>(emptyServer);
  const [proxyDraft, setProxyDraft] = useState<ProxyProfile>(emptyProxy);
  const [settingsDraft, setSettingsDraft] = useState<AppSettings>(emptyConfig.settings);
  const [traffic, setTraffic] = useState<TrafficPoint[]>([]);
  const [logs, setLogs] = useState<string[]>([]);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState<BusyAction>(null);
  const [showServerHost, setShowServerHost] = useState(false);
  const [showIpinfoToken, setShowIpinfoToken] = useState(false);
  const [copiedField, setCopiedField] = useState<CopyField>(null);
  const settingsDirtyRef = useRef(false);

  const currentServer = useMemo(() => selectedServer(config), [config]);
  const currentProxy = useMemo(() => selectedProxy(config), [config]);
  const connected = status.state === "running";
  const stateClass = connected ? "state good" : status.last_error ? "state bad" : "state idle";
  const showSensitive = !config.settings.hide_sensitive;
  const visibleProxyIp = showSensitive ? status.ip || "-" : maskValue(status.ip);

  async function refreshAll() {
    try {
      const [nextConfig, nextStatus, nextTraffic, nextLogs] = await Promise.all([
        invoke<AppConfig>("get_config"),
        invoke<TunnelStatus>("tunnel_status"),
        invoke<TrafficPoint[]>("traffic_history"),
        invoke<string[]>("get_logs"),
      ]);
      setConfig(nextConfig);
      setStatus(nextStatus);
      setTraffic(nextTraffic);
      setLogs(nextLogs);
      if (!settingsDirtyRef.current) setSettingsDraft(nextConfig.settings);
      if (!serverDraft.id) setServerDraft(selectedServer(nextConfig));
      if (!proxyDraft.id) setProxyDraft(selectedProxy(nextConfig));
    } catch (error) {
      setMessage(String(error));
    }
  }

  async function runAction(action: BusyAction, callback: () => Promise<string | void>) {
    setBusy(action);
    setMessage("");
    try {
      const output = await callback();
      if (output) setMessage(output);
      await refreshAll();
    } catch (error) {
      setMessage(String(error));
      await refreshAll();
    } finally {
      setBusy(null);
    }
  }

  function updateServer<K extends keyof ServerProfile>(key: K, value: ServerProfile[K]) {
    setServerDraft((current) => ({
      ...current,
      [key]: value,
      id: key === "name" && !current.id ? makeId(String(value)) : current.id,
    }));
  }

  function updateProxy<K extends keyof ProxyProfile>(key: K, value: ProxyProfile[K]) {
    setProxyDraft((current) => ({
      ...current,
      [key]: value,
      id: key === "name" && !current.id ? makeId(String(value)) : current.id,
    }));
  }

  function updateSettings<K extends keyof AppSettings>(key: K, value: AppSettings[K]) {
    settingsDirtyRef.current = true;
    setSettingsDraft((current) => ({ ...current, [key]: value }));
  }

  async function choosePrivateKey() {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        title: "Select SSH private key",
      });
      if (typeof selected === "string" && selected.trim()) {
        updateServer("key_path", selected);
      }
    } catch (error) {
      setMessage(String(error));
    }
  }

  async function setSensitiveVisible(visible: boolean) {
    await runAction("save", async () => {
      const nextSettings = { ...config.settings, hide_sensitive: !visible };
      const nextConfig = await invoke<AppConfig>("save_settings", { settings: nextSettings });
      setConfig(nextConfig);
      if (!settingsDirtyRef.current) setSettingsDraft(nextConfig.settings);
      return visible ? "Sensitive values visible" : "Sensitive values hidden";
    });
  }

  async function copyText(value: string, field: CopyField) {
    if (!value) return;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
      } else {
        const textarea = document.createElement("textarea");
        textarea.value = value;
        textarea.style.position = "fixed";
        textarea.style.opacity = "0";
        document.body.appendChild(textarea);
        textarea.select();
        document.execCommand("copy");
        document.body.removeChild(textarea);
      }
      setCopiedField(field);
      window.setTimeout(() => setCopiedField(null), 1200);
    } catch (error) {
      setMessage(String(error));
    }
  }

  useEffect(() => {
    refreshAll();
    const timer = window.setInterval(refreshAll, 2500);
    return () => window.clearInterval(timer);
  }, []);

  return (
    <main className="app">
      <aside className="sidebar">
        <div className="brand">
          <img src={appIconUrl} alt="" className="brandIcon" />
            <div>
              <h1>Secret Tunnel</h1>
              <p>SSH SOCKS proxy · v{status.app_version}</p>
            </div>
          </div>

        <section className="statusPanel">
          <div className={stateClass}>
            {status.state_emoji} {stateLabel(status.state)}
          </div>
          <div className="meta">
            <div className="metaHeader">
              <span>IP прокси</span>
              <div className="miniActions">
                <button
                  type="button"
                  className="miniIconButton"
                  title={showSensitive ? "Скрыть IP" : "Показать IP"}
                  aria-label={showSensitive ? "Скрыть IP" : "Показать IP"}
                  onClick={() => setSensitiveVisible(!showSensitive)}
                >
                  {showSensitive ? <EyeOff size={13} /> : <Eye size={13} />}
                </button>
                <button
                  type="button"
                  className="miniIconButton"
                  title="Скопировать IP"
                  aria-label="Скопировать IP"
                  disabled={!status.ip}
                  onClick={() => copyText(status.ip, "proxy-ip")}
                >
                  {copiedField === "proxy-ip" ? <Check size={13} /> : <Copy size={13} />}
                </button>
              </div>
            </div>
            <strong>{visibleProxyIp}</strong>
          </div>
          <div className="meta">
            <span>Страна</span>
            <strong>
              {status.country_flag || ""} {status.country_name || status.country_code || "-"}
            </strong>
          </div>
            <div className="meta">
              <span>Трафик</span>
              <strong>
                ↓ {formatBytes(status.rx_total)} · ↑ {formatBytes(status.tx_total)}
              </strong>
            </div>
            <div className="meta">
              <span>SSH engine</span>
              <strong>{engineLabel(status.ssh_engine)}</strong>
            </div>
          </section>

        <nav className="tabs">
          <button className={activeTab === "connect" ? "active" : ""} onClick={() => setActiveTab("connect")}>
            <Shield size={16} />
            Подключение
          </button>
          <button className={activeTab === "servers" ? "active" : ""} onClick={() => setActiveTab("servers")}>
            <Server size={16} />
            Серверы
          </button>
          <button className={activeTab === "proxies" ? "active" : ""} onClick={() => setActiveTab("proxies")}>
            <SlidersHorizontal size={16} />
            Прокси
          </button>
          <button className={activeTab === "settings" ? "active" : ""} onClick={() => setActiveTab("settings")}>
            <Settings size={16} />
            Настройки
          </button>
        </nav>
      </aside>

      <section className="workspace">
        <div className={activeTab === "connect" ? "workspaceContent noScroll" : "workspaceContent"}>
          {activeTab === "connect" && (
            <section className="panel connectPanel">
              <div className="connectHeader">
                <div>
                  <h2>Подключение</h2>
                  <p>
                    {currentServer.name || "Сервер не выбран"} → {currentProxy.name || "Прокси не выбран"}
                  </p>
                </div>
                <div className="actions">
                  <button
                    className="primary"
                    disabled={busy !== null || connected || !currentServer.id || !currentProxy.id}
                    onClick={() => runAction("connect", () => invoke<string>("start_tunnel"))}
                  >
                    <Play size={16} />
                    Запустить
                  </button>
                  <button className="danger" disabled={busy !== null || !connected} onClick={() => runAction("disconnect", () => invoke<string>("stop_tunnel"))}>
                    <CircleStop size={16} />
                    Остановить
                  </button>
                  <button className="restart" disabled={busy !== null || !currentServer.id || !currentProxy.id} onClick={() => runAction("restart", () => invoke<string>("restart_tunnel"))}>
                    <RefreshCcw size={16} />
                    Restart
                  </button>
                </div>
              </div>

              <div className="formGrid">
                <label>
                  Сервер
                  <select
                    value={config.selected_server_id}
                    disabled={connected}
                    onChange={(event) =>
                      runAction("save", async () => {
                        await invoke<AppConfig>("select_server", { id: event.target.value });
                        return "Server selected";
                      })
                    }
                  >
                    {config.servers.map((server) => (
                      <option key={server.id} value={server.id}>
                        {server.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  Локальный прокси
                  <select
                    value={config.selected_proxy_id}
                    disabled={connected}
                    onChange={(event) =>
                      runAction("save", async () => {
                        await invoke<AppConfig>("select_proxy", { id: event.target.value });
                        return "Proxy selected";
                      })
                    }
                  >
                    {config.proxies.map((proxy) => (
                      <option key={proxy.id} value={proxy.id}>
                        {proxy.name} · {proxy.host}:{proxy.port}
                      </option>
                    ))}
                  </select>
                </label>
              </div>

              <div className="statusGrid">
                <div>
                  <span>Статус</span>
                  <strong>{status.state_emoji} {stateLabel(status.state)}</strong>
                </div>
                <div>
                  <span>SSH engine</span>
                  <strong>{engineLabel(status.ssh_engine)}</strong>
                </div>
                <div>
                  <span>IP прокси</span>
                  <div className="statusSecretValue">
                    <strong>{visibleProxyIp}</strong>
                    <button
                      type="button"
                      className="miniIconButton light"
                      title={showSensitive ? "Скрыть IP" : "Показать IP"}
                      aria-label={showSensitive ? "Скрыть IP" : "Показать IP"}
                      onClick={() => setSensitiveVisible(!showSensitive)}
                    >
                      {showSensitive ? <EyeOff size={13} /> : <Eye size={13} />}
                    </button>
                    <button
                      type="button"
                      className="miniIconButton light"
                      title="Скопировать IP"
                      aria-label="Скопировать IP"
                      disabled={!status.ip}
                      onClick={() => copyText(status.ip, "proxy-ip")}
                    >
                      {copiedField === "proxy-ip" ? <Check size={13} /> : <Copy size={13} />}
                    </button>
                  </div>
                </div>
                <div>
                  <span>Страна</span>
                  <strong>{status.country_flag || ""} {status.country_name || status.country_code || "-"}</strong>
                </div>
                <div>
                  <span>Получено</span>
                  <strong>{formatBytes(status.rx_total)}</strong>
                </div>
                <div>
                  <span>Отправлено</span>
                  <strong>{formatBytes(status.tx_total)}</strong>
                </div>
                <div>
                  <span>Запущен</span>
                  <strong>{formatStartedAt(status.started_at)}</strong>
                </div>
              </div>

              <TrafficChart points={traffic} />
            </section>
          )}

          {activeTab === "servers" && (
            <div className="grid">
              <section className="panel">
                <div className="panelTitle">
                  <Server size={18} />
                  Серверы
                </div>
                <div className="serverList">
                  {config.servers.map((server) => (
                    <button
                      key={server.id}
                      className={server.id === config.selected_server_id ? "serverItem selected" : "serverItem"}
                      onClick={() => setServerDraft(server)}
                    >
                      <span>{server.name}</span>
                      <strong>{server.user}@{maskValue(server.host)}:{server.port}</strong>
                    </button>
                  ))}
                </div>
                <button onClick={() => setServerDraft(emptyServer)}>
                  <Plus size={16} />
                  Новый сервер
                </button>
              </section>

              <section className="panel">
                <div className="panelTitle">
                  <Save size={18} />
                  Профиль сервера
                </div>
                <div className="formGrid">
                  <label>
                    Алиас
                    <input value={serverDraft.name} onChange={(event) => updateServer("name", event.target.value)} placeholder="Production" />
                  </label>
                  <label>
                    ID
                    <input value={serverDraft.id} onChange={(event) => updateServer("id", event.target.value)} placeholder="production" />
                  </label>
                  <label>
                    Host
                    <div className="secretField">
                      <input
                        type={showServerHost ? "text" : "password"}
                        value={serverDraft.host}
                        onChange={(event) => updateServer("host", event.target.value)}
                        placeholder="example.com"
                      />
                      <button
                        type="button"
                        className="iconButton"
                        title={showServerHost ? "Скрыть IP" : "Показать IP"}
                        aria-label={showServerHost ? "Скрыть IP" : "Показать IP"}
                        onClick={() => setShowServerHost((value) => !value)}
                      >
                        {showServerHost ? <EyeOff size={16} /> : <Eye size={16} />}
                      </button>
                      <button
                        type="button"
                        className="iconButton"
                        title="Скопировать IP"
                        aria-label="Скопировать IP"
                        disabled={!serverDraft.host}
                        onClick={() => copyText(serverDraft.host, "server-host")}
                      >
                        {copiedField === "server-host" ? <Check size={16} /> : <Copy size={16} />}
                      </button>
                    </div>
                  </label>
                  <label>
                    SSH port
                    <input type="number" value={serverDraft.port} onChange={(event) => updateServer("port", Number(event.target.value))} />
                  </label>
                  <label>
                    User
                    <input value={serverDraft.user} onChange={(event) => updateServer("user", event.target.value)} placeholder="user" />
                  </label>
                  <label className="wide">
                    SSH key
                    <div className="fileField">
                      <input value={serverDraft.key_path} onChange={(event) => updateServer("key_path", event.target.value)} placeholder="~/.ssh/id_ed25519" />
                      <button
                        type="button"
                        className="iconButton"
                        title="Выбрать SSH private key"
                        aria-label="Выбрать SSH private key"
                        onClick={choosePrivateKey}
                      >
                        <FolderOpen size={16} />
                      </button>
                    </div>
                  </label>
                  <label>
                    StrictHostKeyChecking
                    <select value={serverDraft.strict_host_key_checking} onChange={(event) => updateServer("strict_host_key_checking", event.target.value)}>
                      <option value="yes">yes</option>
                      <option value="accept-new">accept-new</option>
                      <option value="no">no</option>
                    </select>
                  </label>
                  <label>
                    Connect timeout, sec
                    <input type="number" min={1} max={120} value={serverDraft.connect_timeout} onChange={(event) => updateServer("connect_timeout", Number(event.target.value))} />
                  </label>
                  <label>
                    ServerAliveInterval, sec
                    <input type="number" min={1} max={300} value={serverDraft.server_alive_interval} onChange={(event) => updateServer("server_alive_interval", Number(event.target.value))} />
                  </label>
                  <label>
                    ServerAliveCountMax
                    <input type="number" min={1} max={20} value={serverDraft.server_alive_count_max} onChange={(event) => updateServer("server_alive_count_max", Number(event.target.value))} />
                  </label>
                  <label className="checkboxLine wide">
                    <input type="checkbox" checked={serverDraft.compression} onChange={(event) => updateServer("compression", event.target.checked)} />
                    SSH compression
                  </label>
                </div>
                <div className="actions left">
                  <button className="primary" disabled={busy !== null} onClick={() => runAction("save", async () => {
                    await invoke<AppConfig>("save_server", { server: serverDraft });
                    await invoke<AppConfig>("select_server", { id: serverDraft.id });
                    return "Server saved";
                  })}>
                    <Save size={16} />
                    Сохранить
                  </button>
                  <button disabled={busy !== null || !serverDraft.id} onClick={() => runAction("test", () => invoke<string>("test_server", { server: serverDraft }))}>
                    <Terminal size={16} />
                    Проверить
                  </button>
                  <button disabled={busy !== null || !serverDraft.id || connected} onClick={() => runAction("delete", async () => {
                    await invoke<AppConfig>("delete_server", { id: serverDraft.id });
                    setServerDraft(emptyServer);
                    return "Server deleted";
                  })}>
                    <Trash2 size={16} />
                    Удалить
                  </button>
                </div>
              </section>
            </div>
          )}

          {activeTab === "proxies" && (
            <div className="grid">
              <section className="panel">
                <div className="panelTitle">
                  <SlidersHorizontal size={18} />
                  Локальные прокси
                </div>
                <div className="serverList">
                  {config.proxies.map((proxy) => (
                    <button
                      key={proxy.id}
                      className={proxy.id === config.selected_proxy_id ? "serverItem selected" : "serverItem"}
                      onClick={() => setProxyDraft(proxy)}
                    >
                      <span>{proxy.name}</span>
                      <strong>{proxy.host}:{proxy.port}</strong>
                    </button>
                  ))}
                </div>
                <button onClick={() => setProxyDraft(emptyProxy)}>
                  <Plus size={16} />
                  Новый прокси
                </button>
              </section>

              <section className="panel">
                <div className="panelTitle">
                  <Save size={18} />
                  Профиль прокси
                </div>
                <div className="formGrid">
                  <label>
                    Название
                    <input value={proxyDraft.name} onChange={(event) => updateProxy("name", event.target.value)} placeholder="Local 1080" />
                  </label>
                  <label>
                    ID
                    <input value={proxyDraft.id} onChange={(event) => updateProxy("id", event.target.value)} placeholder="local-1080" />
                  </label>
                  <label>
                    Bind address
                    <input value={proxyDraft.host} disabled={connected} onChange={(event) => updateProxy("host", event.target.value)} placeholder="127.0.0.1" />
                  </label>
                  <label>
                    Port
                    <input type="number" disabled={connected} value={proxyDraft.port} onChange={(event) => updateProxy("port", Number(event.target.value))} />
                  </label>
                </div>
                <div className="actions left">
                  <button className="primary" disabled={busy !== null || connected} onClick={() => runAction("save", async () => {
                    await invoke<AppConfig>("save_proxy", { proxy: proxyDraft });
                    await invoke<AppConfig>("select_proxy", { id: proxyDraft.id });
                    return "Proxy saved";
                  })}>
                    <Save size={16} />
                    Сохранить
                  </button>
                  <button disabled={busy !== null || !proxyDraft.id || connected} onClick={() => runAction("delete", async () => {
                    await invoke<AppConfig>("delete_proxy", { id: proxyDraft.id });
                    setProxyDraft(emptyProxy);
                    return "Proxy deleted";
                  })}>
                    <Trash2 size={16} />
                    Удалить
                  </button>
                </div>
              </section>
            </div>
          )}

          {activeTab === "settings" && (
            <section className="panel narrow">
              <div className="panelTitle">
                <Settings size={18} />
                Настройки IPinfo
              </div>
              <label>
                <SettingLabel hint="Выбирает механизм туннеля. System OpenSSH стабильнее и использует системный ssh. Embedded Rust SSH работает внутри приложения, подходит для Windows без ssh.exe, но пока experimental.">
                  SSH engine
                </SettingLabel>
                <select value={settingsDraft.ssh_engine} onChange={(event) => updateSettings("ssh_engine", event.target.value)}>
                  <option value="system">System OpenSSH</option>
                  <option value="embedded">Embedded Rust SSH (experimental)</option>
                </select>
              </label>
              {settingsDraft.ssh_engine === "embedded" && (
                <div className="notice">
                  Embedded Rust SSH runs the SOCKS tunnel inside the app. Build with --embedded and keep System OpenSSH as the stable fallback.
                </div>
              )}
              <label>
                <SettingLabel hint="Токен IPinfo Lite нужен только для определения публичного IP, страны и флага через запущенный SOCKS proxy. Можно оставить пустым, тогда IP/страна не будут подтягиваться.">
                  IPinfo token
                </SettingLabel>
                <div className="secretField">
                  <input
                    type={showIpinfoToken ? "text" : "password"}
                    value={settingsDraft.ipinfo_token}
                    onChange={(event) => {
                      updateSettings("ipinfo_token", event.target.value);
                    }}
                    placeholder="PASTE_YOUR_IPINFO_LITE_TOKEN_HERE"
                  />
                  <button
                    type="button"
                    className="iconButton"
                    title={showIpinfoToken ? "Скрыть токен" : "Показать токен"}
                    aria-label={showIpinfoToken ? "Скрыть токен" : "Показать токен"}
                    onClick={() => setShowIpinfoToken((value) => !value)}
                  >
                    {showIpinfoToken ? <EyeOff size={16} /> : <Eye size={16} />}
                  </button>
                  <button
                    type="button"
                    className="iconButton"
                    title="Скопировать токен"
                    aria-label="Скопировать токен"
                    disabled={!settingsDraft.ipinfo_token}
                    onClick={() => copyText(settingsDraft.ipinfo_token, "ipinfo-token")}
                  >
                    {copiedField === "ipinfo-token" ? <Check size={16} /> : <Copy size={16} />}
                  </button>
                </div>
              </label>
              <label className="checkboxLine">
                <input type="checkbox" checked={settingsDraft.hide_sensitive} onChange={(event) => updateSettings("hide_sensitive", event.target.checked)} />
                <SettingLabel hint="Скрывает IP адреса и токены в интерфейсе до нажатия на глаз. Рекомендуется включить, особенно для скриншотов и демонстраций.">
                  Скрывать IP и токены по умолчанию
                </SettingLabel>
              </label>
              <label className="checkboxLine">
                <input type="checkbox" checked={settingsDraft.verify_proxy_on_start} onChange={(event) => updateSettings("verify_proxy_on_start", event.target.checked)} />
                <SettingLabel hint="После запуска приложение делает тестовый запрос через SOCKS proxy. Рекомендуется включить: так сразу видно, что туннель реально пропускает внешний трафик.">
                  Проверять SOCKS-прокси после запуска
                </SettingLabel>
              </label>
              <label className="checkboxLine">
                <input type="checkbox" checked={settingsDraft.auto_reconnect} onChange={(event) => updateSettings("auto_reconnect", event.target.checked)} />
                <SettingLabel hint="Если SSH процесс завершился или туннель упал, приложение попробует запустить выбранный сервер и proxy снова. Удобно для постоянного подключения.">
                  Автоматически переподключать туннель
                </SettingLabel>
              </label>
              <label className="checkboxLine">
                <input type="checkbox" checked={settingsDraft.start_tunnel_on_launch} onChange={(event) => updateSettings("start_tunnel_on_launch", event.target.checked)} />
                <SettingLabel hint="При открытии приложения автоматически запускает выбранные профили сервера и локального proxy. Лучше включать только после проверки, что выбранный туннель стабильно работает.">
                  Запускать выбранный туннель при открытии приложения
                </SettingLabel>
              </label>
              <label className="checkboxLine">
                <input type="checkbox" checked={settingsDraft.clear_logs_on_start} onChange={(event) => updateSettings("clear_logs_on_start", event.target.checked)} />
                <SettingLabel hint="Очищает Runtime logs перед новым запуском туннеля. Удобно для чистой диагностики; выключите, если хотите сохранять историю прошлых запусков.">
                  Очищать логи при запуске туннеля
                </SettingLabel>
              </label>
              <label>
                <SettingLabel hint="Пауза перед автоматическим переподключением. Обычно 3-5 секунд достаточно; увеличьте значение, если сервер временно недоступен или есть rate limit.">
                  Задержка переподключения, sec
                </SettingLabel>
                <input
                  type="number"
                  min={1}
                  max={120}
                  value={settingsDraft.reconnect_delay_seconds}
                  onChange={(event) => updateSettings("reconnect_delay_seconds", Number(event.target.value))}
                />
              </label>
              <div className="actions left">
                <button className="primary" disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const nextConfig = await invoke<AppConfig>("save_settings", { settings: settingsDraft });
                  settingsDirtyRef.current = false;
                  setConfig(nextConfig);
                  setSettingsDraft(nextConfig.settings);
                  return "Settings saved";
                })}>
                  <Save size={16} />
                  Сохранить настройки
                </button>
                <button disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const nextConfig = await invoke<AppConfig>("save_settings", { settings: defaultSettings });
                  settingsDirtyRef.current = false;
                  setConfig(nextConfig);
                  setSettingsDraft(nextConfig.settings);
                  return "Settings reset to defaults";
                })}>
                  <RefreshCcw size={16} />
                  По умолчанию
                </button>
              </div>
            </section>
          )}
        </div>

        <section className="bottomLogs">
          <div className="bottomLogsHeader">
            <div className="panelTitle">
              <FileText size={18} />
              Runtime logs
            </div>
            <button disabled={busy !== null} onClick={() => runAction("logs", async () => {
              await invoke<void>("clear_logs");
              return "Logs cleared";
            })}>
              <Trash2 size={16} />
              Очистить
            </button>
          </div>
          <pre className="logs">{logs.length ? logs.join("\n") : "No logs yet"}</pre>
        </section>
      </section>
    </main>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
