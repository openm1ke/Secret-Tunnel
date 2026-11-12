import DomainsPanel from "./DomainsPanel";
import StatsPanel, { SessionRecord, knownAppMatcher } from "./StatsPanel";
import SpeedPanel, { SpeedLive, SpeedRun, emptyLive } from "./SpeedPanel";
import React, { useEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowDown,
  ArrowUp,
  ChartColumn,
  Check,
  CircleStop,
  Copy,
  Eye,
  EyeOff,
  FolderOpen,
  Gauge,
  Globe,
  HelpCircle,
  LayoutGrid,
  Network,
  Play,
  Plus,
  Power,
  RefreshCcw,
  Save,
  Search,
  Server,
  Settings,
  Star,
  Terminal,
  Trash2,
} from "lucide-react";
import appIconUrl from "./assets/app-icon.png";
import {
  Flag,
  LogDock,
  RouteNode,
  RouteRibbon,
  TrafficChart,
  TrafficPoint,
  countryName,
  formatBitRate,
  formatBytes,
} from "./components";
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

type ProxyProfile = { id: string; name: string; host: string; port: number };

type SshTuning = {
  cipher: "auto" | "aes128_gcm" | "chacha20_poly1305";
  channel_buffer_messages: number;
  channel_receive_window_bytes: number;
  tcp_interactive_bytes?: number | null;
};
const balancedTcpBudget = 96 * 1024;
const defaultSshTuning: SshTuning = { cipher: "auto", channel_buffer_messages: 32, channel_receive_window_bytes: 4194304, tcp_interactive_bytes: balancedTcpBudget };

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
  bypass_vpn_for_ssh?: boolean;
  follow_child_processes?: boolean;
  close_to_tray?: boolean;
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

