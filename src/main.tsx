import DomainsPanel from "./DomainsPanel";
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
  jump_server_id: string | null;
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

type SshTuning = {
  cipher: "auto" | "aes128_gcm" | "chacha20_poly1305";
  channel_buffer_messages: number;
  channel_receive_window_bytes: number;
};
const defaultSshTuning: SshTuning = {cipher:"auto",channel_buffer_messages:32,channel_receive_window_bytes:2097152};

type AppSettings = {
  redirect_cli_path: string;
  redirect_processes: string[];
  domain_rules: DomainRule[];
  ipinfo_token: string;
  ssh_engine: string;
  ssh_tuning?: SshTuning | null;
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
  ssh_route: string[];
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

type Tab = "connect" | "servers" | "proxies" | "applications" | "domains" | "settings";
type DomainRule = { id: string; pattern: string; enabled: boolean; action: "tunnel" | "direct" };
type DomainStats = { events: number; unknown: number; rules: {id: string; observed: number; tunnel: number; direct: number}[] };
type RedirectStatus = { running: boolean; starting: boolean; platform: string; requires_administrator: boolean; supports_system_ssh: boolean; supports_domains: boolean; setup_message: string; pid: number | null; administrator: boolean; component_available: boolean; last_error: string; domain_stats?: DomainStats };
type InstalledApp = { id: string; name: string; executable: string; patterns: string[]; source: string };
const normalizeAppPattern = (value: string) => value.trim().replace(/\//g, "\\").toLowerCase();
const appAliases = (app: InstalledApp) => (app.source === "macos" ? app.patterns : [...app.patterns, app.executable.split(/[\\/]/).pop() || ""]).map(normalizeAppPattern);
type BusyAction = "connect" | "disconnect" | "restart" | "save" | "delete" | "test" | "logs" | null;
type CopyField = "proxy-ip" | "server-host" | "ipinfo-token" | null;

const emptyServer: ServerProfile = {
  id: "",
  name: "",
  host: "",
  port: 22,
  user: "",
  key_path: "~/.ssh/id_ed25519",
  jump_server_id: null,
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
    redirect_cli_path: "",
    redirect_processes: ["Discord.exe"],
    domain_rules: [],
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

const emptyStatus: TunnelStatus = {
  state: "stopped",
  state_emoji: "🔴",
  app_version: "0.1.0",
  ssh_engine: "system",
  server_id: "",
  server_name: "",
  ssh_route: [],
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
  if (state === "starting") return "Подключение";
  if (state === "reconnecting") return "Переподключение";
  if (state === "stopping") return "Остановка";
  if (state === "failed") return "Ошибка подключения";
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

function serverRouteLabel(server: ServerProfile, servers: ServerProfile[]) {
  const route = [server.name];
  const seen = new Set([server.id]);
  let next = server.jump_server_id;
  while (next) {
    const hop = servers.find((item) => item.id === next);
    if (!hop || seen.has(next) || route.length >= 4) return "Проверьте цепочку SSH";
    seen.add(next);
    route.unshift(hop.name);
    next = hop.jump_server_id;
  }
  return route.join(" → ");
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
  const [redirect, setRedirect] = useState<RedirectStatus>({ running: false, starting: false, platform: "unknown", requires_administrator: true, supports_system_ssh: false, supports_domains: false, setup_message: "", pid: null, administrator: false, component_available: false, last_error: "" });
  const [installedApps, setInstalledApps] = useState<InstalledApp[]>([]);
  const [appsLoaded, setAppsLoaded] = useState(false);
  const [appsScanning, setAppsScanning] = useState(false);
  const [appsError, setAppsError] = useState("");
  const [appSearch, setAppSearch] = useState("");
  const [selectedAppsOnly, setSelectedAppsOnly] = useState(false);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState<BusyAction>(null);
  const [showServerHost, setShowServerHost] = useState(false);
  const [showIpinfoToken, setShowIpinfoToken] = useState(false);
  const [copiedField, setCopiedField] = useState<CopyField>(null);
  const serverDraftDirtyRef = useRef(false);
  const proxyDraftDirtyRef = useRef(false);
  const settingsDirtyRef = useRef(false);
  const refreshInFlightRef = useRef(false);

  const currentServer = useMemo(() => selectedServer(config), [config]);
  const currentProxy = useMemo(() => selectedProxy(config), [config]);
  const connected = status.state === "running";
  const sessionActive = ["running", "starting", "reconnecting", "stopping"].includes(status.state);
  const isMac = redirect.platform === "macos";
  const redirectActive = redirect.running || redirect.starting;
  const redirectSupported = isMac || redirect.platform === "windows";
  const canRedirect = redirectSupported && connected && !redirectActive && redirect.component_available &&
    (!redirect.requires_administrator || redirect.administrator) &&
    (redirect.supports_system_ssh || status.ssh_engine === "embedded");
  const stateClass = connected ? "state good" : status.last_error ? "state bad" : "state idle";
  const showSensitive = !config.settings.hide_sensitive;
  const visibleProxyIp = showSensitive ? status.ip || "-" : maskValue(status.ip);
  const selectedPatterns = new Set(settingsDraft.redirect_processes.map(normalizeAppPattern));
  const isAppSelected = (app: InstalledApp) => appAliases(app).some(pattern => selectedPatterns.has(pattern));
  const appChoices = useMemo(() => {
    const known = new Set(installedApps.flatMap(appAliases));
    const custom = settingsDraft.redirect_processes.filter(pattern => pattern && !known.has(normalizeAppPattern(pattern)))
      .map(pattern => ({ id: normalizeAppPattern(pattern), name: pattern.split(/[\\/]/).pop()?.replace(/\.exe$/i, "") || pattern,
        executable: pattern, patterns: [pattern], source: "manual" }));
    return [...installedApps, ...custom].sort((a,b) => a.name.localeCompare(b.name));
  }, [installedApps, settingsDraft.redirect_processes]);
  const visibleApps = appChoices.filter(app => (!selectedAppsOnly || isAppSelected(app)) &&
    `${app.name} ${app.executable}`.toLowerCase().includes(appSearch.toLowerCase()));
  const selectedAppCount = appChoices.filter(isAppSelected).length;

  async function scanApplications() {
    setAppsScanning(true); setAppsError("");
    try { setInstalledApps(await invoke<InstalledApp[]>("scan_installed_applications")); }
    catch (error) { setAppsError(String(error)); }
    finally { setAppsScanning(false); setAppsLoaded(true); }
  }

  function toggleApplication(app: InstalledApp, checked: boolean) {
    const aliases = new Set(appAliases(app));
    const patterns = settingsDraft.redirect_processes.filter(pattern => !aliases.has(normalizeAppPattern(pattern)));
    updateSettings("redirect_processes", checked ? [...patterns, ...app.patterns] : patterns);
  }

  async function addApplicationFile() {
    try {
      const chosen = await open({ multiple: true, filters: [{ name: isMac ? "Приложения macOS" : "Приложения Windows", extensions: [isMac ? "app" : "exe"] }] });
      const paths = typeof chosen === "string" ? [chosen] : chosen || [];
      if (!paths.length) return;
      const entries = isMac ? await invoke<InstalledApp[]>("resolve_application_paths", { paths }) : paths.map(path => installedApps.find(app => normalizeAppPattern(app.executable) === normalizeAppPattern(path)) ||
        ({id: normalizeAppPattern(path), name: path.split(/[\\/]/).pop()?.replace(/\.exe$/i, "") || path,
        executable: path, patterns: [path], source: "manual"}));
      setInstalledApps(current => [...current.filter(app => !entries.some(entry => entry.id === app.id)), ...entries]);
      updateSettings("redirect_processes", [...new Set([...settingsDraft.redirect_processes, ...entries.flatMap(app => app.patterns)])]);
    } catch (error) { setMessage(String(error)); }
  }

  function applicationSettings() {
    // Expand selected app families, including their updaters, when saving old rules.
    let patterns = [...settingsDraft.redirect_processes];
    for (const app of appChoices.filter(isAppSelected)) {
      const aliases = new Set(appAliases(app));
      patterns = [...patterns.filter(pattern => !aliases.has(normalizeAppPattern(pattern))), ...app.patterns];
    }
    return {...settingsDraft, redirect_processes: [...new Map(patterns.filter(Boolean).map(pattern => [normalizeAppPattern(pattern), pattern])).values()]};
  }

  async function refreshAll() {
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    try {
      const [nextConfig, nextStatus, nextTraffic, nextLogs, nextRedirect] = await Promise.all([
        invoke<AppConfig>("get_config"),
        invoke<TunnelStatus>("tunnel_status"),
        invoke<TrafficPoint[]>("traffic_history"),
        invoke<string[]>("get_logs"),
        invoke<RedirectStatus>("redirect_status"),
      ]);
      setConfig(nextConfig);
      setStatus(nextStatus);
      setTraffic(nextTraffic);
      setLogs(nextLogs);
      setRedirect(nextRedirect);
      if (!settingsDirtyRef.current) setSettingsDraft(nextConfig.settings);
      setServerDraft((current) => (serverDraftDirtyRef.current ? current : selectedServer(nextConfig)));
      setProxyDraft((current) => (proxyDraftDirtyRef.current ? current : selectedProxy(nextConfig)));
    } catch (error) {
      setMessage(String(error));
    } finally {
      refreshInFlightRef.current = false;
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
    serverDraftDirtyRef.current = true;
    setServerDraft((current) => ({
      ...current,
      [key]: value,
      id: key === "name" && !current.id ? makeId(String(value)) : current.id,
    }));
  }

  function updateProxy<K extends keyof ProxyProfile>(key: K, value: ProxyProfile[K]) {
    proxyDraftDirtyRef.current = true;
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

  useEffect(() => {
    if (activeTab === "applications" && !appsLoaded && !appsScanning) scanApplications();
  }, [activeTab]);

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
          <button className={activeTab === "applications" ? "active" : ""} onClick={() => setActiveTab("applications")}>
            <Shield size={18} />
            Приложения
          </button>
          <button className={activeTab === "domains" ? "active" : ""} onClick={() => setActiveTab("domains")}><SlidersHorizontal size={18} />Домены</button>
        </nav>
      </aside>

      <section className={`workspace${message ? " hasMessage" : ""}${(activeTab === "applications" || activeTab === "domains") ? " appsWorkspace" : ""}`}>
        {message && <div className="actionMessage" role="status"><span>{message}</span><button onClick={() => setMessage("")} aria-label="Скрыть сообщение">×</button></div>}
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
                    disabled={busy !== null || sessionActive || !currentServer.id || !currentProxy.id}
                    onClick={() => runAction("connect", () => invoke<string>("start_tunnel"))}
                  >
                    <Play size={16} />
                    Запустить
                  </button>
                  <button className="danger" disabled={busy !== null || !sessionActive} onClick={() => runAction("disconnect", () => invoke<string>("stop_tunnel"))}>
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
                    disabled={sessionActive}
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
                    disabled={sessionActive}
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

              {status.ssh_route?.length > 1 && <div className="notice">{status.ssh_route.join(" → ")}</div>}
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
                      onClick={() => {
                        serverDraftDirtyRef.current = true;
                        setServerDraft(server);
                      }}
                    >
                      <span>{server.name}</span>
                      <strong>{server.user}@{maskValue(server.host)}:{server.port}</strong>
                    </button>
                  ))}
                </div>
                <button onClick={() => {
                  serverDraftDirtyRef.current = true;
                  setServerDraft(emptyServer);
                }}>
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
                    Подключаться через SSH-сервер
                    <select value={serverDraft.jump_server_id ?? ""} onChange={(event) => updateServer("jump_server_id", event.target.value || null)}>
                      <option value="">Напрямую</option>
                      {config.servers.filter((server) => server.id !== serverDraft.id).map((server) => (
                        <option key={server.id} value={server.id}>{server.name}</option>
                      ))}
                    </select>
                    {serverDraft.jump_server_id && <span>{serverRouteLabel(serverDraft, config.servers)} · Embedded SSH</span>}
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
                    const savedServer = serverDraft;
                    await invoke<AppConfig>("save_server", { server: savedServer });
                    await invoke<AppConfig>("select_server", { id: savedServer.id });
                    serverDraftDirtyRef.current = false;
                    setServerDraft(savedServer);
                    return "Server saved";
                  })}>
                    <Save size={16} />
                    Сохранить
                  </button>
                  <button disabled={busy !== null || !serverDraft.id} onClick={() => runAction("test", () => invoke<string>("test_server", { server: serverDraft }))}>
                    <Terminal size={16} />
                    {(serverDraft.jump_server_id || config.settings.ssh_engine === "embedded") &&
                      !config.servers.some((profile) => profile.jump_server_id === serverDraft.id)
                      ? "Проверить SSH и агент" : "Проверить SSH"}
                  </button>
                  <button disabled={busy !== null || !serverDraft.id || sessionActive} onClick={() => runAction("delete", async () => {
                    await invoke<AppConfig>("delete_server", { id: serverDraft.id });
                    serverDraftDirtyRef.current = false;
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
                      onClick={() => {
                        proxyDraftDirtyRef.current = true;
                        setProxyDraft(proxy);
                      }}
                    >
                      <span>{proxy.name}</span>
                      <strong>{proxy.host}:{proxy.port}</strong>
                    </button>
                  ))}
                </div>
                <button onClick={() => {
                  proxyDraftDirtyRef.current = true;
                  setProxyDraft(emptyProxy);
                }}>
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
                    <input value={proxyDraft.host} disabled={sessionActive} onChange={(event) => updateProxy("host", event.target.value)} placeholder="127.0.0.1" />
                  </label>
                  <label>
                    Port
                    <input type="number" disabled={sessionActive} value={proxyDraft.port} onChange={(event) => updateProxy("port", Number(event.target.value))} />
                  </label>
                </div>
                <div className="actions left">
                  <button className="primary" disabled={busy !== null || sessionActive} onClick={() => runAction("save", async () => {
                    const savedProxy = proxyDraft;
                    await invoke<AppConfig>("save_proxy", { proxy: savedProxy });
                    await invoke<AppConfig>("select_proxy", { id: savedProxy.id });
                    proxyDraftDirtyRef.current = false;
                    setProxyDraft(savedProxy);
                    return "Proxy saved";
                  })}>
                    <Save size={16} />
                    Сохранить
                  </button>
                  <button disabled={busy !== null || !proxyDraft.id || sessionActive} onClick={() => runAction("delete", async () => {
                    await invoke<AppConfig>("delete_proxy", { id: proxyDraft.id });
                    proxyDraftDirtyRef.current = false;
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

          {activeTab === "applications" && (
            <section className="panel narrow applicationsPanel">
              <div className="applicationBody">
              <div className="panelTitle"><Shield size={18} />Трафик приложений</div>
              <p>{isMac ? "Выберите приложения macOS, чей трафик нужно направить через SSH-туннель." : "Отметьте программы: их TCP и UDP будут идти через SSH-туннель."}</p>
              <div role="status" className={redirect.running ? "state good" : "state idle"}>
                {redirect.starting ? "Ожидаем разрешение macOS" : redirect.running ? "Перехват включён" : "Перехват выключен"}
              </div>
              <p>Остальные приложения работают напрямую. После выключения перехвата выбранные приложения также выходят напрямую.</p>
              {!redirectSupported && redirect.platform !== "unknown" && <div className="notice">Перехват приложений доступен на macOS и Windows.</div>}
              {redirect.requires_administrator && !redirect.administrator && <div className="notice">Для включения перехвата запустите Secret Tunnel от имени администратора.</div>}
              {!redirect.component_available && <div className="notice">{isMac ? <>Установите <a href="https://github.com/InterceptSuite/ProxyBridge/releases" target="_blank" rel="noreferrer">ProxyBridge 4.1.2 или новее</a> в /Applications. Закройте его окно перед включением перехвата здесь. При первом запуске macOS попросит разрешить сетевое расширение.</> : "Комплект перехвата не установлен или неполон. Укажите путь к проверенному компоненту и сохраните настройки."}</div>}
              {isMac && <div className="notice">{status.ssh_engine === "embedded" ? "TCP и UDP идут через Embedded SSH. Для TCP и UDP обновите secret-tunnel-agent на SSH-сервере до версии 0.2.3." : "System OpenSSH: TCP идёт через SSH, UDP выбранных приложений блокируется. Для TCP и UDP выберите Embedded Rust SSH и установите агент 0.2.3 на сервере."} Системный DNS может идти напрямую.</div>}
              {!isMac && status.ssh_engine !== "embedded" && <div className="notice">Для TCP и UDP выберите Embedded Rust SSH в настройках и запустите туннель.</div>}
              {isMac && (settingsDraft.domain_rules || []).some(r => r.enabled) && <div className="notice">Отключите доменные правила: их применение на macOS пока не поддерживается.</div>}
              {isMac && settingsDraft.redirect_processes.some(id => ["shipit", "chrome_crashpad_handler"].includes(id.toLowerCase())) && <div className="notice">ShipIt и chrome_crashpad_handler используются разными Electron-приложениями. Эти правила также направят в туннель одноимённые утилиты других программ.</div>}
              {isMac && redirectActive && redirect.setup_message && <div className="notice" role="status">{redirect.setup_message}</div>}
              {redirect.last_error && <div className="notice">{redirect.last_error}</div>}
              <div className="appChooser">
                <div className="appChooserToolbar">
                  <input aria-label="Поиск программы" placeholder="Найти программу: Telegram, Firefox…" value={appSearch} onChange={event => setAppSearch(event.target.value)} />
                  <button disabled={appsScanning} onClick={scanApplications}><RefreshCcw size={16} />{appsScanning ? "Ищем…" : "Обновить"}</button>
                </div>
                <div className="appChooserSummary">
                  <span>Выбрано: {selectedAppCount} · Найдено: {installedApps.length}</span>
                  <label className="checkboxLine"><input type="checkbox" checked={selectedAppsOnly} onChange={event => setSelectedAppsOnly(event.target.checked)} />Только выбранные</label>
                </div>
                {appsError && <div className="notice">{appsError}</div>}
                <div className="appChoices" aria-label="Установленные программы" aria-busy={appsScanning}>
                  {visibleApps.map(app => <label key={app.id} className={isAppSelected(app) ? "appChoice selected" : "appChoice"}>
                    <input type="checkbox" checked={isAppSelected(app)} disabled={busy !== null} onChange={event => toggleApplication(app, event.target.checked)} />
                    <span className="appBadge" aria-hidden="true">{app.name.slice(0,1).toUpperCase()}</span>
                    <span className="appDescription"><strong>{app.name}</strong><small title={app.executable}>{app.executable}</small>
                      {app.patterns.length > 1 && <small title={app.patterns.join("\n")}>Включая вспомогательные процессы ({app.patterns.length - 1})</small>}</span>
                  </label>)}
                  {!visibleApps.length && <p className="appListEmpty">{appsScanning ? "Ищем установленные программы…" : `Программы не найдены. Измените поиск или добавьте ${isMac ? ".app" : "exe-файл"}.`}</p>}
                </div>
                <button disabled={busy !== null} onClick={addApplicationFile}><Plus size={16} />Добавить программу из файла</button>
              </div>
              <details className="appAdvanced"><summary>Дополнительные настройки</summary>
              <label>{isMac ? "Bundle ID или идентификаторы подписи через запятую" : "Правила вручную (имена exe или полные пути через запятую)"}
                <input disabled={redirectActive} value={settingsDraft.redirect_processes.join(", ")}
                  onChange={event => updateSettings("redirect_processes",event.target.value.split(",").map(name=>name.trim()))} />
              </label>
              <label>
                Компонент перехвата
                <div className="fileField">
                  <input disabled={redirectActive} value={settingsDraft.redirect_cli_path}
                    onChange={(event) => updateSettings("redirect_cli_path", event.target.value)} placeholder={isMac ? "/Applications/ProxyBridge.app/Contents/MacOS/ProxyBridge" : "Путь к ProxyBridge_CLI.exe"} />
                  <button disabled={redirectActive} title="Выбрать компонент" onClick={async () => {
                    try {
                      const selected = await open({ multiple: false, ...(isMac ? {} : {filters: [{ name: "ProxyBridge CLI", extensions: ["exe"] }]}) });
                      if (typeof selected === "string") updateSettings("redirect_cli_path", selected);
                    } catch (error) { setMessage(String(error)); }
                  }}><FolderOpen size={16} /></button>
                </div>
              </label>
              <p>{isMac ? "Нужен ProxyBridge с разрешённым системным расширением. Приложения сопоставляются по точным bundle ID, включая обнаруженные вложенные .app и .xpc." : "Нужен проверенный комплект ProxyBridge с WinDivert."} Перехват действует на новые соединения: после включения переподключите или перезапустите выбранные программы. При аварии блокировка прямого выхода не гарантируется.</p>
              </details>
              </div>
              <div className="actions left applicationActions">
                <button className="primary" disabled={busy !== null || !canRedirect || (selectedAppCount === 0 && !(settingsDraft.domain_rules || []).some(r => r.enabled)) || (isMac && (settingsDraft.domain_rules || []).some(r => r.enabled))}
                  onClick={() => runAction("connect", async () => {
                    const settings = applicationSettings();
                    await invoke<AppConfig>("save_settings", { settings });
                    setSettingsDraft(settings);
                    settingsDirtyRef.current = false;
                    return invoke<string>("start_redirect");
                  })}><Play size={16} />Включить перехват</button>
                <button disabled={busy !== null || !redirectActive}
                  onClick={() => runAction("disconnect", () => invoke<string>("stop_redirect"))}>
                  <CircleStop size={16} />{redirect.starting ? "Отменить запуск" : "Выключить"}
                </button>
                <button disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const settings = applicationSettings();
                    await invoke<AppConfig>("save_settings", { settings });
                    setSettingsDraft(settings);
                  settingsDirtyRef.current = false;
                  if (redirectActive) {
                    await invoke<string>("stop_redirect");
                    if (settings.redirect_processes.length || (settings.domain_rules || []).some(r => r.enabled)) return invoke<string>("start_redirect");
                    return "Выбор очищен. Перехват выключен";
                  }
                  return "Выбор приложений сохранён";
                })}><Save size={16} />{redirectActive ? "Применить выбор" : "Сохранить выбор"}</button>
              </div>
            </section>
          )}

          {activeTab === "domains" && <><div className="notice" hidden={redirect.supports_domains}>Доменные regex-правила применяются только в Windows. На macOS используйте выбор приложений; отключите ранее сохранённые доменные правила перед запуском.</div><DomainsPanel rules={settingsDraft.domain_rules || []} stats={redirect.domain_stats}
            running={redirect.running} busy={busy !== null} canStart={canRedirect && redirect.supports_domains}
            onChange={rules => updateSettings("domain_rules", rules)}
            onSave={() => runAction("save", async () => {
              await invoke<AppConfig>("save_settings", {settings: settingsDraft});
              settingsDirtyRef.current = false;
              if (redirect.running) {
                await invoke<string>("stop_redirect");
                return invoke<string>("start_redirect");
              }
              return "Правила доменов сохранены. Включите перехват для применения";
            })}
            onStart={() => runAction("connect", async () => {
              await invoke<AppConfig>("save_settings", {settings: settingsDraft}); settingsDirtyRef.current = false;
              return invoke<string>("start_redirect");
            })}
            onStop={() => runAction("disconnect", () => invoke<string>("stop_redirect"))} /></>}

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
              <details>
                <summary>Расширенные · Embedded SSH</summary>
                <p className="hint">Применяется при следующем подключении через встроенный движок, включая все звенья SSH-цепочки. Сжатие выключено.</p>
                <label>
                  Шифрование
                  <select value={settingsDraft.ssh_tuning?.cipher ?? "auto"} onChange={(event) => updateSettings("ssh_tuning", {...defaultSshTuning,...settingsDraft.ssh_tuning,cipher:event.target.value as SshTuning["cipher"]})}>
                    <option value="auto">Авто</option>
                    <option value="aes128_gcm">AES-128-GCM</option>
                    <option value="chacha20_poly1305">ChaCha20-Poly1305</option>
                  </select>
                </label>
                <label>
                  Очередь SSH
                  <select value={settingsDraft.ssh_tuning?.channel_buffer_messages === 8 ? "small" : "default"} onChange={(event) => updateSettings("ssh_tuning", {...defaultSshTuning,...settingsDraft.ssh_tuning,channel_buffer_messages:event.target.value === "small" ? 8 : 32,channel_receive_window_bytes:event.target.value === "small" ? 1048576 : 2097152})}>
                    <option value="default">Стандартная · 32 сообщения / 2 МиБ</option>
                    <option value="small">Уменьшенная · 8 сообщений / 1 МиБ · тест</option>
                  </select>
                </label>
                <p className="hint">Уменьшенная очередь предназначена для сравнения задержек под нагрузкой и может ограничить скорость на соединениях с большим RTT.</p>
              </details>
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
                  const settings = await invoke<AppSettings>("default_settings");
                  const nextConfig = await invoke<AppConfig>("save_settings", { settings });
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