/** A program started by a selected application that went around the tunnel. */
type Leak = { app: string; process: string; path: string; rule: string; flows: number };
/** A program a selected application started that was given a rule while running. */
type Followed = { app: string; process: string; rule: string };
type Tab = "connect" | "servers" | "proxies" | "applications" | "domains" | "stats" | "speed" | "settings";
type DomainRule = { id: string; pattern: string; enabled: boolean; action: "tunnel" | "direct" };
type DomainStats = { events: number; unknown: number; rules: { id: string; observed: number; tunnel: number; direct: number }[] };
type RedirectStatus = { running: boolean; starting: boolean; platform: string; requires_administrator: boolean; supports_system_ssh: boolean; supports_domains: boolean; setup_message: string; pid: number | null; administrator: boolean; component_available: boolean; last_error: string; domain_stats?: DomainStats };
type InstalledApp = { id: string; name: string; executable: string; patterns: string[]; source: string; primary?: string };
const normalizeAppPattern = (value: string) => value.trim().replace(/\//g, "\\").toLowerCase();
/** Every rule an application contributes, including its helpers. */
const appAliases = (app: InstalledApp) => (app.source === "macos" ? app.patterns : [...app.patterns, app.executable.split(/[\\/]/).pop() || ""]).map(normalizeAppPattern);
/** Rules that mean the application itself is selected. On macOS helper IDs
 * such as Sparkle's or Mozilla's are shared by many applications, so only the
 * bundle's own identifier counts. */
const appOwnKeys = (app: InstalledApp) => (app.source === "macos" && app.primary ? [normalizeAppPattern(app.primary)] : appAliases(app));
type BusyAction = "connect" | "disconnect" | "restart" | "save" | "delete" | "test" | "logs" | null;
type CopyField = "proxy-ip" | "server-host" | "ipinfo-token" | "logs" | null;
type Phase = "up" | "busy" | "down" | "error";

const emptyServer: ServerProfile = {
  id: "", name: "", host: "", port: 22, user: "", key_path: "~/.ssh/id_ed25519", jump_server_id: null, compression: false,
  strict_host_key_checking: "yes", connect_timeout: 8, server_alive_interval: 15, server_alive_count_max: 3,
};
const emptyProxy: ProxyProfile = { id: "", name: "", host: "127.0.0.1", port: 1080 };
const emptyConfig: AppConfig = {
  selected_server_id: "", selected_proxy_id: "", servers: [], proxies: [],
  settings: {
    redirect_cli_path: "", redirect_processes: [], domain_rules: [], ipinfo_token: "", ssh_engine: "embedded", hide_sensitive: true,
    verify_proxy_on_start: true, auto_reconnect: false, reconnect_delay_seconds: 3, start_tunnel_on_launch: false, clear_logs_on_start: false,
  },
};
const emptyStatus: TunnelStatus = {
  state: "stopped", state_emoji: "", app_version: "", ssh_engine: "embedded", server_id: "", server_name: "", ssh_route: [],
  proxy_id: "", proxy_name: "", proxy_host: "127.0.0.1", proxy_port: 1080, pid: null, started_at: null, ip: "",
  country_code: "", country_name: "", country_flag: "", rx_total: 0, tx_total: 0, last_error: "",
};

// Backend messages that are still in English, as people read them here.
const messageText: Record<string, string> = {
  "Tunnel startup requested": "Запускаем туннель…",
  "Tunnel stop requested": "Останавливаем туннель…",
  "Tunnel restart requested": "Перезапускаем туннель…",
};

function SettingHint({ text }: { text: string }) {
  return (
    <span className="settingHint" tabIndex={0} aria-label={text}>
      <HelpCircle size={14} />
      <span className="tooltip" role="tooltip">{text}</span>
    </span>
  );
}

function SettingLabel({ children, hint }: { children: React.ReactNode; hint: string }) {
  return <span className="settingLabel"><span>{children}</span><SettingHint text={hint} /></span>;
}

function makeId(name: string) {
  return name.trim().toLowerCase().replace(/[^a-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
}

function phaseOf(status: TunnelStatus): Phase {
  if (status.state === "running") return "up";
  if (["starting", "reconnecting", "stopping", "unknown"].includes(status.state)) return "busy";
  if (status.state === "failed" || status.last_error) return "error";
  return "down";
}

function stateTitle(state: string) {
  switch (state) {
    case "running": return "Подключено";
    case "starting": return "Подключаемся";
    case "reconnecting": return "Переподключаемся";
    case "stopping": return "Отключаемся";
    case "failed": return "Не удалось подключиться";
    case "unknown": return "Проверяем";
    default: return "Отключено";
  }
}

function engineLabel(engine: string) {
  return engine === "embedded" ? "Встроенный SSH" : "Системный OpenSSH";
}

function selectedServer(config: AppConfig) {
  return config.servers.find((server) => server.id === config.selected_server_id) ?? config.servers[0] ?? emptyServer;
}

function selectedProxy(config: AppConfig) {
  return config.proxies.find((proxy) => proxy.id === config.selected_proxy_id) ?? config.proxies[0] ?? emptyProxy;
}

/** Names from the first hop to the server itself, or null for a broken chain. */
function serverChain(server: ServerProfile, servers: ServerProfile[]): string[] | null {
  const route = [server.name];
  const seen = new Set([server.id]);
  let next = server.jump_server_id;
  while (next) {
    const hop = servers.find((item) => item.id === next);
    if (!hop || seen.has(next) || route.length >= 4) return null;
    seen.add(next);
    route.unshift(hop.name);
    next = hop.jump_server_id;
  }
  return route;
}

function formatUptime(startedAt: number | null, now: number) {
  if (!startedAt) return "—";
  const seconds = Math.max(0, Math.floor(now / 1000 - startedAt));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (hours) return `${hours} ч ${minutes} мин`;
  if (minutes) return `${minutes} мин`;
  return `${seconds} с`;
}

function maskValue(value: string) {
  return value ? "••••••••" : "—";
}

function dedupePatterns(patterns: string[]) {
  return [...new Map(patterns.filter((pattern) => pattern.trim()).map((pattern) => [normalizeAppPattern(pattern), pattern.trim()])).values()];
}

function plural(count: number, one: string, few: string, many: string) {
  const mod10 = count % 10;
  const mod100 = count % 100;
  if (mod10 === 1 && mod100 !== 11) return one;
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return few;
  return many;
}

function appSubtitle(app: InstalledApp) {
  if (app.source === "manual") return "добавлено вручную";
  const helpers = app.patterns.length - 1;
  const place = app.executable.startsWith("/System/") ? "Системное" : app.executable.startsWith("/Applications/") ? "Программы" : app.executable.replace(/\/[^/]+$/, "");
  return helpers > 0 ? `${place} · +${helpers} ${plural(helpers, "процесс", "процесса", "процессов")}` : place;
}

function AppIcon({ app, icon }: { app: InstalledApp; icon?: string }) {
  if (icon) return <img className="appIcon" src={icon} alt="" />;
  return <span className="appIcon letter" aria-hidden="true">{app.name.slice(0, 1).toUpperCase()}</span>;
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
  const [appIcons, setAppIcons] = useState<Record<string, string>>({});
  const [appsLoaded, setAppsLoaded] = useState(false);
  const [appsScanning, setAppsScanning] = useState(false);
  const [appsError, setAppsError] = useState("");
  const [appSearch, setAppSearch] = useState("");
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [speedLive, setSpeedLive] = useState<SpeedLive>(emptyLive);
  const [speedHistory, setSpeedHistory] = useState<SpeedRun[]>([]);
  const [leaks, setLeaks] = useState<Leak[]>([]);
  const [followed, setFollowed] = useState<Followed[]>([]);
  const [selectedAppsOnly, setSelectedAppsOnly] = useState(false);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const [busy, setBusy] = useState<BusyAction>(null);
  const [showServerHost, setShowServerHost] = useState(false);
  const [showIpinfoToken, setShowIpinfoToken] = useState(false);
  const [copiedField, setCopiedField] = useState<CopyField>(null);
  const [now, setNow] = useState(Date.now());
  const serverDraftDirtyRef = useRef(false);
  const proxyDraftDirtyRef = useRef(false);
  const settingsDirtyRef = useRef(false);
  const refreshInFlightRef = useRef(false);
  const iconRequestsRef = useRef(new Set<string>());

  const currentServer = useMemo(() => selectedServer(config), [config]);
  const currentProxy = useMemo(() => selectedProxy(config), [config]);
  const phase = phaseOf(status);
  const connected = status.state === "running";
  const sessionActive = ["running", "starting", "reconnecting", "stopping"].includes(status.state);
  const isMac = redirect.platform === "macos";
  const redirectActive = redirect.running || redirect.starting;
  const redirectSupported = isMac || redirect.platform === "windows";
  const canRedirect = redirectSupported && connected && !redirectActive && redirect.component_available &&
    (!redirect.requires_administrator || redirect.administrator) &&
    (redirect.supports_system_ssh || status.ssh_engine === "embedded");
  const showSensitive = !config.settings.hide_sensitive;
  const visibleIp = showSensitive ? status.ip || "—" : maskValue(status.ip);
  const country = countryName(status.country_code, status.country_name);
  const latest = traffic[traffic.length - 1];
  const latestPing = [...traffic].reverse().find((point) => point.latency_ms !== null)?.latency_ms ?? null;
  const selectedPatterns = new Set(settingsDraft.redirect_processes.map(normalizeAppPattern));
  const isAppSelected = (app: InstalledApp) => appOwnKeys(app).some((pattern) => selectedPatterns.has(pattern));
  const appChoices = useMemo(() => {
    const known = new Set(installedApps.flatMap(appAliases));
    const custom = settingsDraft.redirect_processes.filter((pattern) => pattern && !known.has(normalizeAppPattern(pattern)))
      .map((pattern) => ({ id: normalizeAppPattern(pattern), name: pattern.split(/[\\/]/).pop()?.replace(/\.exe$/i, "") || pattern,
        executable: pattern, patterns: [pattern], source: "manual" }));
    return [...installedApps, ...custom].sort((a, b) => a.name.localeCompare(b.name));
  }, [installedApps, settingsDraft.redirect_processes]);
  const visibleApps = appChoices
    .filter((app) => (!selectedAppsOnly || isAppSelected(app)) && `${app.name} ${app.executable}`.toLowerCase().includes(appSearch.toLowerCase()))
    .sort((a, b) => Number(isAppSelected(b)) - Number(isAppSelected(a)));
  const selectedApps = appChoices.filter(isAppSelected);
  // Installed applications that appear in the session history, for icons.
  const statsKnownApps = useMemo(() => {
    const match = knownAppMatcher(installedApps);
    const names = new Set(sessions.flatMap((session) => session.apps.map((app) => app.name)));
    const found = new Map<string, InstalledApp>();
    names.forEach((name) => {
      const app = match(name);
      if (app) found.set(app.executable, app);
    });
    return [...found.values()];
  }, [installedApps, sessions]);

  const chain = serverChain(currentServer, config.servers) ?? [currentServer.name];
  const routeNames = connected && status.ssh_route?.length ? status.ssh_route : chain;
  const routeNodes: RouteNode[] = [
    { label: isMac ? "Этот Mac" : redirect.platform === "windows" ? "Этот ПК" : "Этот компьютер", kind: "device" },
    ...routeNames.filter(Boolean).map((name, index, all) => ({ label: name, kind: index === all.length - 1 ? "exit" as const : "hop" as const })),
    { label: "Интернет", kind: "internet", flag: connected ? status.country_code : undefined },
  ];

  function notify(text: string, error = false) {
    setMessage({ text: messageText[text] ?? text, error });
  }

  useEffect(() => {
    if (!message || message.error) return;
    const timer = window.setTimeout(() => setMessage(null), 4500);
    return () => window.clearTimeout(timer);
  }, [message]);

  async function scanApplications() {
    setAppsScanning(true);
    setAppsError("");
    try {
      setInstalledApps(await invoke<InstalledApp[]>("scan_installed_applications"));
    } catch (error) {
      setAppsError(String(error));
    } finally {
      setAppsScanning(false);
      setAppsLoaded(true);
    }
  }

  // Icons load in the background for the apps on screen, a batch at a time:
  // .app bundles on macOS, programs and shortcuts by full path on Windows.
  useEffect(() => {
    if (activeTab !== "applications" && activeTab !== "stats") return;
    const source = activeTab === "stats" ? statsKnownApps : visibleApps;
    const hasIcon = (path: string) => (isMac ? path.endsWith(".app") : /^[a-z]:\\|^\\\\/i.test(path));
    const wanted = source.map((app) => app.executable)
      .filter((path) => hasIcon(path) && !iconRequestsRef.current.has(path)).slice(0, 48);
    if (!wanted.length) return;
    wanted.forEach((path) => iconRequestsRef.current.add(path));
    invoke<Record<string, string>>("application_icons", { paths: wanted })
      .then((icons) => setAppIcons((current) => ({ ...current, ...icons })))
      .catch(() => undefined);
  }, [activeTab, isMac, visibleApps.map((app) => app.id).join("|"), statsKnownApps.map((app) => app.executable).join("|"), appIcons]);

  function toggleApplication(app: InstalledApp, checked: boolean) {
    const current = settingsDraft.redirect_processes;
    if (checked) {
      updateSettings("redirect_processes", dedupePatterns([...current, ...app.patterns]));
      return;
    }
    // Keep helpers another selected application still needs.
    const stillNeeded = new Set(appChoices.filter((other) => other.id !== app.id && isAppSelected(other)).flatMap(appAliases));
    const remove = new Set([...appAliases(app).filter((pattern) => !stillNeeded.has(pattern)), ...appOwnKeys(app)]);
    updateSettings("redirect_processes", current.filter((pattern) => !remove.has(normalizeAppPattern(pattern))));
  }

  async function addApplicationFile() {
    try {
      const chosen = await open({ multiple: true, filters: [{ name: isMac ? "Приложения macOS" : "Программы Windows", extensions: [isMac ? "app" : "exe"] }] });
      const paths = typeof chosen === "string" ? [chosen] : chosen || [];
      if (!paths.length) return;
      const entries = isMac ? await invoke<InstalledApp[]>("resolve_application_paths", { paths }) : paths.map((path) => installedApps.find((app) => normalizeAppPattern(app.executable) === normalizeAppPattern(path)) ||
        ({ id: normalizeAppPattern(path), name: path.split(/[\\/]/).pop()?.replace(/\.exe$/i, "") || path, executable: path, patterns: [path], source: "manual" }));
      setInstalledApps((current) => [...current.filter((app) => !entries.some((entry) => entry.id === app.id)), ...entries]);
      updateSettings("redirect_processes", [...new Set([...settingsDraft.redirect_processes, ...entries.flatMap((app) => app.patterns)])]);
    } catch (error) {
      notify(String(error), true);
    }
  }

  function applicationSettings() {
    // Exactly the rules of the selected applications, with their helpers,
    // plus manual rules that belong to no found application. Leftovers of
    // deselected applications (shared helper IDs) are dropped.
    const known = new Set(installedApps.flatMap(appAliases));
    const manual = settingsDraft.redirect_processes.filter((pattern) => pattern.trim() && !known.has(normalizeAppPattern(pattern)));
    const selected = installedApps.filter(isAppSelected).flatMap((app) => app.patterns);
    return { ...settingsDraft, redirect_processes: dedupePatterns([...selected, ...manual]) };
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
      setNow(Date.now());
      if (!settingsDirtyRef.current) setSettingsDraft(nextConfig.settings);
      setServerDraft((current) => (serverDraftDirtyRef.current ? current : selectedServer(nextConfig)));
      setProxyDraft((current) => (proxyDraftDirtyRef.current ? current : selectedProxy(nextConfig)));
    } catch (error) {
      notify(String(error), true);
    } finally {
      refreshInFlightRef.current = false;
    }
  }

  async function runAction(action: BusyAction, callback: () => Promise<string | void>) {
    setBusy(action);
    setMessage(null);
    try {
      const output = await callback();
      if (output) notify(output);
    } catch (error) {
      notify(String(error), true);
    } finally {
      await refreshAll();
      setBusy(null);
    }
  }

  function updateServer<K extends keyof ServerProfile>(key: K, value: ServerProfile[K]) {
    serverDraftDirtyRef.current = true;
    setServerDraft((current) => ({ ...current, [key]: value, id: key === "name" && !current.id ? makeId(String(value)) : current.id }));
  }

  function updateProxy<K extends keyof ProxyProfile>(key: K, value: ProxyProfile[K]) {
    proxyDraftDirtyRef.current = true;
    setProxyDraft((current) => ({ ...current, [key]: value, id: key === "name" && !current.id ? makeId(String(value)) : current.id }));
  }

  function updateSettings<K extends keyof AppSettings>(key: K, value: AppSettings[K]) {
    settingsDirtyRef.current = true;
    setSettingsDraft((current) => ({ ...current, [key]: value }));
  }

  async function choosePrivateKey() {
    try {
      const selected = await open({ multiple: false, directory: false, title: "Выберите приватный ключ SSH" });
      if (typeof selected === "string" && selected.trim()) updateServer("key_path", selected);
    } catch (error) {
      notify(String(error), true);
    }
  }

  async function setSensitiveVisible(visible: boolean) {
    await runAction("save", async () => {
      const nextConfig = await invoke<AppConfig>("save_settings", { settings: { ...config.settings, hide_sensitive: !visible } });
      setConfig(nextConfig);
      if (!settingsDirtyRef.current) setSettingsDraft(nextConfig.settings);
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
      if (field === "logs") notify("Журнал скопирован");
    } catch (error) {
      notify(String(error), true);
    }
  }

  useEffect(() => {
    refreshAll();
    const timer = window.setInterval(refreshAll, 2500);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if ((activeTab === "applications" || activeTab === "stats") && !appsLoaded && !appsScanning) scanApplications();
  }, [activeTab]);

  // The sidebar counts selected applications, not their rules, so the list
  // is needed as soon as interception runs.
  useEffect(() => {
    if (redirect.running && !appsLoaded && !appsScanning) scanApplications();
  }, [redirect.running]);

  // Programs the selected applications start under their own signing
  // identifier go direct until they have a rule of their own.
  useEffect(() => {
    if (activeTab !== "applications" || !redirect.running || !isMac) {
      setLeaks([]);
      setFollowed([]);
      return;
    }
    const load = () => {
      invoke<Leak[]>("redirect_leaks").then(setLeaks).catch(() => undefined);
      invoke<Followed[]>("redirect_followed").then(setFollowed).catch(() => undefined);
    };
    load();
    const timer = window.setInterval(load, 4000);
    return () => window.clearInterval(timer);
  }, [activeTab, redirect.running, isMac]);

  // The tray menu connects, disconnects and opens tabs without the window.
  useEffect(() => {
    const stops = [
      listen<{ text: string; error: boolean }>("tray-result", (event) => setMessage({ text: event.payload.text, error: event.payload.error })),
      listen<string>("tray-open", (event) => setActiveTab(event.payload as Tab)),
    ];
    return () => { stops.forEach((stop) => stop.then((unlisten) => unlisten())); };
  }, []);

  // Speed tests: the gauges follow a running test closely, history reloads
  // when one finishes.
  useEffect(() => {
    if (activeTab !== "speed") return;
    invoke<SpeedRun[]>("speedtest_history").then(setSpeedHistory).catch(() => undefined);
  }, [activeTab]);
  useEffect(() => {
    if (activeTab !== "speed" && !speedLive.running) return;
    let stopped = false;
    let timer = 0;
    const poll = async () => {
      try {
        const next = await invoke<SpeedLive>("speedtest_status");
        if (stopped) return;
        setSpeedLive((previous) => {
          if (previous.running && !next.running) invoke<SpeedRun[]>("speedtest_history").then(setSpeedHistory).catch(() => undefined);
          return next;
        });
        timer = window.setTimeout(poll, next.running ? 300 : 2000);
      } catch {
        timer = window.setTimeout(poll, 2000);
      }
    };
    poll();
    return () => { stopped = true; window.clearTimeout(timer); };
  }, [activeTab, speedLive.running]);

  // Session history refreshes while the statistics are on screen.
  useEffect(() => {
    if (activeTab !== "stats") return;
    const load = () => invoke<SessionRecord[]>("session_history").then(setSessions).catch(() => undefined);
    load();
    const timer = window.setInterval(load, 3000);
    return () => window.clearInterval(timer);
  }, [activeTab]);

  const ipActions = (light: boolean) => (
    <span className="inlineActions">
      <button type="button" className={light ? "tinyButton light" : "tinyButton"} title={showSensitive ? "Скрыть IP" : "Показать IP"}
        aria-label={showSensitive ? "Скрыть IP" : "Показать IP"} onClick={() => setSensitiveVisible(!showSensitive)}>
        {showSensitive ? <EyeOff size={13} /> : <Eye size={13} />}
      </button>
      <button type="button" className={light ? "tinyButton light" : "tinyButton"} title="Скопировать IP" aria-label="Скопировать IP"
        disabled={!status.ip} onClick={() => copyText(status.ip, "proxy-ip")}>
        {copiedField === "proxy-ip" ? <Check size={13} /> : <Copy size={13} />}
      </button>
    </span>
  );

  const nav: { tab: Tab; label: string; icon: React.ReactNode; badge?: string }[] = [
    { tab: "connect", label: "Подключение", icon: <Power size={17} /> },
    { tab: "servers", label: "Серверы", icon: <Server size={17} />, badge: String(config.servers.length || "") },
    { tab: "proxies", label: "Локальный прокси", icon: <Network size={17} /> },
    { tab: "applications", label: "Приложения", icon: <LayoutGrid size={17} />, badge: redirect.running ? "вкл" : undefined },
    { tab: "domains", label: "Домены", icon: <Globe size={17} /> },
    { tab: "stats", label: "Статистика", icon: <ChartColumn size={17} /> },
    { tab: "speed", label: "Скорость", icon: <Gauge size={17} />, badge: speedLive.running ? "тест" : undefined },
    { tab: "settings", label: "Настройки", icon: <Settings size={17} /> },
  ];

  return (
    <main className="app">
      <aside className="sidebar">
        <div className="brand">
          <img src={appIconUrl} alt="" className="brandIcon" />
          <div>
            <h1>Secret Tunnel</h1>
            <p>{status.app_version ? `Версия ${status.app_version}` : "SSH-туннель"}</p>
          </div>
        </div>

        <section className={`sideStatus phase-${phase}`} aria-live="polite">
          <div className="sideState"><span className="pulse" aria-hidden="true" />{stateTitle(status.state)}</div>
          <dl>
            <div><dt>Сервер</dt><dd title={routeNames.join(" → ")}>{currentServer.name || "не выбран"}</dd></div>
            <div><dt>Выход</dt><dd>{connected && status.country_code ? <><Flag code={status.country_code} label={country} /> {country}</> : "—"}</dd></div>
            <div><dt>IP</dt><dd className="withActions"><span className="mono">{connected ? visibleIp : "—"}</span>{connected && ipActions(false)}</dd></div>
            <div><dt>В сети</dt><dd>{connected ? formatUptime(status.started_at, now) : "—"}</dd></div>
            <div><dt>Сейчас</dt><dd className="rates" title="Скорость сейчас"><span><ArrowDown size={12} />{connected && latest ? formatBitRate(latest.rx_bps) : "—"}</span><span><ArrowUp size={12} />{connected && latest ? formatBitRate(latest.tx_bps) : "—"}</span></dd></div>
          </dl>
          {redirect.running && <p className="sideNote">Перехват: {selectedApps.length} прил.</p>}
        </section>

        <nav className="tabs" aria-label="Разделы">
          {nav.map((item) => (
            <button key={item.tab} className={activeTab === item.tab ? "active" : ""} onClick={() => setActiveTab(item.tab)} aria-current={activeTab === item.tab ? "page" : undefined}>
              {item.icon}<span>{item.label}</span>{item.badge && <em>{item.badge}</em>}
            </button>
          ))}
        </nav>
      </aside>

      <section className="workspace">
        {message && (
          <div className={message.error ? "toast error" : "toast"} role={message.error ? "alert" : "status"}>
            <span>{message.text}</span>
            <button onClick={() => setMessage(null)} aria-label="Скрыть сообщение">×</button>
          </div>
        )}
        <div className={activeTab === "connect" ? "workspaceContent fill" : "workspaceContent"}>
          {activeTab === "connect" && (
            <section className="connect">
              <div className={`hero phase-${phase}`}>
                <div className="heroMain">
                  <h2>{stateTitle(status.state)}</h2>
                  <p className="heroSub">
                    {connected ? <>Трафик выходит в интернет через {currentServer.name}{country ? <> · <Flag code={status.country_code} label={country} /> {country}</> : null}. В сети {formatUptime(status.started_at, now)}.</>
                      : phase === "busy" ? "Устанавливаем SSH-соединение и проверяем агент на сервере."
                      : phase === "error" ? (status.last_error || "Соединение прервалось. Подробности в журнале ниже.")
                      : currentServer.id ? `Нажмите «Подключить», чтобы направить трафик через ${currentServer.name}.` : "Добавьте SSH-сервер во вкладке «Серверы»."}
                  </p>
                  <RouteRibbon nodes={routeNodes} state={phase} />
                </div>
                <div className="heroActions">
                  {!sessionActive ? (
                    <button className="primary big" disabled={busy !== null || !currentServer.id || !currentProxy.id}
                      onClick={() => runAction("connect", () => invoke<string>("start_tunnel"))}>
                      <Play size={17} />Подключить
                    </button>
                  ) : (
                    <button className="danger big" disabled={busy !== null} onClick={() => runAction("disconnect", () => invoke<string>("stop_tunnel"))}>
                      <CircleStop size={17} />Отключить
                    </button>
                  )}
                  <button className="ghost" disabled={busy !== null || !currentServer.id || !currentProxy.id} onClick={() => runAction("restart", () => invoke<string>("restart_tunnel"))}>
                    <RefreshCcw size={15} />Переподключить
                  </button>
                </div>
              </div>

              <div className="pickers">
                <label>
                  <span>Сервер</span>
                  <select value={config.selected_server_id} disabled={sessionActive}
                    onChange={(event) => runAction("save", async () => { await invoke<AppConfig>("select_server", { id: event.target.value }); })}>
                    {config.servers.map((server) => {
                      const route = serverChain(server, config.servers);
                      return <option key={server.id} value={server.id}>{route && route.length > 1 ? route.join(" → ") : server.name}</option>;
                    })}
                  </select>
                </label>
                <label>
                  <span>Локальный прокси для приложений</span>
                  <select value={config.selected_proxy_id} disabled={sessionActive}
                    onChange={(event) => runAction("save", async () => { await invoke<AppConfig>("select_proxy", { id: event.target.value }); })}>
                    {config.proxies.map((proxy) => <option key={proxy.id} value={proxy.id}>{proxy.name} — {proxy.host}:{proxy.port}</option>)}
                  </select>
                </label>
                {sessionActive && <p className="pickerNote">Чтобы сменить сервер или прокси, отключите туннель.</p>}
              </div>

              <div className="metrics">
                <div className="metric">
                  <span>Внешний IP</span>
                  <strong className="withActions"><span className="mono">{connected ? visibleIp : "—"}</span>{connected && ipActions(true)}</strong>
                </div>
                <div className="metric">
                  <span>Страна выхода</span>
                  <strong title={country}>{connected && status.country_code ? <><Flag code={status.country_code} label={country} /> {country}</> : "—"}</strong>
                </div>
                <div className="metric">
                  <span>Скачивание сейчас</span>
                  <strong className="rx">{connected && latest ? formatBitRate(latest.rx_bps) : "—"}</strong>
                </div>
                <div className="metric">
                  <span>Отдача сейчас</span>
                  <strong className="tx">{connected && latest ? formatBitRate(latest.tx_bps) : "—"}</strong>
                </div>
                <div className="metric">
                  <span>Пинг до сервера</span>
                  <strong>{connected && latestPing !== null ? `${latestPing} мс` : "—"}</strong>
                </div>
                <div className="metric">
                  <span>За сеанс</span>
                  <strong className="totals"><span><ArrowDown size={12} />{formatBytes(status.rx_total)}</span><span><ArrowUp size={12} />{formatBytes(status.tx_total)}</span></strong>
                </div>
                <div className="metric">
                  <span>Локальный прокси</span>
                  <strong className="mono" title="Адрес SOCKS5 для браузера и приложений">{currentProxy.host}:{currentProxy.port}</strong>
                </div>
                <div className="metric">
                  <span>Движок</span>
                  <strong>{engineLabel(status.ssh_engine)}</strong>
                </div>
              </div>

              <TrafficChart points={traffic} />
            </section>
          )}

          {activeTab === "servers" && (
            <div className="split">
              <section className="panel listPanel">
                <header className="panelHead">
                  <h2>Серверы</h2>
                  <button className="ghost" onClick={() => { serverDraftDirtyRef.current = true; setServerDraft(emptyServer); }}><Plus size={15} />Добавить</button>
                </header>
                <div className="profileList" role="listbox" aria-label="SSH-серверы">
                  {config.servers.map((server) => {
                    const route = serverChain(server, config.servers);
                    const active = server.id === config.selected_server_id;
                    return (
                      <button key={server.id} role="option" aria-selected={server.id === serverDraft.id}
                        className={`profileItem${server.id === serverDraft.id ? " editing" : ""}`}
                        onClick={() => { serverDraftDirtyRef.current = true; setServerDraft(server); }}>
                        <span className="profileName">{server.name}{active && <em className="activeBadge"><Star size={11} />для подключения</em>}</span>
                        <span className="profileMeta">{server.user}@{showSensitive ? server.host : maskValue(server.host)}:{server.port}</span>
                        {route && route.length > 1 && <span className="profileMeta">через {route.slice(0, -1).join(" → ")}</span>}
                      </button>
                    );
                  })}
                  {!config.servers.length && <p className="emptyHint">Серверов пока нет. Нажмите «Добавить» и укажите адрес, пользователя и ключ SSH.</p>}
                </div>
              </section>

              <section className="panel">
                <header className="panelHead">
                  <h2>{serverDraft.name || "Новый сервер"}</h2>
                  {serverDraft.id && serverDraft.id === config.selected_server_id ? <span className="chip good">Используется для подключения</span>
                    : serverDraft.id && config.servers.some((server) => server.id === serverDraft.id) ? (
                      <button className="ghost" disabled={busy !== null || sessionActive} onClick={() => runAction("save", async () => { await invoke<AppConfig>("select_server", { id: serverDraft.id }); return `Подключение пойдёт через ${serverDraft.name}`; })}>
                        <Star size={15} />Использовать для подключения
                      </button>) : null}
                </header>
                <div className="formGrid">
                  <label><span>Название</span><input value={serverDraft.name} onChange={(event) => updateServer("name", event.target.value)} placeholder="Франция" /></label>
                  <label><span>ID</span><input value={serverDraft.id} onChange={(event) => updateServer("id", event.target.value)} placeholder="fr" /></label>
                  <label>
                    <span>Адрес сервера</span>
                    <div className="secretField">
                      <input type={showServerHost ? "text" : "password"} value={serverDraft.host} onChange={(event) => updateServer("host", event.target.value)} placeholder="example.com" />
                      <button type="button" className="iconButton" title={showServerHost ? "Скрыть адрес" : "Показать адрес"} aria-label={showServerHost ? "Скрыть адрес" : "Показать адрес"} onClick={() => setShowServerHost((value) => !value)}>
                        {showServerHost ? <EyeOff size={16} /> : <Eye size={16} />}
                      </button>
                      <button type="button" className="iconButton" title="Скопировать адрес" aria-label="Скопировать адрес" disabled={!serverDraft.host} onClick={() => copyText(serverDraft.host, "server-host")}>
                        {copiedField === "server-host" ? <Check size={16} /> : <Copy size={16} />}
                      </button>
                    </div>
                  </label>
                  <label><span>Порт SSH</span><input type="number" value={serverDraft.port} onChange={(event) => updateServer("port", Number(event.target.value))} /></label>
                  <label><span>Пользователь</span><input value={serverDraft.user} onChange={(event) => updateServer("user", event.target.value)} placeholder="user" /></label>
                  <label>
                    <span>Подключаться через</span>
                    <select value={serverDraft.jump_server_id ?? ""} onChange={(event) => updateServer("jump_server_id", event.target.value || null)}>
                      <option value="">Напрямую</option>
                      {config.servers.filter((server) => server.id !== serverDraft.id).map((server) => <option key={server.id} value={server.id}>{server.name}</option>)}
                    </select>
                  </label>
                  {serverDraft.jump_server_id && (
                    <p className="wide fieldNote">Маршрут: {serverChain(serverDraft, config.servers)?.join(" → ") ?? "проверьте цепочку SSH"}. Агент нужен только на последнем сервере.</p>
                  )}
                  <label className="wide">
                    <span>Приватный ключ SSH</span>
                    <div className="fileField">
                      <input value={serverDraft.key_path} onChange={(event) => updateServer("key_path", event.target.value)} placeholder="~/.ssh/id_ed25519" />
                      <button type="button" className="iconButton" title="Выбрать файл ключа" aria-label="Выбрать файл ключа" onClick={choosePrivateKey}><FolderOpen size={16} /></button>
                    </div>
                  </label>
                </div>
                <details className="advanced">
                  <summary>Для опытных: проверка ключа хоста, таймауты</summary>
                  <div className="formGrid">
                    <label>
                      <SettingLabel hint="yes — подключаться только к серверу из known_hosts; accept-new — один раз запомнить новый ключ хоста; no — не проверять (небезопасно).">Проверка ключа хоста</SettingLabel>
                      <select value={serverDraft.strict_host_key_checking} onChange={(event) => updateServer("strict_host_key_checking", event.target.value)}>
                        <option value="yes">Строго (yes)</option>
                        <option value="accept-new">Запомнить новый (accept-new)</option>
                        <option value="no">Не проверять (no)</option>
                      </select>
                    </label>
                    <label><span>Таймаут подключения, с</span><input type="number" min={1} max={120} value={serverDraft.connect_timeout} onChange={(event) => updateServer("connect_timeout", Number(event.target.value))} /></label>
                    <label><span>Проверка связи каждые, с</span><input type="number" min={1} max={300} value={serverDraft.server_alive_interval} onChange={(event) => updateServer("server_alive_interval", Number(event.target.value))} /></label>
                    <label><span>Пропусков до разрыва</span><input type="number" min={1} max={20} value={serverDraft.server_alive_count_max} onChange={(event) => updateServer("server_alive_count_max", Number(event.target.value))} /></label>
                    <label className="checkboxLine wide"><input type="checkbox" checked={serverDraft.compression} onChange={(event) => updateServer("compression", event.target.checked)} />Сжатие SSH (только для системного OpenSSH)</label>
                  </div>
                </details>
                <div className="actions">
                  <button className="primary" disabled={busy !== null || !serverDraft.id} onClick={() => runAction("save", async () => {
                    const saved = serverDraft;
                    await invoke<AppConfig>("save_server", { server: saved });
                    serverDraftDirtyRef.current = false;
                    setServerDraft(saved);
                    return `Сервер «${saved.name}» сохранён`;
                  })}><Save size={15} />Сохранить</button>
                  <button disabled={busy !== null || !serverDraft.id} onClick={() => runAction("test", () => invoke<string>("test_server", { server: serverDraft }))}>
                    <Terminal size={15} />
                    {(serverDraft.jump_server_id || config.settings.ssh_engine === "embedded") && !config.servers.some((profile) => profile.jump_server_id === serverDraft.id)
                      ? "Проверить SSH и агент" : "Проверить SSH"}
                  </button>
                  <button className="quietDanger" disabled={busy !== null || !serverDraft.id || sessionActive} onClick={() => runAction("delete", async () => {
                    await invoke<AppConfig>("delete_server", { id: serverDraft.id });
                    serverDraftDirtyRef.current = false;
                    setServerDraft(emptyServer);
                    return "Сервер удалён";
                  })}><Trash2 size={15} />Удалить</button>
                </div>
              </section>
            </div>
          )}

          {activeTab === "proxies" && (
            <div className="split">
              <section className="panel listPanel">
                <header className="panelHead">
                  <h2>Локальные прокси</h2>
                  <button className="ghost" onClick={() => { proxyDraftDirtyRef.current = true; setProxyDraft(emptyProxy); }}><Plus size={15} />Добавить</button>
                </header>
                <p className="panelIntro">SOCKS5-адрес на этом компьютере. Его можно указать в браузере или программе, а перехват приложений использует его сам.</p>
                <div className="profileList" role="listbox" aria-label="Локальные прокси">
                  {config.proxies.map((proxy) => (
                    <button key={proxy.id} role="option" aria-selected={proxy.id === proxyDraft.id}
                      className={`profileItem${proxy.id === proxyDraft.id ? " editing" : ""}`}
                      onClick={() => { proxyDraftDirtyRef.current = true; setProxyDraft(proxy); }}>
                      <span className="profileName">{proxy.name}{proxy.id === config.selected_proxy_id && <em className="activeBadge"><Star size={11} />для подключения</em>}</span>
                      <span className="profileMeta mono">{proxy.host}:{proxy.port}</span>
                    </button>
                  ))}
                </div>
              </section>
              <section className="panel">
                <header className="panelHead">
                  <h2>{proxyDraft.name || "Новый прокси"}</h2>
                  {proxyDraft.id && proxyDraft.id === config.selected_proxy_id ? <span className="chip good">Используется для подключения</span>
                    : proxyDraft.id && config.proxies.some((proxy) => proxy.id === proxyDraft.id) ? (
                      <button className="ghost" disabled={busy !== null || sessionActive} onClick={() => runAction("save", async () => { await invoke<AppConfig>("select_proxy", { id: proxyDraft.id }); return `Туннель будет слушать ${proxyDraft.host}:${proxyDraft.port}`; })}>
                        <Star size={15} />Использовать для подключения
                      </button>) : null}
                </header>
                <div className="formGrid">
                  <label><span>Название</span><input value={proxyDraft.name} onChange={(event) => updateProxy("name", event.target.value)} placeholder="Local 1080" /></label>
                  <label><span>ID</span><input value={proxyDraft.id} onChange={(event) => updateProxy("id", event.target.value)} placeholder="local-1080" /></label>
                  <label>
                    <SettingLabel hint="127.0.0.1 — прокси доступен только этому компьютеру. Другой адрес откроет его для сети, без пароля это небезопасно.">Адрес</SettingLabel>
                    <input value={proxyDraft.host} disabled={sessionActive} onChange={(event) => updateProxy("host", event.target.value)} placeholder="127.0.0.1" />
                  </label>
                  <label><span>Порт</span><input type="number" disabled={sessionActive} value={proxyDraft.port} onChange={(event) => updateProxy("port", Number(event.target.value))} /></label>
                </div>
                <div className="actions">
                  <button className="primary" disabled={busy !== null || sessionActive || !proxyDraft.id} onClick={() => runAction("save", async () => {
                    const saved = proxyDraft;
                    await invoke<AppConfig>("save_proxy", { proxy: saved });
                    proxyDraftDirtyRef.current = false;
                    setProxyDraft(saved);
                    return `Прокси «${saved.name}» сохранён`;
                  })}><Save size={15} />Сохранить</button>
                  <button className="quietDanger" disabled={busy !== null || !proxyDraft.id || sessionActive} onClick={() => runAction("delete", async () => {
                    await invoke<AppConfig>("delete_proxy", { id: proxyDraft.id });
                    proxyDraftDirtyRef.current = false;
                    setProxyDraft(emptyProxy);
                    return "Прокси удалён";
                  })}><Trash2 size={15} />Удалить</button>
                </div>
              </section>
            </div>
          )}

          {activeTab === "applications" && (
            <section className="panel appsPanel">
              <header className="panelHead">
                <div>
                  <h2>Приложения через туннель</h2>
                  <p className="panelIntro">Отмеченные программы идут через SSH-туннель целиком, по TCP и UDP. Остальные работают напрямую.</p>
                </div>
                <div className="appsControl">
                  <span className={redirect.running ? "chip good" : redirect.starting ? "chip busy" : "chip"}>
                    {redirect.starting ? "Ждём разрешения macOS" : redirect.running ? `Перехват включён · ${selectedApps.length}` : "Перехват выключен"}
                  </span>
                  {!redirectActive ? (
                    <button className="primary" disabled={busy !== null || !canRedirect || (selectedApps.length === 0 && !(settingsDraft.domain_rules || []).some((r) => r.enabled)) || (isMac && (settingsDraft.domain_rules || []).some((r) => r.enabled))}
                      title={!connected ? "Сначала подключите туннель" : undefined}
                      onClick={() => runAction("connect", async () => {
                        const settings = applicationSettings();
                        await invoke<AppConfig>("save_settings", { settings });
                        setSettingsDraft(settings);
                        settingsDirtyRef.current = false;
                        return invoke<string>("start_redirect");
                      })}><Play size={15} />Включить перехват</button>
                  ) : (
                    <button className="danger" disabled={busy !== null} onClick={() => runAction("disconnect", () => invoke<string>("stop_redirect"))}>
                      <CircleStop size={15} />{redirect.starting ? "Отменить" : "Выключить"}
                    </button>
                  )}
                </div>
              </header>

              <div className="notices">
                {followed.length > 0 && (
                  <div className="notice info followNotice">
                    <strong>Подхвачены на ходу, без перезапуска перехвата:</strong>
                    <span className="followList">
                      {followed.map((item) => <span key={item.rule} className="followChip" title={item.rule}><code>{item.process}</code> из {item.app}</span>)}
                    </span>
                  </div>
                )}
                {leaks.length > 0 && (
                  <div className="notice leakNotice">
                    <strong>Мимо туннеля идут программы, которые запустили выбранные приложения.</strong>
                    <span> macOS опознаёт их по собственной подписи, поэтому правило приложения на них не действует.</span>
                    <ul className="leakList">
                      {leaks.slice(0, 8).map((leak) => (
                        <li key={leak.rule} title={leak.path}><code>{leak.process}</code> из {leak.app} · {leak.flows} {leak.flows === 1 ? "соединение" : leak.flows < 5 ? "соединения" : "соединений"}</li>
                      ))}
                    </ul>
                    <button className="primary" disabled={busy !== null} onClick={() => runAction("save", async () => {
                      const settings = { ...applicationSettings() };
                      settings.redirect_processes = dedupePatterns([...settings.redirect_processes, ...leaks.map((leak) => leak.rule)]);
                      await invoke<AppConfig>("save_settings", { settings });
                      setSettingsDraft(settings);
                      settingsDirtyRef.current = false;
                      setLeaks([]);
                      await invoke<string>("stop_redirect");
                      await invoke<string>("start_redirect");
                      return `Через туннель пойдут ещё ${leaks.length} ${leaks.length === 1 ? "программа" : leaks.length < 5 ? "программы" : "программ"}. Перехват перезапущен`;
                    })}><Play size={15} />Направить через туннель</button>
                  </div>
                )}
                {!connected && redirectSupported && <div className="notice info">Перехват работает поверх подключённого туннеля. Подключитесь на вкладке «Подключение».</div>}
                {!redirectSupported && redirect.platform !== "unknown" && <div className="notice">Перехват приложений доступен на macOS и Windows.</div>}
                {redirect.requires_administrator && !redirect.administrator && <div className="notice">Для перехвата запустите Secret Tunnel от имени администратора.</div>}
                {!redirect.component_available && <div className="notice">{isMac ? <>Установите <a href="https://github.com/InterceptSuite/ProxyBridge/releases" target="_blank" rel="noreferrer">ProxyBridge 4.1.2 или новее</a> в «Программы» и разрешите его сетевое расширение в системных настройках. Окно ProxyBridge должно быть закрыто.</> : "Компонент перехвата не установлен. Укажите путь к нему в разделе «Дополнительно» ниже."}</div>}
                {isMac && status.ssh_engine !== "embedded" && <div className="notice">Системный OpenSSH передаёт только TCP, UDP выбранных приложений будет заблокирован. Для TCP и UDP выберите встроенный SSH в настройках.</div>}
                {!isMac && status.ssh_engine !== "embedded" && <div className="notice">Для TCP и UDP выберите встроенный SSH в настройках и переподключитесь.</div>}
                {isMac && (settingsDraft.domain_rules || []).some((r) => r.enabled) && <div className="notice">Отключите доменные правила: на macOS они пока не применяются.</div>}
                {isMac && settingsDraft.redirect_processes.some((id) => ["shipit", "chrome_crashpad_handler"].includes(id.toLowerCase())) && <div className="notice">ShipIt и chrome_crashpad_handler есть во многих Electron-приложениях: правило направит в туннель и их одноимённые утилиты.</div>}
                {isMac && redirectActive && redirect.setup_message && <div className="notice info" role="status">{redirect.setup_message}</div>}
                {redirect.last_error && <div className="notice bad">{redirect.last_error}</div>}
                {redirect.running && <div className="notice info">Перехват действует на новые соединения. Перезапустите выбранные программы, если они были открыты до включения.</div>}
              </div>

              <div className="appToolbar">
                <label className="search">
                  <Search size={15} />
                  <input aria-label="Поиск программы" placeholder="Найти программу" value={appSearch} onChange={(event) => setAppSearch(event.target.value)} />
                </label>
                <div className="segmented" role="group" aria-label="Показать">
                  <button className={!selectedAppsOnly ? "on" : ""} onClick={() => setSelectedAppsOnly(false)}>Все · {appChoices.length}</button>
                  <button className={selectedAppsOnly ? "on" : ""} onClick={() => setSelectedAppsOnly(true)}>Выбранные · {selectedApps.length}</button>
                </div>
                <button className="ghost" disabled={appsScanning} onClick={scanApplications} title="Найти установленные программы заново"><RefreshCcw size={15} />{appsScanning ? "Ищем…" : "Обновить"}</button>
                <button className="ghost" disabled={busy !== null} onClick={addApplicationFile}><Plus size={15} />Добавить {isMac ? ".app" : ".exe"}</button>
              </div>
              {appsError && <div className="notice bad">{appsError}</div>}

              <div className="appGrid" aria-label="Установленные программы" aria-busy={appsScanning}>
                {visibleApps.map((app) => {
                  const selected = isAppSelected(app);
                  return (
                    <label key={app.id} className={selected ? "appTile selected" : "appTile"} title={app.executable}>
                      <AppIcon app={app} icon={appIcons[app.executable]} />
                      <span className="appText">
                        <strong>{app.name}</strong>
                        <small>{appSubtitle(app)}</small>
                      </span>
                      <input type="checkbox" className="switch" checked={selected} disabled={busy !== null} onChange={(event) => toggleApplication(app, event.target.checked)}
                        aria-label={`Направлять ${app.name} через туннель`} />
                    </label>
                  );
                })}
                {!visibleApps.length && <p className="emptyHint">{appsScanning ? "Ищем установленные программы…" : selectedAppsOnly ? "Пока ничего не выбрано. Переключитесь на «Все» и отметьте программы." : `Ничего не найдено. Измените запрос или добавьте ${isMac ? ".app" : ".exe"} вручную.`}</p>}
              </div>

              <footer className="appsFooter">
                <details className="advanced">
                  <summary>Дополнительно: правила вручную и компонент перехвата</summary>
                  <label>
                    <span>{isMac ? "Bundle ID или идентификаторы подписи через запятую" : "Имена exe или полные пути через запятую"}</span>
                    <input disabled={redirectActive} value={settingsDraft.redirect_processes.join(", ")}
                      onChange={(event) => updateSettings("redirect_processes", event.target.value.split(",").map((name) => name.trim()))} />
                  </label>
                  <label>
                    <span>Компонент перехвата</span>
                    <div className="fileField">
                      <input disabled={redirectActive} value={settingsDraft.redirect_cli_path} onChange={(event) => updateSettings("redirect_cli_path", event.target.value)}
                        placeholder={isMac ? "/Applications/ProxyBridge.app/Contents/MacOS/ProxyBridge" : "Путь к ProxyBridge_CLI.exe"} />
                      <button className="iconButton" disabled={redirectActive} title="Выбрать компонент" aria-label="Выбрать компонент" onClick={async () => {
                        try {
                          const selected = await open({ multiple: false, ...(isMac ? {} : { filters: [{ name: "ProxyBridge CLI", extensions: ["exe"] }] }) });
                          if (typeof selected === "string") updateSettings("redirect_cli_path", selected);
                        } catch (error) { notify(String(error), true); }
                      }}><FolderOpen size={16} /></button>
                    </div>
                  </label>
                  <p className="fieldNote">{isMac ? "Программы сопоставляются по точным идентификаторам подписи: самого приложения, вложенных .app, .xpc, .appex и программ внутри него. DNS-запросы выбранных приложений идут через туннель на 1.1.1.1." : "Нужен проверенный комплект ProxyBridge с WinDivert. Системный DNS может идти напрямую."} При аварии перехвата прямой выход не блокируется.</p>
                </details>
                <button disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const settings = applicationSettings();
                  await invoke<AppConfig>("save_settings", { settings });
                  setSettingsDraft(settings);
                  settingsDirtyRef.current = false;
                  if (redirectActive) {
                    await invoke<string>("stop_redirect");
                    if (settings.redirect_processes.length || (settings.domain_rules || []).some((r) => r.enabled)) return invoke<string>("start_redirect");
                    return "Выбор очищен, перехват выключен";
                  }
                  return "Выбор приложений сохранён";
                })}><Save size={15} />{redirectActive ? "Применить выбор" : "Сохранить выбор"}</button>
              </footer>
            </section>
          )}

          {activeTab === "domains" && (
            <>
              <div className="notice" hidden={redirect.supports_domains}>Доменные правила применяются только в Windows. На macOS используйте выбор приложений и отключите ранее сохранённые доменные правила.</div>
              <DomainsPanel rules={settingsDraft.domain_rules || []} stats={redirect.domain_stats}
                running={redirect.running} busy={busy !== null} canStart={canRedirect && redirect.supports_domains}
                onChange={(rules) => updateSettings("domain_rules", rules)}
                onSave={() => runAction("save", async () => {
                  await invoke<AppConfig>("save_settings", { settings: settingsDraft });
                  settingsDirtyRef.current = false;
                  if (redirect.running) {
                    await invoke<string>("stop_redirect");
                    return invoke<string>("start_redirect");
                  }
                  return "Правила доменов сохранены. Включите перехват, чтобы применить их";
                })}
                onStart={() => runAction("connect", async () => {
                  await invoke<AppConfig>("save_settings", { settings: settingsDraft });
                  settingsDirtyRef.current = false;
                  return invoke<string>("start_redirect");
                })}
                onStop={() => runAction("disconnect", () => invoke<string>("stop_redirect"))} />
            </>
          )}

          {activeTab === "speed" && (
            <SpeedPanel connected={connected} live={speedLive} history={speedHistory} serverPing={connected ? latestPing : null}
              tunnelRoute={connected && status.ssh_route?.length ? status.ssh_route : chain.length > 1 ? chain : []} tunnelServer={currentServer.name}
              tunnelCountry={connected ? status.country_code : ""} hideAddresses={config.settings.hide_sensitive}
              onStart={(kind, direct, provider) => invoke("speedtest_start", { kind, direct, provider }).then(() => invoke<SpeedLive>("speedtest_status").then(setSpeedLive)).catch((error) => setMessage({ text: String(error), error: true }))}
              onCancel={() => invoke("speedtest_cancel").catch(() => undefined)}
              onClear={() => invoke<SpeedRun[]>("speedtest_clear").then(setSpeedHistory).catch(() => undefined)} />
          )}

          {activeTab === "stats" && (
            <StatsPanel sessions={sessions}
              known={statsKnownApps.map((app) => ({ name: app.name, executable: app.executable, icon: appIcons[app.executable], patterns: app.patterns }))}
              onClear={() => runAction("save", async () => {
                setSessions(await invoke<SessionRecord[]>("clear_session_history"));
                return "История сеансов очищена";
              })} />
          )}

          {activeTab === "settings" && (
            <section className="panel settingsPanel">
              <header className="panelHead"><h2>Настройки</h2></header>

              <fieldset>
                <legend>Подключение</legend>
                <label>
                  <SettingLabel hint="Встроенный SSH работает внутри приложения, передаёт TCP и UDP и умеет цепочки серверов. Системный OpenSSH использует ssh из системы и передаёт только TCP.">Движок SSH</SettingLabel>
                  <select value={settingsDraft.ssh_engine} onChange={(event) => updateSettings("ssh_engine", event.target.value)}>
                    <option value="embedded">Встроенный SSH — TCP и UDP (рекомендуется)</option>
                    <option value="system">Системный OpenSSH — только TCP</option>
                  </select>
                </label>
                {isMac && <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.bypass_vpn_for_ssh ?? true} onChange={(event) => updateSettings("bypass_vpn_for_ssh", event.target.checked)} />
                  <SettingLabel hint="SSH-соединение к серверу идёт напрямую через Wi-Fi или Ethernet, даже если включён другой VPN (Clash, v2RayTun). Иначе туннель окажется внутри чужого VPN: двойное шифрование и ниже скорость. Выключите, если сервер доступен только через тот VPN.">SSH мимо других VPN</SettingLabel></label>}
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.close_to_tray ?? true} onChange={(event) => updateSettings("close_to_tray", event.target.checked)} />
                  <SettingLabel hint={`Закрытие окна прячет приложение ${isMac ? "в строку меню" : "в трей"}, туннель продолжает работать. Подключиться, отключиться и выйти можно из меню значка.`}>{isMac ? "Сворачивать в строку меню при закрытии окна" : "Сворачивать в трей при закрытии окна"}</SettingLabel></label>
                {isMac && <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.follow_child_processes ?? true} onChange={(event) => updateSettings("follow_child_processes", event.target.checked)} />
                  <SettingLabel hint="Программы, которые запускают выбранные приложения (node, python, MCP-серверы, программы обновления), macOS опознаёт по их собственной подписи. Перехват замечает их за пару секунд и добавляет правило, не обрывая соединения. Командные оболочки и мелкие утилиты не добавляются.">Подхватывать программы, запущенные выбранными приложениями</SettingLabel></label>}
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.auto_reconnect} onChange={(event) => updateSettings("auto_reconnect", event.target.checked)} />
                  <SettingLabel hint="Если соединение оборвалось, приложение подключится снова с выбранным сервером и прокси.">Переподключаться автоматически</SettingLabel></label>
                <label className="inlineNumber">
                  <span>Пауза перед переподключением, с</span>
                  <input type="number" min={1} max={120} value={settingsDraft.reconnect_delay_seconds} onChange={(event) => updateSettings("reconnect_delay_seconds", Number(event.target.value))} />
                </label>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.start_tunnel_on_launch} onChange={(event) => updateSettings("start_tunnel_on_launch", event.target.checked)} />
                  <SettingLabel hint="При открытии приложения сразу подключает выбранный сервер. Включайте, когда туннель уже проверен.">Подключаться при запуске приложения</SettingLabel></label>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.verify_proxy_on_start} onChange={(event) => updateSettings("verify_proxy_on_start", event.target.checked)} />
                  <SettingLabel hint="После подключения делает пробный запрос через прокси, чтобы сразу увидеть, что трафик проходит.">Проверять прокси после подключения</SettingLabel></label>
              </fieldset>

              <fieldset>
                <legend>Конфиденциальность</legend>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.hide_sensitive} onChange={(event) => updateSettings("hide_sensitive", event.target.checked)} />
                  <SettingLabel hint="IP-адреса и токены скрыты, пока не нажать на глаз. Удобно для скриншотов и демонстраций.">Скрывать IP-адреса и токены</SettingLabel></label>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.clear_logs_on_start} onChange={(event) => updateSettings("clear_logs_on_start", event.target.checked)} />
                  <SettingLabel hint="Очищает журнал перед каждым подключением.">Очищать журнал при подключении</SettingLabel></label>
                <label>
                  <SettingLabel hint="Токен IPinfo Lite нужен, чтобы показывать внешний IP и страну выхода. Можно оставить пустым.">Токен IPinfo</SettingLabel>
                  <div className="secretField">
                    <input type={showIpinfoToken ? "text" : "password"} value={settingsDraft.ipinfo_token} onChange={(event) => updateSettings("ipinfo_token", event.target.value)} placeholder="Токен IPinfo Lite" />
                    <button type="button" className="iconButton" title={showIpinfoToken ? "Скрыть токен" : "Показать токен"} aria-label={showIpinfoToken ? "Скрыть токен" : "Показать токен"} onClick={() => setShowIpinfoToken((value) => !value)}>
                      {showIpinfoToken ? <EyeOff size={16} /> : <Eye size={16} />}
                    </button>
                    <button type="button" className="iconButton" title="Скопировать токен" aria-label="Скопировать токен" disabled={!settingsDraft.ipinfo_token} onClick={() => copyText(settingsDraft.ipinfo_token, "ipinfo-token")}>
                      {copiedField === "ipinfo-token" ? <Check size={16} /> : <Copy size={16} />}
                    </button>
                  </div>
                </label>
              </fieldset>

              <fieldset>
                <legend>Для опытных: встроенный SSH</legend>
                <p className="fieldNote">Применяется при следующем подключении ко всем серверам цепочки.</p>
                <label>
                  <span>Шифрование</span>
                  <select value={settingsDraft.ssh_tuning?.cipher ?? "auto"} onChange={(event) => updateSettings("ssh_tuning", { ...defaultSshTuning, ...settingsDraft.ssh_tuning, cipher: event.target.value as SshTuning["cipher"] })}>
                    <option value="auto">Авто</option>
                    <option value="aes128_gcm">AES-128-GCM</option>
                    <option value="chacha20_poly1305">ChaCha20-Poly1305</option>
                  </select>
                </label>
                <label>
                  <SettingLabel hint="Стандартная очередь быстрее на дальних маршрутах. Уменьшенная нужна для сравнения задержек и может ограничить скорость.">Очередь SSH</SettingLabel>
                  <select value={settingsDraft.ssh_tuning?.channel_buffer_messages === 8 ? "small" : "default"} onChange={(event) => updateSettings("ssh_tuning", { ...defaultSshTuning, ...settingsDraft.ssh_tuning, channel_buffer_messages: event.target.value === "small" ? 8 : 32, channel_receive_window_bytes: event.target.value === "small" ? 1048576 : 4194304 })}>
                    <option value="default">Стандартная · 4 МиБ</option>
                    <option value="small">Уменьшенная · 1 МиБ · для тестов</option>
                  </select>
                </label>
                <label>
                  <SettingLabel hint="Во время звонков туннель придерживает загрузки, чтобы голос не прерывался. «Голос» придерживает сильнее, «Максимальная скорость» не придерживает и отключает контроль очереди.">Приоритет звонков</SettingLabel>
                  <select value={settingsDraft.ssh_tuning?.tcp_interactive_bytes === undefined ? balancedTcpBudget : settingsDraft.ssh_tuning.tcp_interactive_bytes ?? 0} onChange={(event) => updateSettings("ssh_tuning", { ...defaultSshTuning, ...settingsDraft.ssh_tuning, tcp_interactive_bytes: Number(event.target.value) || null })}>
                    <option value={balancedTcpBudget}>Сбалансированный (рекомендуется)</option>
                    <option value={65536}>Голос</option>
                    <option value={0}>Максимальная скорость</option>
                  </select>
                </label>
              </fieldset>

              <div className="actions">
                <button className="primary" disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const nextConfig = await invoke<AppConfig>("save_settings", { settings: settingsDraft });
                  settingsDirtyRef.current = false;
                  setConfig(nextConfig);
                  setSettingsDraft(nextConfig.settings);
                  return "Настройки сохранены";
                })}><Save size={15} />Сохранить настройки</button>
                <button className="ghost" disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const settings = await invoke<AppSettings>("default_settings");
                  const nextConfig = await invoke<AppConfig>("save_settings", { settings });
                  settingsDirtyRef.current = false;
                  setConfig(nextConfig);
                  setSettingsDraft(nextConfig.settings);
                  return "Настройки сброшены";
                })}><RefreshCcw size={15} />Вернуть по умолчанию</button>
              </div>
            </section>
          )}
        </div>

        <LogDock logs={logs} busy={busy !== null} hideAddresses={config.settings.hide_sensitive}
          onClear={() => runAction("logs", async () => { await invoke<void>("clear_logs"); return "Журнал очищен"; })}
          onCopied={(text) => copyText(text, "logs")} />
      </section>
    </main>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
