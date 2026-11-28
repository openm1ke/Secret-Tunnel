import { t, translateMessage, formatLocale, getLanguage, useLanguage } from "./i18n";
import DomainsPanel from "./DomainsPanel";
import SetupWizard from "./SetupWizard";
import HostingCatalog from "./HostingCatalog";
import ProfileTransfer from "./ProfileTransfer";
import StatsPanel, { SessionRecord, knownAppMatcher } from "./StatsPanel";
import SpeedPanel, { SpeedLive, SpeedRun, emptyLive } from "./SpeedPanel";
import AboutPanel, { Release, UpdateDownload } from "./AboutPanel";
import ConnectionHealth, { ConnectionChecks } from "./ConnectionHealth";
import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
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
  Info,
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
  maskAddresses,
  countryName,
  formatBitRate,
  formatBytes,
} from "./components";
import "./styles.css";

type Appearance = "system" | "light" | "dark";
const appearanceKey = "secret-tunnel.appearance";

function readAppearance(): Appearance {
  try {
    const saved = localStorage.getItem(appearanceKey);
    if (saved === "light" || saved === "dark") return saved;
  } catch { /* use the system appearance when storage is unavailable */ }
  return "system";
}

type ServerProfile = {
  id: string;
  name: string;
  host: string;
  port: number;
  user: string;
  key_path: string;
  host_key_sha256?: string | null;
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
const voiceTcpBudget = 64 * 1024;
const defaultSshTuning: SshTuning = { cipher: "auto", channel_buffer_messages: 32, channel_receive_window_bytes: 4194304, tcp_interactive_bytes: balancedTcpBudget };

type AppSettings = {
  redirect_cli_path: string;
  redirect_processes: string[];
  domain_rules: DomainRule[];
  ipinfo_token: string;
  ssh_engine: string;
  ssh_tuning?: SshTuning | null;
  priority_udp_ssh?: boolean;
  priority_udp_processes?: string[];
  hide_sensitive: boolean;
  verify_proxy_on_start: boolean;
  auto_reconnect: boolean;
  reconnect_delay_seconds: number;
  start_tunnel_on_launch: boolean;
  clear_logs_on_start: boolean;
  bypass_vpn_for_ssh?: boolean;
  follow_child_processes?: boolean;
  close_to_tray?: boolean;
  check_updates?: boolean;
  udp_receive_window?: UdpWindow | null;
  agent_udp_receive_window?: UdpWindow | null;
  app_added_at?: Record<string, number>;
};

type UdpWindow = { bytes: number; slots: number };
// The same choices as on Android, so a profile reads the same on both.
const udpPresets: { id: string; label: string; value: UdpWindow | null }[] = [
  { id: "auto", get label() { return t("По умолчанию"); }, value: null },
  { id: "256", get label() { return t("256 КиБ · 192 слота"); }, value: { bytes: 262144, slots: 192 } },
  { id: "1024", get label() { return t("1 МиБ · 512 слотов"); }, value: { bytes: 1048576, slots: 512 } },
];

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
  checks: ConnectionChecks;
  priority_udp_ssh?: { separate: boolean; error: string | null } | null;
};

/** A program started by a selected application that went around the tunnel. */
type Leak = { app: string; process: string; path: string; rule: string; flows: number };
/** A program a selected application started that was given a rule while running. */
type Followed = { app: string; process: string; rule: string };
type Tab = "connect" | "servers" | "catalog" | "proxies" | "applications" | "domains" | "stats" | "speed" | "settings" | "about";
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
type BusyAction = "connect" | "disconnect" | "restart" | "save" | "delete" | "logs" | null;
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
  checks: { running: false, report: null },
};

// Backend messages that are still in English, as people read them here.
const messageText: Record<string, string> = {
  get "Tunnel startup requested"() { return t("Запускаем туннель…"); },
  get "Tunnel stop requested"() { return t("Останавливаем туннель…"); },
  get "Tunnel restart requested"() { return t("Перезапускаем туннель…"); },
  get "TCP and SSH key authentication OK"() { return t("SSH и ключ: OK."); },
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

const TRANSLIT: Record<string, string> = {
  а: "a", б: "b", в: "v", г: "g", д: "d", е: "e", ё: "e", ж: "zh", з: "z", и: "i", й: "y", к: "k", л: "l", м: "m",
  н: "n", о: "o", п: "p", р: "r", с: "s", т: "t", у: "u", ф: "f", х: "h", ц: "ts", ч: "ch", ш: "sh", щ: "sch",
  ъ: "", ы: "y", ь: "", э: "e", ю: "yu", я: "ya",
};

// Russian names are the norm here, so they are transliterated: an empty ID
// would leave «Сохранить» disabled.
function UdpWindowField({ title, hint, value, onChange }: { title: string; hint: string; value?: UdpWindow | null; onChange: (value: UdpWindow | null) => void }) {
  const preset = !value ? "auto" : udpPresets.find((item) => item.value?.bytes === value.bytes && item.value?.slots === value.slots)?.id ?? "manual";
  return (
    <>
      <label>
        <SettingLabel hint={hint}>{title}</SettingLabel>
        <select value={preset} onChange={(event) => {
          const id = event.target.value;
          onChange(id === "manual" ? { bytes: 524288, slots: 256 } : udpPresets.find((item) => item.id === id)?.value ?? null);
        }}>
          {udpPresets.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
          <option value="manual">{t("Вручную")}</option>
        </select>
      </label>
      {preset === "manual" && value && (
        <div className="udpManual">
          <label className="inlineNumber"><span>{t("Окно, КиБ (64–1024)")}</span>
            <input type="number" min={64} max={1024} value={value.bytes / 1024} onChange={(event) => onChange({ ...value, bytes: Math.round(Number(event.target.value)) * 1024 })} /></label>
          <label className="inlineNumber"><span>{t("Слоты (4–512)")}</span>
            <input type="number" min={4} max={512} value={value.slots} onChange={(event) => onChange({ ...value, slots: Math.round(Number(event.target.value)) })} /></label>
        </div>
      )}
    </>
  );
}

function makeId(name: string) {
  return [...name.trim().toLowerCase()].map((char) => TRANSLIT[char] ?? char).join("")
    .replace(/[^a-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
}

// The ID follows the name while it is still the one derived from it and the
// profile is unsaved; a typed ID or a saved one stays as it is.
function nextId(key: string, value: unknown, current: { id: string; name: string }, saved: { id: string }[]) {
  if (key === "id") return String(value);
  const derived = !current.id || current.id === makeId(current.name);
  if (key === "name" && derived && !saved.some((profile) => profile.id === current.id)) return makeId(String(value));
  return current.id;
}

function phaseOf(status: TunnelStatus): Phase {
  if (status.state === "running") return "up";
  if (["starting", "reconnecting", "stopping", "unknown"].includes(status.state)) return "busy";
  if (status.state === "failed" || status.last_error) return "error";
  return "down";
}

function stateTitle(state: string) {
  switch (state) {
    case "running": return t("Подключено");
    case "starting": return t("Подключаемся");
    case "reconnecting": return t("Переподключаемся");
    case "stopping": return t("Отключаемся");
    case "failed": return t("Не удалось подключиться");
    case "unknown": return t("Проверяем");
    default: return t("Отключено");
  }
}

function engineLabel(engine: string) {
  return engine === "embedded" ? t("Встроенный SSH") : t("Системный OpenSSH");
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
  if (hours) return t("{0} ч {1} мин", hours, minutes);
  if (minutes) return t("{0} мин", minutes);
  return t("{0} с", seconds);
}

function maskValue(value: string) {
  return value ? "••••••••" : "—";
}

function dedupePatterns(patterns: string[]) {
  return [...new Map(patterns.filter((pattern) => pattern.trim()).map((pattern) => [normalizeAppPattern(pattern), pattern.trim()])).values()];
}

function plural(count: number, one: string, few: string, many: string) {
  if (getLanguage() === "en") return count === 1 ? one : few;
  const mod10 = count % 10;
  const mod100 = count % 100;
  if (mod10 === 1 && mod100 !== 11) return one;
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return few;
  return many;
}

type AppView = "tiles" | "table" | "cards";
const appViews: { id: AppView; title: string; hint: string }[] = [
  { id: "tiles", get title() { return t("Плитки"); }, get hint() { return t("Компактные плитки: значок, название, папка"); } },
  { id: "table", get title() { return t("Таблица"); }, get hint() { return t("Путь, число процессов, трафик и дата включения в одну строку"); } },
  { id: "cards", get title() { return t("Карточки"); }, get hint() { return t("Крупные карточки с путём, трафиком за 30 дней и датой включения"); } },
];
const formatDay = (epoch: number) => new Date(epoch * 1000).toLocaleDateString(formatLocale(), { day: "numeric", month: "short", year: "numeric" });

function appSubtitle(app: InstalledApp) {
  if (app.source === "manual") return t("добавлено вручную");
  const helpers = app.patterns.length - 1;
  const place = app.executable.startsWith("/System/") ? t("Системное") : app.executable.startsWith("/Applications/") ? t("Программы") : app.executable.replace(/[\\/][^\\/]+$/, "");
  return helpers > 0 ? `${place} · +${helpers} ${plural(helpers, t("процесс"), t("процесса"), t("процессов"))}` : place;
}

function AppIcon({ app, icon }: { app: InstalledApp; icon?: string }) {
  if (icon) return <img className="appIcon" src={icon} alt="" />;
  return <span className="appIcon letter" aria-hidden="true">{app.name.slice(0, 1).toUpperCase()}</span>;
}

function App() {
  const [language, changeLanguage] = useLanguage();
  useEffect(() => {
    document.documentElement.lang = language;
    invoke("set_ui_language", { language }).catch(() => undefined);
  }, [language]);
  const [activeTab, setActiveTab] = useState<Tab>("connect");
  const [appearance, setAppearance] = useState<Appearance>(readAppearance);

  useLayoutEffect(() => {
    const system = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme = appearance === "system" ? (system.matches ? "dark" : "light") : appearance;
    };
    apply();
    system.addEventListener("change", apply);
    return () => system.removeEventListener("change", apply);
  }, [appearance]);

  useEffect(() => {
    try {
      localStorage.setItem(appearanceKey, appearance);
    } catch { /* the appearance still applies for this session */ }
    // Web previews don't have a native title bar.
    try {
      getCurrentWindow().setTheme(appearance === "system" ? null : appearance).catch(() => undefined);
    } catch { /* Tauri window metadata is unavailable */ }
  }, [appearance]);

  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [status, setStatus] = useState<TunnelStatus>(emptyStatus);
  const [serverDraft, setServerDraft] = useState<ServerProfile>(emptyServer);
  const [proxyDraft, setProxyDraft] = useState<ProxyProfile>(emptyProxy);
  const [settingsDraft, setSettingsDraft] = useState<AppSettings>(emptyConfig.settings);
  const [traffic, setTraffic] = useState<TrafficPoint[]>([]);
  const [logs, setLogs] = useState<string[]>([]);
  const [redirect, setRedirect] = useState<RedirectStatus>({ running: false, starting: false, platform: "unknown", requires_administrator: true, supports_system_ssh: false, supports_domains: false, setup_message: "", pid: null, administrator: false, component_available: false, last_error: "" });
  const [scannedApps, setScannedApps] = useState<InstalledApp[]>([]);
  const [addedApps, setAddedApps] = useState<InstalledApp[]>([]);
  const [appIcons, setAppIcons] = useState<Record<string, string>>({});
  const [appsScanning, setAppsScanning] = useState(false);
  const [appsError, setAppsError] = useState("");
  const [appSearch, setAppSearch] = useState("");
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [speedLive, setSpeedLive] = useState<SpeedLive>(emptyLive);
  const [speedHistory, setSpeedHistory] = useState<SpeedRun[]>([]);
  const [leaks, setLeaks] = useState<Leak[]>([]);
  const [followed, setFollowed] = useState<Followed[]>([]);
  const [selectedAppsOnly, setSelectedAppsOnly] = useState(false);
  const [appView, setAppView] = useState<AppView>(() => {
    try { const saved = localStorage.getItem("secret-tunnel.app-view"); return saved === "table" || saved === "cards" ? saved : "tiles"; } catch { return "tiles"; }
  });
  const [appMenu, setAppMenu] = useState<{ app: InstalledApp; x: number; y: number } | null>(null);
  const appMenuRef = useRef<HTMLDivElement>(null);
  const [appTip, setAppTip] = useState<{ app: InstalledApp; x: number; y: number } | null>(null);
  const appTipTimer = useRef<number>();
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const [release, setRelease] = useState<Release | null>(null);
  const [updatePrompt, setUpdatePrompt] = useState<Release | null>(null);
  const [serverCheck, setServerCheck] = useState<{ id: string; state: "running" | "ok" | "error"; text: string; at: number } | null>(null);
  const [busy, setBusy] = useState<BusyAction>(null);
  const [showServerHost, setShowServerHost] = useState(false);
  const [setupOpen, setSetupOpen] = useState(false);
  const [setupPurchased, setSetupPurchased] = useState(false);
  const [transfer, setTransfer] = useState<{ serverId: string | null } | null>(null);
  const [showIpinfoToken, setShowIpinfoToken] = useState(false);
  const [copiedField, setCopiedField] = useState<CopyField>(null);
  const [now, setNow] = useState(Date.now());
  const serverDraftDirtyRef = useRef(false);
  const proxyDraftDirtyRef = useRef(false);
  const settingsDirtyRef = useRef(false);
  const refreshInFlightRef = useRef(false);
  const appScanInFlightRef = useRef(false);
  const appsLoadedRef = useRef(false);
  const iconRequestsRef = useRef(new Set<string>());

  const installedApps = useMemo(() => {
    const scannedIds = new Set(scannedApps.map((app) => app.id));
    return [...scannedApps, ...addedApps.filter((app) => !scannedIds.has(app.id))];
  }, [scannedApps, addedApps]);
  const currentServer = useMemo(() => selectedServer(config), [config]);
  const currentProxy = useMemo(() => selectedProxy(config), [config]);
  const phase = phaseOf(status);
  const connected = status.state === "running";
  const sessionActive = ["running", "starting", "reconnecting", "stopping"].includes(status.state);
  const isMac = redirect.platform === "macos";
  const callPriority = settingsDraft.ssh_tuning?.tcp_interactive_bytes;
  const embeddedForProfile = settingsDraft.ssh_engine === "embedded" || Boolean(currentServer.jump_server_id);
  // Before the first status arrives the browser's guess names the device.
  const deviceLabel = isMac || (redirect.platform === "unknown" && navigator.userAgent.includes("Mac")) ? t("Этот Mac")
    : redirect.platform === "windows" || navigator.userAgent.includes("Windows") ? t("Этот ПК") : t("Этот компьютер");
  const redirectActive = redirect.running || redirect.starting;
  const redirectSupported = isMac || redirect.platform === "windows";
  const showPriorityChoices = redirectSupported && embeddedForProfile && settingsDraft.priority_udp_ssh
    && callPriority === voiceTcpBudget;
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
  const priorityPatterns = new Set((settingsDraft.priority_udp_processes ?? []).map(normalizeAppPattern));
  const isPriorityApp = (app: InstalledApp) => appOwnKeys(app).some((pattern) => priorityPatterns.has(pattern));
  const domainRulesOn = (settingsDraft.domain_rules || []).some((rule) => rule.enabled);
  const canStartInterception = busy === null && canRedirect && (selectedApps.length > 0 || domainRulesOn) && !(isMac && domainRulesOn);
  const emptyAppsHint = appsScanning ? t("Ищем установленные программы…") : selectedAppsOnly ? t("Пока ничего не выбрано. Переключитесь на «Все» и отметьте программы.") : t("Ничего не найдено. Измените запрос или добавьте {0} вручную.", isMac ? ".app" : ".exe");
  // Traffic of each listed program over 30 days, matched the way the statistics match it.
  const appTraffic = useMemo(() => {
    const match = knownAppMatcher(appChoices);
    const from = Math.floor(Date.now() / 1000) - 30 * 86400;
    const totals = new Map<string, { rx: number; tx: number; sessions: number }>();
    for (const session of sessions) {
      if (session.ended_at < from && !session.active) continue;
      const seen = new Set<string>();
      for (const usage of session.apps) {
        const app = match(usage.name);
        if (!app) continue;
        const entry = totals.get(app.id) ?? { rx: 0, tx: 0, sessions: 0 };
        entry.rx += usage.rx; entry.tx += usage.tx;
        if (!seen.has(app.id)) { entry.sessions += 1; seen.add(app.id); }
        totals.set(app.id, entry);
      }
    }
    return totals;
  }, [sessions, appChoices]);
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
    { label: deviceLabel, kind: "device" },
    ...routeNames.filter(Boolean).map((name, index, all) => ({ label: name, kind: index === all.length - 1 ? "exit" as const : "hop" as const })),
    { label: t("Интернет"), kind: "internet", flag: connected ? status.country_code : undefined },
  ];

  function notify(text: string, error = false) {
    setMessage({ text: messageText[text] ?? text, error });
  }

  useEffect(() => {
    if (!message || message.error) return;
    const timer = window.setTimeout(() => setMessage(null), 4500);
    return () => window.clearTimeout(timer);
  }, [message]);

  const scanApplications = useCallback(async (manual = false) => {
    if (appScanInFlightRef.current) {
      if (manual) setAppsScanning(true);
      return;
    }
    appScanInFlightRef.current = true;
    if (manual || !appsLoadedRef.current) setAppsScanning(true);
    try {
      setScannedApps(await invoke<InstalledApp[]>("scan_installed_applications"));
      setAppsError("");
    } catch (error) {
      setAppsError(String(error));
    } finally {
      appScanInFlightRef.current = false;
      appsLoadedRef.current = true;
      setAppsScanning(false);
    }
  }, []);

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

  function markAdded(app: InstalledApp, added: boolean) {
    const dates = { ...(settingsDraft.app_added_at ?? {}) };
    if (added) dates[app.id] = Math.floor(Date.now() / 1000); else delete dates[app.id];
    updateSettings("app_added_at", dates);
  }

  function toggleApplication(app: InstalledApp, checked: boolean) {
    const current = settingsDraft.redirect_processes;
    markAdded(app, checked);
    if (checked) {
      updateSettings("redirect_processes", dedupePatterns([...current, ...app.patterns]));
      return;
    }
    // Keep helpers another selected application still needs.
    const stillNeeded = new Set(appChoices.filter((other) => other.id !== app.id && isAppSelected(other)).flatMap(appAliases));
    const remove = new Set([...appAliases(app).filter((pattern) => !stillNeeded.has(pattern)), ...appOwnKeys(app)]);
    updateSettings("redirect_processes", current.filter((pattern) => !remove.has(normalizeAppPattern(pattern))));
    if (isPriorityApp(app)) togglePriorityApplication(app, false);
  }

  function togglePriorityApplication(app: InstalledApp, checked: boolean) {
    const current = settingsDraft.priority_udp_processes ?? [];
    const own = new Set(appOwnKeys(app));
    const primaryKeys = app.patterns.filter((pattern) => own.has(normalizeAppPattern(pattern)));
    updateSettings("priority_udp_processes", checked
      ? dedupePatterns([...current, ...primaryKeys])
      : current.filter((pattern) => !own.has(normalizeAppPattern(pattern))));
  }

  function priorityCheckbox(app: InstalledApp) {
    return <label className="checkboxLine appPriority">
      <input type="checkbox" checked={isPriorityApp(app)} disabled={busy !== null}
        aria-label={t("UDP через второй SSH: {0}", app.name)}
        onChange={(event) => togglePriorityApplication(app, event.target.checked)} />
      <span>{t("UDP через второй SSH")}</span>
    </label>;
  }

  function chooseAppView(view: AppView) {
    setAppView(view);
    try { localStorage.setItem("secret-tunnel.app-view", view); } catch { /* the choice just is not remembered */ }
  }

  // The full path shows after a short hover, above the list so scrolling does not clip it.
  function showAppTip(event: React.MouseEvent, app: InstalledApp) {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    window.clearTimeout(appTipTimer.current);
    appTipTimer.current = window.setTimeout(() => {
      const below = box.bottom + 6 + 96 < window.innerHeight;
      setAppTip({ app, x: Math.min(box.left, window.innerWidth - 440), y: below ? box.bottom + 6 : box.top - 102 });
    }, 450);
  }

  function hideAppTip() {
    window.clearTimeout(appTipTimer.current);
    setAppTip(null);
  }

  function openAppMenu(event: React.MouseEvent, app: InstalledApp) {
    hideAppTip();
    event.preventDefault();
    setAppMenu({ app, x: Math.min(event.clientX, window.innerWidth - 260), y: Math.min(event.clientY, window.innerHeight - 280) });
  }

  // The keyboard's menu key and Shift+F10 open the same menu.
  function appMenuKey(event: React.KeyboardEvent, app: InstalledApp) {
    if (event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
    event.preventDefault();
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    setAppMenu({ app, x: Math.min(box.left + 24, window.innerWidth - 260), y: Math.min(box.bottom - 8, window.innerHeight - 280) });
  }

  function appMenuItems(app: InstalledApp): ({ label: string; run: () => void; disabled?: boolean; danger?: boolean } | "-")[] {
    const selected = isAppSelected(app);
    const fileMissing = app.source === "manual" && !/^([A-Za-z]:[\\/]|\/)/.test(app.executable);
    return [
      { label: isMac ? t("Показать в Finder") : t("Показать в проводнике"), disabled: fileMissing,
        run: () => invoke("reveal_path", { path: app.executable }).catch((error) => notify(String(error), true)) },
      { label: t("Запустить"), disabled: fileMissing,
        run: () => invoke("launch_application", { path: app.executable }).then(() => notify(t("{0} запускается", app.name))).catch((error) => notify(String(error), true)) },
      { label: t("Копировать путь"), run: () => copyText(app.executable, "logs").then(() => notify(t("Путь скопирован"))) },
      "-",
      { label: selected ? t("Не направлять через туннель") : t("Направлять через туннель"), disabled: busy !== null, run: () => toggleApplication(app, !selected) },
      { label: t("Только эта программа через туннель"), disabled: busy !== null, run: () => {
        const others = new Set(appChoices.filter((other) => other.id !== app.id).flatMap(appAliases));
        const own = new Set(appAliases(app));
        updateSettings("redirect_processes", dedupePatterns([...settingsDraft.redirect_processes.filter((pattern) => own.has(normalizeAppPattern(pattern)) || !others.has(normalizeAppPattern(pattern))), ...app.patterns]));
        updateSettings("app_added_at", { [app.id]: settingsDraft.app_added_at?.[app.id] ?? Math.floor(Date.now() / 1000) });
      } },
      { label: t("Трафик в статистике"), run: () => setActiveTab("stats") },
      ...(app.source === "manual" ? ["-" as const, { label: t("Убрать из списка"), danger: true, run: () => {
        toggleApplication(app, false);
        setAddedApps((current) => current.filter((other) => other.id !== app.id));
      } }] : []),
    ];
  }

  async function addApplicationFile() {
    try {
      const chosen = await open({ multiple: true, filters: [{ name: isMac ? t("Приложения macOS") : t("Программы Windows"), extensions: [isMac ? "app" : "exe"] }] });
      const paths = typeof chosen === "string" ? [chosen] : chosen || [];
      if (!paths.length) return;
      const entries = isMac ? await invoke<InstalledApp[]>("resolve_application_paths", { paths }) : paths.map((path) => installedApps.find((app) => normalizeAppPattern(app.executable) === normalizeAppPattern(path)) ||
        ({ id: normalizeAppPattern(path), name: path.split(/[\\/]/).pop()?.replace(/\.exe$/i, "") || path, executable: path, patterns: [path], source: "manual" }));
      setAddedApps((current) => [...current.filter((app) => !entries.some((entry) => entry.id === app.id)), ...entries]);
      updateSettings("redirect_processes", [...new Set([...settingsDraft.redirect_processes, ...entries.flatMap((app) => app.patterns)])]);
      const now = Math.floor(Date.now() / 1000);
      updateSettings("app_added_at", { ...(settingsDraft.app_added_at ?? {}), ...Object.fromEntries(entries.map((app) => [app.id, now])) });
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
    const marked = installedApps.filter((app) => isAppSelected(app) && isPriorityApp(app)).flatMap((app) => app.patterns);
    return {
      ...settingsDraft,
      redirect_processes: dedupePatterns([...selected, ...manual]),
      priority_udp_processes: dedupePatterns([...marked, ...manual.filter((pattern) => priorityPatterns.has(normalizeAppPattern(pattern)))]),
    };
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

  /** Saves the picked programs and starts interception; used on «Приложения» and «Подключение». */
  function startInterception() {
    return runAction("connect", async () => {
      const settings = applicationSettings();
      await invoke<AppConfig>("save_settings", { settings });
      setSettingsDraft(settings);
      settingsDirtyRef.current = false;
      return invoke<string>("start_redirect");
    });
  }

  const usesEmbeddedCheck = (server: ServerProfile) => Boolean(server.jump_server_id) || config.settings.ssh_engine === "embedded";
  const checksAgent = (server: ServerProfile) =>
    usesEmbeddedCheck(server) && !config.servers.some((profile) => profile.jump_server_id === server.id);
  const checkingServer = serverCheck?.state === "running";

  async function checkServer(server: ServerProfile) {
    if (checkingServer) return;
    const started = Math.floor(Date.now() / 1000);
    // Allow time for the backend's deadline and IPC to return the diagnostic.
    const timeout = usesEmbeddedCheck(server) ? (server.jump_server_id ? 45 : 35) : server.connect_timeout + 10;
    let timer: number | undefined;
    setServerCheck({ id: server.id, state: "running", at: started,
      text: checksAgent(server) ? t("Проверяем вход по ключу и совместимость агента.") : t("Проверяем вход на сервер по ключу.") });
    try {
      const output = await Promise.race([
        invoke<string>("test_server", { server }),
        new Promise<never>((_, reject) => {
          timer = window.setTimeout(() => reject(t("Проверка SSH не вернула ответ вовремя. Повторите проверку.")), timeout * 1000);
        }),
      ]);
      setServerCheck({ id: server.id, state: "ok", at: Math.floor(Date.now() / 1000), text: messageText[output] ?? output });
    } catch (error) {
      setServerCheck({ id: server.id, state: "error", at: Math.floor(Date.now() / 1000), text: String(error) });
    } finally {
      window.clearTimeout(timer);
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
    setServerDraft((current) => ({ ...current, [key]: value,
      host_key_sha256: key === "host" || key === "port" ? null : current.host_key_sha256,
      id: nextId(key, value, current, config.servers) }));
  }

  function updateProxy<K extends keyof ProxyProfile>(key: K, value: ProxyProfile[K]) {
    proxyDraftDirtyRef.current = true;
    setProxyDraft((current) => ({ ...current, [key]: value, id: nextId(key, value, current, config.proxies) }));
  }

  function updateSettings<K extends keyof AppSettings>(key: K, value: AppSettings[K]) {
    settingsDirtyRef.current = true;
    setSettingsDraft((current) => ({ ...current, [key]: value }));
  }

  async function choosePrivateKey() {
    try {
      const selected = await open({ multiple: false, directory: false, title: t("Выберите приватный ключ SSH") });
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
      if (field === "logs") notify(t("Журнал скопирован"));
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
    if (activeTab !== "applications") return;
    const refresh = () => {
      if (document.visibilityState === "visible") void scanApplications();
    };
    refresh();
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    const timer = window.setInterval(() => {
      if (document.hasFocus()) refresh();
    }, 15000);
    return () => {
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
      window.clearInterval(timer);
    };
  }, [activeTab, scanApplications]);

  // Stats and the sidebar need program names and icons as soon as interception runs.
  useEffect(() => {
    if ((activeTab === "stats" || redirect.running) && !appsLoadedRef.current) void scanApplications();
  }, [activeTab, redirect.running, scanApplications]);

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

  useEffect(() => {
    if (!appMenu) return;
    appMenuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    const close = (event: Event) => { if (!appMenuRef.current?.contains(event.target as Node)) setAppMenu(null); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") setAppMenu(null); };
    window.addEventListener("mousedown", close);
    window.addEventListener("scroll", close, true);
    window.addEventListener("keydown", escape);
    return () => { window.removeEventListener("mousedown", close); window.removeEventListener("scroll", close, true); window.removeEventListener("keydown", escape); };
  }, [appMenu]);

  const updateFailureChecked = useRef(false);
  useEffect(() => {
    if (updateFailureChecked.current || redirect.platform !== "windows") return;
    updateFailureChecked.current = true;
    invoke<UpdateDownload>("update_status").then((state) => {
      if (state.error) notify(state.error, true);
    }).catch(() => undefined);
  }, [redirect.platform]);

  // Once per launch: a newer release is offered unless the user skipped it.
  const updateChecked = useRef(false);
  useEffect(() => {
    if (updateChecked.current || !status.app_version || config.settings.check_updates === false) return;
    const timer = window.setTimeout(() => {
      updateChecked.current = true;
      invoke<Release>("check_update").then((found) => {
        setRelease(found);
        let skipped = "";
        try { skipped = localStorage.getItem("secret-tunnel.skip-version") ?? ""; } catch { /* nothing skipped */ }
        if (found.newer && skipped !== found.version) setUpdatePrompt(found);
      }).catch(() => undefined);
    }, 4000);
    return () => window.clearTimeout(timer);
  }, [status.app_version, config.settings.check_updates]);

  // Session history refreshes while the statistics are on screen.
  useEffect(() => {
    if (activeTab !== "stats" && activeTab !== "applications") return;
    const load = () => invoke<SessionRecord[]>("session_history").then(setSessions).catch(() => undefined);
    load();
    const timer = window.setInterval(load, 3000);
    return () => window.clearInterval(timer);
  }, [activeTab]);

  const ipActions = (light: boolean) => (
    <span className="inlineActions">
      <button type="button" className={light ? "tinyButton light" : "tinyButton"} title={showSensitive ? t("Скрыть IP") : t("Показать IP")}
        aria-label={showSensitive ? t("Скрыть IP") : t("Показать IP")} onClick={() => setSensitiveVisible(!showSensitive)}>
        {showSensitive ? <EyeOff size={13} /> : <Eye size={13} />}
      </button>
      <button type="button" className={light ? "tinyButton light" : "tinyButton"} title={t("Скопировать IP")} aria-label={t("Скопировать IP")}
        disabled={!status.ip} onClick={() => copyText(status.ip, "proxy-ip")}>
        {copiedField === "proxy-ip" ? <Check size={13} /> : <Copy size={13} />}
      </button>
    </span>
  );

  const nav: { tab: Tab; label: string; icon: React.ReactNode; badge?: string }[] = [
    { tab: "connect", label: t("Подключение"), icon: <Power size={17} /> },
    { tab: "servers", label: t("Серверы"), icon: <Server size={17} />, badge: String(config.servers.length || "") },
    { tab: "catalog", label: t("Каталог серверов"), icon: <Globe size={17} /> },
    { tab: "proxies", label: t("Локальный прокси"), icon: <Network size={17} /> },
    { tab: "applications", label: t("Приложения"), icon: <LayoutGrid size={17} />, badge: redirect.running ? t("вкл") : undefined },
    { tab: "domains", label: t("Домены"), icon: <Globe size={17} /> },
    { tab: "stats", label: t("Статистика"), icon: <ChartColumn size={17} /> },
    { tab: "speed", label: t("Скорость"), icon: <Gauge size={17} />, badge: speedLive.running ? t("тест") : undefined },
    { tab: "settings", label: t("Настройки"), icon: <Settings size={17} /> },
    { tab: "about", label: t("О программе"), icon: <Info size={17} />, badge: release?.newer ? t("новое") : undefined },
  ];

  return (
    <main className="app">
      <aside className="sidebar">
        <div className="brand">
          <img src={appIconUrl} alt="" className="brandIcon" />
          <div>
            <h1>Secret Tunnel</h1>
            <p>{status.app_version ? t("Версия {0}", status.app_version) : t("SSH-туннель")}</p>
          </div>
        </div>

        <section className={`sideStatus phase-${phase}`} aria-live="polite">
          <div className="sideState"><span className="pulse" aria-hidden="true" />{stateTitle(status.state)}</div>
          <dl>
            <div><dt>{t("Сервер")}</dt><dd title={routeNames.join(" → ")}>{currentServer.name || t("не выбран")}</dd></div>
            <div><dt>{t("Выход")}</dt><dd>{connected && status.country_code ? <><Flag code={status.country_code} label={country} /> {country}</> : "—"}</dd></div>
            <div><dt>IP</dt><dd className="withActions"><span className="mono">{connected ? visibleIp : "—"}</span>{connected && ipActions(false)}</dd></div>
            <div><dt>{t("В сети")}</dt><dd>{connected ? formatUptime(status.started_at, now) : "—"}</dd></div>
            <div><dt>{t("Сейчас")}</dt><dd className="rates" title={t("Скорость сейчас")}><span><ArrowDown size={12} />{connected && latest ? formatBitRate(latest.rx_bps) : "—"}</span><span><ArrowUp size={12} />{connected && latest ? formatBitRate(latest.tx_bps) : "—"}</span></dd></div>
          </dl>
          {redirect.running && <p className="sideNote">{t("Перехват: ")}{selectedApps.length} {t(" прил.")}</p>}
        </section>

        <nav className="tabs" aria-label={t("Разделы")}>
          {nav.map((item) => (
            <button key={item.tab} className={activeTab === item.tab ? "active" : ""} onClick={() => setActiveTab(item.tab)} aria-current={activeTab === item.tab ? "page" : undefined}>
              {item.icon}<span>{item.label}</span>{item.badge && <em>{item.badge}</em>}
            </button>
          ))}
        </nav>
      </aside>

      <section className="workspace">
        <div className={activeTab === "connect" ? "workspaceContent fill" : "workspaceContent"}>
          {activeTab === "connect" && (
            <section className="connect">
              <div className={`hero phase-${phase}`}>
                <div className="heroMain">
                  <h2>{stateTitle(status.state)}</h2>
                  <p className="heroSub">
                    {connected ? <>{t("SSH до ")}{status.server_name || currentServer.name}{country ? <> · <Flag code={status.country_code} label={country} /> {country}</> : null}{t(". В сети ")}{formatUptime(status.started_at, now)}.</>
                      : status.state === "stopping" ? t("Закрываем SSH и останавливаем перехват.")
                      : phase === "busy" ? t("Устанавливаем SSH-соединение.")
                      : phase === "error" ? (translateMessage(status.last_error) || t("Соединение прервалось. Подробности в журнале ниже."))
                      : currentServer.id ? t("Нажмите «Подключить», чтобы направить трафик через {0}.", currentServer.name) : t("Добавьте SSH-сервер во вкладке «Серверы».")}
                  </p>
                  <RouteRibbon nodes={routeNodes} state={phase} />
                </div>
                <div className="heroActions">
                  {!sessionActive && <button className="ghost" onClick={() => setSetupOpen(true)}>{t("Настроить подключение")}</button>}
                  {!sessionActive ? (
                    <button className="primary big" disabled={busy !== null || !currentServer.id || !currentProxy.id}
                      onClick={() => runAction("connect", () => invoke<string>("start_tunnel"))}>
                      <Play size={17} />{t("Подключить")}</button>
                  ) : (
                    <button className="danger big" disabled={busy !== null} onClick={() => runAction("disconnect", () => invoke<string>("stop_tunnel"))}>
                      <CircleStop size={17} />{t("Отключить")}</button>
                  )}
                  <button className="ghost" disabled={busy !== null || !currentServer.id || !currentProxy.id} onClick={() => runAction("restart", () => invoke<string>("restart_tunnel"))}>
                    <RefreshCcw size={15} />{t("Переподключить")}</button>
                  {redirectSupported && (selectedApps.length > 0 || redirectActive) && (!redirectActive ? (
                    <button className="ghost" disabled={!canStartInterception} onClick={startInterception}
                      title={!connected ? t("Сначала подключите туннель") : !redirect.component_available ? t("Компонент перехвата не найден — подробности на вкладке «Приложения»") : t("Направить {0} {1} через туннель", selectedApps.length, plural(selectedApps.length, t("программу"), t("программы"), t("программ")))}>
                      <LayoutGrid size={15} />{t("Включить перехват · ")}{selectedApps.length}
                    </button>
                  ) : (
                    <button className="ghost" disabled={busy !== null} onClick={() => runAction("disconnect", () => invoke<string>("stop_redirect"))}>
                      <CircleStop size={15} />{redirect.starting ? t("Отменить перехват") : t("Выключить перехват")}
                    </button>
                  ))}
                </div>
              </div>

              <ConnectionHealth
                ssh={stateTitle(status.state)}
                connected={connected}
                interception={redirect.running ? (connected ? t("Включён") : t("Без SSH")) : redirect.starting ? t("Запускается")
                  : redirect.last_error ? t("Ошибка") : redirect.platform === "unknown" ? t("Проверяем")
                  : !redirectSupported || !redirect.component_available ? t("Компонент недоступен") : t("Выключен")}
                interceptionError={redirect.last_error}
                directWarning={isMac && redirect.component_available
                  ? t("При остановке или сбое перехвата новые соединения могут идти напрямую.") : ""}
                checks={status.checks}
                priorityUdp={connected ? status.priority_udp_ssh : null}
                interceptionRunning={redirect.running}
                onCheck={() => invoke("check_connection").then(refreshAll).catch((error) => notify(String(error), true))}
              />

              <div className="pickers">
                <label>
                  <span>{t("Сервер")}</span>
                  <select value={config.selected_server_id} disabled={sessionActive}
                    onChange={(event) => runAction("save", async () => { await invoke<AppConfig>("select_server", { id: event.target.value }); })}>
                    {config.servers.map((server) => {
                      const route = serverChain(server, config.servers);
                      return <option key={server.id} value={server.id}>{route && route.length > 1 ? route.join(" → ") : server.name}</option>;
                    })}
                  </select>
                </label>
                <label>
                  <span>{t("Локальный прокси для приложений")}</span>
                  <select value={config.selected_proxy_id} disabled={sessionActive}
                    onChange={(event) => runAction("save", async () => { await invoke<AppConfig>("select_proxy", { id: event.target.value }); })}>
                    {config.proxies.map((proxy) => <option key={proxy.id} value={proxy.id}>{proxy.name} — {proxy.host}:{proxy.port}</option>)}
                  </select>
                </label>
                {sessionActive && <p className="pickerNote">{t("Чтобы сменить сервер или прокси, отключите туннель.")}</p>}
              </div>

              <div className="metrics">
                <div className="metric">
                  <span>{t("Внешний IP")}</span>
                  <strong className="withActions"><span className="mono">{connected ? visibleIp : "—"}</span>{connected && ipActions(true)}</strong>
                </div>
                <div className="metric">
                  <span>{t("Страна выхода")}</span>
                  <strong title={country}>{connected && status.country_code ? <><Flag code={status.country_code} label={country} /> {country}</> : "—"}</strong>
                </div>
                <div className="metric">
                  <span>{t("Скачивание сейчас")}</span>
                  <strong className="rx">{connected && latest ? formatBitRate(latest.rx_bps) : "—"}</strong>
                </div>
                <div className="metric">
                  <span>{t("Отдача сейчас")}</span>
                  <strong className="tx">{connected && latest ? formatBitRate(latest.tx_bps) : "—"}</strong>
                </div>
                <div className="metric">
                  <span>{t("Пинг до сервера")}</span>
                  <strong>{connected && latestPing !== null ? t("{0} мс", latestPing) : "—"}</strong>
                </div>
                <div className="metric">
                  <span>{t("За сеанс")}</span>
                  <strong className="totals"><span><ArrowDown size={12} />{formatBytes(status.rx_total)}</span><span><ArrowUp size={12} />{formatBytes(status.tx_total)}</span></strong>
                </div>
                <div className="metric">
                  <span>{t("Локальный прокси")}</span>
                  <strong className="mono" title={t("Адрес SOCKS5 для браузера и приложений")}>{currentProxy.host}:{currentProxy.port}</strong>
                </div>
                <div className="metric">
                  <span>{t("Движок")}</span>
                  <strong>{engineLabel(status.ssh_engine)}</strong>
                </div>
              </div>

              <TrafficChart points={traffic} />
            </section>
          )}

          {activeTab === "catalog" && <HostingCatalog tunnelActive={sessionActive} onPurchased={() => { setSetupPurchased(true); setSetupOpen(true); }}/>}
          {activeTab === "servers" && (
            <div className="split">
              <section className="panel listPanel">
                <header className="panelHead">
                  <h2>{t("Серверы")}</h2>
                  <button className="ghost" onClick={() => setActiveTab("catalog")}>{t("Каталог серверов")}</button>
                  <button className="ghost" onClick={() => setTransfer({ serverId: null })}>{t("Импортировать")}</button>
                  <button className="primary" onClick={() => setSetupOpen(true)}>{t("Мастер настройки")}</button>
                  <button className="ghost" onClick={() => { serverDraftDirtyRef.current = true; setServerDraft(emptyServer); }}><Plus size={15} />{t("Добавить")}</button>
                </header>
                <div className="profileList" role="listbox" aria-label={t("SSH-серверы")}>
                  {config.servers.map((server) => {
                    const route = serverChain(server, config.servers);
                    const active = server.id === config.selected_server_id;
                    return (
                      <button key={server.id} role="option" aria-selected={server.id === serverDraft.id}
                        className={`profileItem${server.id === serverDraft.id ? " editing" : ""}`}
                        onClick={() => { serverDraftDirtyRef.current = true; setServerDraft(server); }}>
                        <span className="profileName">{server.name}{active && <em className="activeBadge"><Star size={11} />{t("для подключения")}</em>}</span>
                        <span className="profileMeta">{server.user}@{showSensitive ? server.host : maskValue(server.host)}:{server.port}</span>
                        {route && route.length > 1 && <span className="profileMeta">{t("через ")}{route.slice(0, -1).join(" → ")}</span>}
                      </button>
                    );
                  })}
                  {!config.servers.length && <p className="emptyHint">{t("Серверов пока нет. Нажмите «Добавить» и укажите адрес, пользователя и ключ SSH.")}</p>}
                </div>
              </section>

              <section className="panel">
                <header className="panelHead">
                  <h2>{serverDraft.name || t("Новый сервер")}</h2>
                  {serverDraft.id && serverDraft.id === config.selected_server_id ? <span className="chip good">{t("Используется для подключения")}</span>
                    : serverDraft.id && config.servers.some((server) => server.id === serverDraft.id) ? (
                      <button className="ghost" disabled={busy !== null || sessionActive} onClick={() => runAction("save", async () => { await invoke<AppConfig>("select_server", { id: serverDraft.id }); return t("Подключение пойдёт через {0}", serverDraft.name); })}>
                        <Star size={15} />{t("Использовать для подключения")}</button>) : null}
                </header>
                <div className="formGrid">
                  <label><span>{t("Название")}</span><input value={serverDraft.name} onChange={(event) => updateServer("name", event.target.value)} placeholder={t("Франция")} /></label>
                  <label><span>ID</span><input value={serverDraft.id} onChange={(event) => updateServer("id", event.target.value)} placeholder="fr" /></label>
                  <label>
                    <span>{t("Адрес сервера")}</span>
                    <div className="secretField">
                      <input type={showServerHost ? "text" : "password"} value={serverDraft.host} onChange={(event) => updateServer("host", event.target.value)} placeholder="example.com" />
                      <button type="button" className="iconButton" title={showServerHost ? t("Скрыть адрес") : t("Показать адрес")} aria-label={showServerHost ? t("Скрыть адрес") : t("Показать адрес")} onClick={() => setShowServerHost((value) => !value)}>
                        {showServerHost ? <EyeOff size={16} /> : <Eye size={16} />}
                      </button>
                      <button type="button" className="iconButton" title={t("Скопировать адрес")} aria-label={t("Скопировать адрес")} disabled={!serverDraft.host} onClick={() => copyText(serverDraft.host, "server-host")}>
                        {copiedField === "server-host" ? <Check size={16} /> : <Copy size={16} />}
                      </button>
                    </div>
                  </label>
                  <label><span>{t("Порт SSH")}</span><input type="number" value={serverDraft.port} onChange={(event) => updateServer("port", Number(event.target.value))} /></label>
                  <label><span>{t("Пользователь")}</span><input value={serverDraft.user} onChange={(event) => updateServer("user", event.target.value)} placeholder="user" /></label>
                  <label>
                    <span>{t("Подключаться через")}</span>
                    <select value={serverDraft.jump_server_id ?? ""} onChange={(event) => updateServer("jump_server_id", event.target.value || null)}>
                      <option value="">{t("Напрямую")}</option>
                      {config.servers.filter((server) => server.id !== serverDraft.id).map((server) => <option key={server.id} value={server.id}>{server.name}</option>)}
                    </select>
                  </label>
                  {serverDraft.jump_server_id && (
                    <p className="wide fieldNote">{t("Маршрут: ")}{serverChain(serverDraft, config.servers)?.join(" → ") ?? t("проверьте цепочку SSH")}{t(". Агент нужен только на последнем сервере.")}</p>
                  )}
                  <label className="wide">
                    <span>{t("Приватный ключ SSH")}</span>
                    <div className="fileField">
                      <input value={serverDraft.key_path} onChange={(event) => updateServer("key_path", event.target.value)} placeholder="~/.ssh/id_ed25519" />
                      <button type="button" className="iconButton" title={t("Выбрать файл ключа")} aria-label={t("Выбрать файл ключа")} onClick={choosePrivateKey}><FolderOpen size={16} /></button>
                    </div>
                  </label>
                </div>
                <details className="advanced">
                  <summary>{t("Для опытных: проверка ключа хоста, таймауты")}</summary>
                  <div className="formGrid">
                    <label>
                      <SettingLabel hint={t("yes — подключаться только к серверу из known_hosts; accept-new — один раз запомнить новый ключ хоста; no — не проверять (небезопасно).")}>{t("Проверка ключа хоста")}</SettingLabel>
                      <select value={serverDraft.strict_host_key_checking} onChange={(event) => updateServer("strict_host_key_checking", event.target.value)}>
                        <option value="yes">{t("Строго (yes)")}</option>
                        <option value="accept-new">{t("Запомнить новый (accept-new)")}</option>
                        <option value="no">{t("Не проверять (no)")}</option>
                      </select>
                    </label>
                    <label><span>{t("Таймаут подключения, с")}</span><input type="number" min={1} max={120} value={serverDraft.connect_timeout} onChange={(event) => updateServer("connect_timeout", Number(event.target.value))} /></label>
                    <label><span>{t("Проверка связи каждые, с")}</span><input type="number" min={1} max={300} value={serverDraft.server_alive_interval} onChange={(event) => updateServer("server_alive_interval", Number(event.target.value))} /></label>
                    <label><span>{t("Пропусков до разрыва")}</span><input type="number" min={1} max={20} value={serverDraft.server_alive_count_max} onChange={(event) => updateServer("server_alive_count_max", Number(event.target.value))} /></label>
                    <label className="checkboxLine wide"><input type="checkbox" checked={serverDraft.compression} onChange={(event) => updateServer("compression", event.target.checked)} />{t("Сжатие SSH (только для системного OpenSSH)")}</label>
                  </div>
                </details>
                <div className="actions">
                  <button className="primary" disabled={busy !== null || !serverDraft.id} onClick={() => runAction("save", async () => {
                    const saved = serverDraft;
                    await invoke<AppConfig>("save_server", { server: saved });
                    serverDraftDirtyRef.current = false;
                    setServerDraft(saved);
                    return t("Сервер «{0}» сохранён", saved.name);
                  })}><Save size={15} />{t("Сохранить")}</button>
                  {config.servers.some(server => server.id === serverDraft.id) && <button onClick={() => setTransfer({ serverId: serverDraft.id })}>{t("Поделиться")}</button>}
                  <button disabled={checkingServer || busy !== null || !serverDraft.id} onClick={() => checkServer(serverDraft)}>
                    <Terminal size={15} />
                    {checkingServer && serverCheck.id === serverDraft.id ? t("Проверяем…") : checksAgent(serverDraft) ? t("Проверить SSH и агент") : t("Проверить SSH")}
                  </button>
                  <button className="quietDanger" disabled={busy !== null || !serverDraft.id || sessionActive} onClick={() => runAction("delete", async () => {
                    await invoke<AppConfig>("delete_server", { id: serverDraft.id });
                    serverDraftDirtyRef.current = false;
                    setServerDraft(emptyServer);
                    return t("Сервер удалён");
                  })}><Trash2 size={15} />{t("Удалить")}</button>
                </div>
                {serverCheck && serverCheck.id === serverDraft.id && (
                  <div className={`checkResult ${serverCheck.state}`} role={serverCheck.state === "error" ? "alert" : "status"}>
                    <strong>{serverCheck.state === "running" ? t("Проверяем…") : serverCheck.state === "ok" ? t("Проверка пройдена") : t("Проверка не прошла")}</strong>
                    <span>{translateMessage(showSensitive || showServerHost ? serverCheck.text : maskAddresses(serverCheck.text))}</span>
                    {serverCheck.state !== "running" && <small>{new Date(serverCheck.at * 1000).toLocaleTimeString(formatLocale())} {t(" · подробности в журнале внизу")}</small>}
                  </div>
                )}
              </section>
            </div>
          )}

          {activeTab === "proxies" && (
            <div className="split">
              <section className="panel listPanel">
                <header className="panelHead">
                  <h2>{t("Локальные прокси")}</h2>
                  <button className="ghost" onClick={() => { proxyDraftDirtyRef.current = true; setProxyDraft(emptyProxy); }}><Plus size={15} />{t("Добавить")}</button>
                </header>
                <p className="panelIntro">{t("SOCKS5-адрес на этом компьютере. Его можно указать в браузере или программе, а перехват приложений использует его сам.")}</p>
                <div className="profileList" role="listbox" aria-label={t("Локальные прокси")}>
                  {config.proxies.map((proxy) => (
                    <button key={proxy.id} role="option" aria-selected={proxy.id === proxyDraft.id}
                      className={`profileItem${proxy.id === proxyDraft.id ? " editing" : ""}`}
                      onClick={() => { proxyDraftDirtyRef.current = true; setProxyDraft(proxy); }}>
                      <span className="profileName">{proxy.name}{proxy.id === config.selected_proxy_id && <em className="activeBadge"><Star size={11} />{t("для подключения")}</em>}</span>
                      <span className="profileMeta mono">{proxy.host}:{proxy.port}</span>
                    </button>
                  ))}
                </div>
              </section>
              <section className="panel">
                <header className="panelHead">
                  <h2>{proxyDraft.name || t("Новый прокси")}</h2>
                  {proxyDraft.id && proxyDraft.id === config.selected_proxy_id ? <span className="chip good">{t("Используется для подключения")}</span>
                    : proxyDraft.id && config.proxies.some((proxy) => proxy.id === proxyDraft.id) ? (
                      <button className="ghost" disabled={busy !== null || sessionActive} onClick={() => runAction("save", async () => { await invoke<AppConfig>("select_proxy", { id: proxyDraft.id }); return t("Туннель будет слушать {0}:{1}", proxyDraft.host, proxyDraft.port); })}>
                        <Star size={15} />{t("Использовать для подключения")}</button>) : null}
                </header>
                <div className="formGrid">
                  <label><span>{t("Название")}</span><input value={proxyDraft.name} onChange={(event) => updateProxy("name", event.target.value)} placeholder="Local 1080" /></label>
                  <label><span>ID</span><input value={proxyDraft.id} onChange={(event) => updateProxy("id", event.target.value)} placeholder="local-1080" /></label>
                  <label>
                    <SettingLabel hint={t("127.0.0.1 — прокси доступен только этому компьютеру. Другой адрес откроет его для сети, без пароля это небезопасно.")}>{t("Адрес")}</SettingLabel>
                    <input value={proxyDraft.host} disabled={sessionActive} onChange={(event) => updateProxy("host", event.target.value)} placeholder="127.0.0.1" />
                  </label>
                  <label><span>{t("Порт")}</span><input type="number" disabled={sessionActive} value={proxyDraft.port} onChange={(event) => updateProxy("port", Number(event.target.value))} /></label>
                </div>
                <div className="actions">
                  <button className="primary" disabled={busy !== null || sessionActive || !proxyDraft.id} onClick={() => runAction("save", async () => {
                    const saved = proxyDraft;
                    await invoke<AppConfig>("save_proxy", { proxy: saved });
                    proxyDraftDirtyRef.current = false;
                    setProxyDraft(saved);
                    return t("Прокси «{0}» сохранён", saved.name);
                  })}><Save size={15} />{t("Сохранить")}</button>
                  <button className="quietDanger" disabled={busy !== null || !proxyDraft.id || sessionActive} onClick={() => runAction("delete", async () => {
                    await invoke<AppConfig>("delete_proxy", { id: proxyDraft.id });
                    proxyDraftDirtyRef.current = false;
                    setProxyDraft(emptyProxy);
                    return t("Прокси удалён");
                  })}><Trash2 size={15} />{t("Удалить")}</button>
                </div>
              </section>
            </div>
          )}

          {activeTab === "applications" && (
            <section className="panel appsPanel">
              <header className="panelHead">
                <div>
                  <h2>{t("Приложения через туннель")}</h2>
                  <p className="panelIntro">{t("Отмеченные программы идут через SSH-туннель целиком, по TCP и UDP. Остальные работают напрямую.")}</p>
                </div>
                <div className="appsControl">
                  <span className={redirect.running ? "chip good" : redirect.starting ? "chip busy" : "chip"}>
                    {redirect.starting ? t("Ждём разрешения macOS") : redirect.running ? t("Перехват включён · {0}", selectedApps.length) : t("Перехват выключен")}
                  </span>
                  {!redirectActive ? (
                    <button className="primary" disabled={!canStartInterception}
                      title={!connected ? t("Сначала подключите туннель") : undefined}
                      onClick={startInterception}><Play size={15} />{t("Включить перехват")}</button>
                  ) : (
                    <button className="danger" disabled={busy !== null} onClick={() => runAction("disconnect", () => invoke<string>("stop_redirect"))}>
                      <CircleStop size={15} />{redirect.starting ? t("Отменить") : t("Выключить")}
                    </button>
                  )}
                </div>
              </header>

              <div className="notices">
                {followed.length > 0 && (
                  <div className="notice info followNotice">
                    <strong>{t("Подхвачены на ходу, без перезапуска перехвата:")}</strong>
                    <span className="followList">
                      {followed.map((item) => <span key={item.rule} className="followChip" title={item.rule}><code>{item.process}</code> {t(" из ")}{item.app}</span>)}
                    </span>
                  </div>
                )}
                {leaks.length > 0 && (
                  <div className="notice leakNotice">
                    <strong>{t("Мимо туннеля идут программы, которые запустили выбранные приложения.")}</strong>
                    <span> {t(" macOS опознаёт их по собственной подписи, поэтому правило приложения на них не действует.")}</span>
                    <ul className="leakList">
                      {leaks.slice(0, 8).map((leak) => (
                        <li key={leak.rule} title={leak.path}><code>{leak.process}</code> {t(" из ")}{leak.app} · {leak.flows} {leak.flows === 1 ? t("соединение") : leak.flows < 5 ? t("соединения") : t("соединений")}</li>
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
                      return t("Через туннель пойдут ещё {0} {1}. Перехват перезапущен", leaks.length, leaks.length === 1 ? t("программа") : leaks.length < 5 ? t("программы") : t("программ"));
                    })}><Play size={15} />{t("Направить через туннель")}</button>
                  </div>
                )}
                {!connected && redirectSupported && <div className="notice info">{t("Перехват работает поверх подключённого туннеля. Подключитесь на вкладке «Подключение».")}</div>}
                {!redirectSupported && redirect.platform !== "unknown" && <div className="notice">{t("Перехват приложений доступен на macOS и Windows.")}</div>}
                {redirect.requires_administrator && !redirect.administrator && <div className="notice">{t("Для перехвата запустите Secret Tunnel от имени администратора.")}</div>}
                {!redirect.component_available && <div className="notice">{isMac ? <>{t("Установите ")}<a href="https://github.com/InterceptSuite/ProxyBridge/releases" target="_blank" rel="noreferrer">{t("ProxyBridge 4.1.2 или новее")}</a> {t(" в «Программы» и разрешите его сетевое расширение в системных настройках. Окно ProxyBridge должно быть закрыто.")}</> : t("Компонент перехвата не установлен. Укажите путь к нему в разделе «Дополнительно» ниже.")}</div>}
                {isMac && status.ssh_engine !== "embedded" && <div className="notice">{t("Системный OpenSSH передаёт только TCP, UDP выбранных приложений будет заблокирован. Для TCP и UDP выберите встроенный SSH в настройках.")}</div>}
                {!isMac && status.ssh_engine !== "embedded" && <div className="notice">{t("Для TCP и UDP выберите встроенный SSH в настройках и переподключитесь.")}</div>}
                {isMac && (settingsDraft.domain_rules || []).some((r) => r.enabled) && <div className="notice">{t("Отключите доменные правила: на macOS они пока не применяются.")}</div>}
                {isMac && settingsDraft.redirect_processes.some((id) => ["shipit", "chrome_crashpad_handler"].includes(id.toLowerCase())) && <div className="notice">{t("ShipIt и chrome_crashpad_handler есть во многих Electron-приложениях: правило направит в туннель и их одноимённые утилиты.")}</div>}
                {isMac && redirectActive && redirect.setup_message && <div className="notice info" role="status">{redirect.setup_message}</div>}
                {redirect.last_error && <div className="notice bad">{redirect.last_error}</div>}
                {redirect.running && <div className="notice info">{t("Перехват действует на новые соединения. Перезапустите выбранные программы, если они были открыты до включения.")}</div>}
              </div>

              <div className="appToolbar">
                <label className="search">
                  <Search size={15} />
                  <input aria-label={t("Поиск программы")} placeholder={t("Найти программу")} value={appSearch} onChange={(event) => setAppSearch(event.target.value)} />
                </label>
                <div className="segmented" role="group" aria-label={t("Показать")}>
                  <button className={!selectedAppsOnly ? "on" : ""} onClick={() => setSelectedAppsOnly(false)}>{t("Все · ")}{appChoices.length}</button>
                  <button className={selectedAppsOnly ? "on" : ""} onClick={() => setSelectedAppsOnly(true)}>{t("Выбранные · ")}{selectedApps.length}</button>
                </div>
                <button className="ghost" disabled={appsScanning} onClick={() => scanApplications(true)} title={t("Найти установленные программы заново")}><RefreshCcw size={15} />{appsScanning ? t("Ищем…") : t("Обновить")}</button>
                <button className="ghost" disabled={busy !== null} onClick={addApplicationFile}><Plus size={15} />{t("Добавить ")}{isMac ? ".app" : ".exe"}</button>
                <div className="segmented viewSwitch" role="group" aria-label={t("Вид списка")}>
                  {appViews.map((view) => (
                    <button key={view.id} className={appView === view.id ? "on" : ""} title={view.hint} aria-pressed={appView === view.id} onClick={() => chooseAppView(view.id)}>{view.title}</button>
                  ))}
                </div>
              </div>
              {appsError && <div className="notice bad">{appsError}</div>}

              {showPriorityChoices && (
                <p className="fieldNote">{t("Отметьте «UDP через второй SSH» у приложений для звонков. После сохранения выбора переподключите туннель и снова включите перехват.")}</p>
              )}
              {appView === "table" ? (
                <div className="appTableWrap" aria-busy={appsScanning}>
                  <table className="appTable" aria-label={t("Установленные программы")}>
                    <thead><tr><th>{t("Программа")}</th><th>{t("Путь")}</th><th className="num">{t("Процессов")}</th><th className="num">{t("Трафик, 30 дней")}</th><th>{t("Через туннель с")}</th><th aria-label={t("Через туннель")} /></tr></thead>
                    <tbody>
                      {visibleApps.map((app) => {
                        const selected = isAppSelected(app);
                        const usage = appTraffic.get(app.id);
                        const added = settingsDraft.app_added_at?.[app.id];
                        return (
                          <tr key={app.id} className={selected ? "selected" : undefined} onContextMenu={(event) => openAppMenu(event, app)} onKeyDown={(event) => appMenuKey(event, app)}>
                            <td><span className="appName"><AppIcon app={app} icon={appIcons[app.executable]} /><strong>{app.name}</strong></span></td>
                            <td className="path" title={app.executable}>{app.executable}</td>
                            <td className="num">{app.patterns.length}</td>
                            <td className="num">{usage ? formatBytes(usage.rx + usage.tx) : "—"}</td>
                            <td>{selected && added ? formatDay(added) : "—"}</td>
                            <td><input type="checkbox" className="switch" checked={selected} disabled={busy !== null} onChange={(event) => toggleApplication(app, event.target.checked)} aria-label={t("Направлять {0} через туннель", app.name)} />{selected && showPriorityChoices && priorityCheckbox(app)}</td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                  {!visibleApps.length && <p className="emptyHint">{emptyAppsHint}</p>}
                </div>
              ) : (
                <div className={appView === "cards" ? "appGrid large" : "appGrid"} aria-label={t("Установленные программы")} aria-busy={appsScanning}>
                  {visibleApps.map((app) => {
                    const selected = isAppSelected(app);
                    const usage = appTraffic.get(app.id);
                    const added = settingsDraft.app_added_at?.[app.id];
                    return (
                      <div key={app.id} className={selected ? "appTile selected" : "appTile"} onContextMenu={(event) => openAppMenu(event, app)} onKeyDown={(event) => appMenuKey(event, app)}
                        onMouseEnter={(event) => showAppTip(event, app)} onMouseLeave={hideAppTip}>
                        <label htmlFor={`app-tunnel-${app.id}`}><AppIcon app={app} icon={appIcons[app.executable]} /></label>
                        <span className="appText">
                          <label htmlFor={`app-tunnel-${app.id}`}><strong>{app.name}</strong><small>{appSubtitle(app)}</small></label>
                          {appView === "cards" && (
                            <span className="appFacts">
                              <span className="path">{app.executable}</span>
                              <span>{usage ? t("{0} за 30 дней · {1} {2}", formatBytes(usage.rx + usage.tx), usage.sessions, plural(usage.sessions, t("сеанс"), t("сеанса"), t("сеансов"))) : t("Через туннель ещё не ходила")}</span>
                              <span>{selected ? (added ? t("Через туннель с {0}", formatDay(added)) : t("Через туннель")) : t("Работает напрямую")} · {app.patterns.length} {plural(app.patterns.length, t("процесс"), t("процесса"), t("процессов"))}</span>
                            </span>
                          )}
                          {selected && showPriorityChoices && priorityCheckbox(app)}
                        </span>
                        <input id={`app-tunnel-${app.id}`} type="checkbox" className="switch" checked={selected} disabled={busy !== null} onChange={(event) => toggleApplication(app, event.target.checked)}
                          aria-label={t("Направлять {0} через туннель", app.name)} />
                      </div>
                    );
                  })}
                  {!visibleApps.length && <p className="emptyHint">{emptyAppsHint}</p>}
                </div>
              )}
              {appTip && !appMenu && (
                <div className="appTip" role="tooltip" style={{ left: appTip.x, top: appTip.y }}>
                  <span className="path">{appTip.app.executable}</span>
                  <span>{appTip.app.patterns.length > 1 ? t("Правила: {0}", appTip.app.patterns.map((pattern) => pattern.split(/[\\/]/).pop()).join(", ")) : t("Правило: сама программа")}</span>
                  <span className="tipHint">{t("Правая кнопка — действия с программой")}</span>
                </div>
              )}
              {appMenu && (
                <div className="contextMenu" role="menu" aria-label={appMenu.app.name} style={{ left: appMenu.x, top: appMenu.y }} ref={appMenuRef}
                  onKeyDown={(event) => { if (event.key === "Escape") setAppMenu(null); }}>
                  <strong>{appMenu.app.name}</strong>
                  {appMenuItems(appMenu.app).map((item) => item === "-" ? <hr key={Math.random()} /> : (
                    <button key={item.label} role="menuitem" className={item.danger ? "danger" : undefined} disabled={item.disabled} onClick={() => { setAppMenu(null); item.run(); }}>{item.label}</button>
                  ))}
                </div>
              )}

              <footer className="appsFooter">
                <details className="advanced">
                  <summary>{t("Дополнительно: правила вручную и компонент перехвата")}</summary>
                  <label>
                    <span>{isMac ? t("Bundle ID или идентификаторы подписи через запятую") : t("Имена exe или полные пути через запятую")}</span>
                    <input disabled={redirectActive} value={settingsDraft.redirect_processes.join(", ")}
                      onChange={(event) => updateSettings("redirect_processes", event.target.value.split(",").map((name) => name.trim()))} />
                  </label>
                  <label>
                    <span>{t("Компонент перехвата")}</span>
                    <div className="fileField">
                      <input disabled={redirectActive} value={settingsDraft.redirect_cli_path} onChange={(event) => updateSettings("redirect_cli_path", event.target.value)}
                        placeholder={isMac ? "/Applications/ProxyBridge.app/Contents/MacOS/ProxyBridge" : t("Путь к ProxyBridge_CLI.exe")} />
                      <button className="iconButton" disabled={redirectActive} title={t("Выбрать компонент")} aria-label={t("Выбрать компонент")} onClick={async () => {
                        try {
                          const selected = await open({ multiple: false, ...(isMac ? {} : { filters: [{ name: "ProxyBridge CLI", extensions: ["exe"] }] }) });
                          if (typeof selected === "string") updateSettings("redirect_cli_path", selected);
                        } catch (error) { notify(String(error), true); }
                      }}><FolderOpen size={16} /></button>
                    </div>
                  </label>
                  <p className="fieldNote">{isMac ? t("Программы сопоставляются по точным идентификаторам подписи: самого приложения, вложенных .app, .xpc, .appex и программ внутри него. DNS-запросы выбранных приложений идут через туннель на 1.1.1.1.") : t("Нужен проверенный комплект ProxyBridge с WinDivert. Системный DNS может идти напрямую.")} {t(" При аварии перехвата прямой выход не блокируется.")}</p>
                </details>
                <button disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const settings = applicationSettings();
                  await invoke<AppConfig>("save_settings", { settings });
                  setSettingsDraft(settings);
                  settingsDirtyRef.current = false;
                  if (redirectActive) {
                    await invoke<string>("stop_redirect");
                    if (settings.redirect_processes.length || (settings.domain_rules || []).some((r) => r.enabled)) return invoke<string>("start_redirect");
                    return t("Выбор очищен, перехват выключен");
                  }
                  return t("Выбор приложений сохранён");
                })}><Save size={15} />{redirectActive ? t("Применить выбор") : t("Сохранить выбор")}</button>
              </footer>
            </section>
          )}

          {activeTab === "domains" && (
            <>
              <div className="notice" hidden={redirect.supports_domains}>{t("Доменные правила применяются только в Windows. На macOS используйте выбор приложений и отключите ранее сохранённые доменные правила.")}</div>
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
                  return t("Правила доменов сохранены. Включите перехват, чтобы применить их");
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
            <SpeedPanel deviceLabel={deviceLabel} connected={connected} live={speedLive} history={speedHistory} serverPing={connected ? latestPing : null}
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
                return t("История сеансов очищена");
              })} />
          )}

          {activeTab === "about" && (
            <AboutPanel platform={redirect.platform} version={status.app_version || "—"} checkUpdates={config.settings.check_updates ?? true}
              onCheckUpdates={(value) => runAction("save", async () => {
                const settings = { ...config.settings, check_updates: value };
                const next = await invoke<AppConfig>("save_settings", { settings });
                setConfig(next);
                if (!settingsDirtyRef.current) setSettingsDraft(next.settings);
                return value ? t("Обновления будут проверяться при запуске") : t("Проверка обновлений при запуске выключена");
              })}
              servers={config.servers.map((server) => ({ id: server.id, name: server.name }))}
              selectedServerId={config.selected_server_id} release={release} onRelease={setRelease} />
          )}

          {activeTab === "settings" && (
            <section className="panel settingsPanel">
              <header className="panelHead"><h2>{t("Настройки")}</h2></header>

              <fieldset>
                <legend>{t("Оформление")}</legend>
                <label className="appearanceSetting">
                  <span>{t("Язык")}</span>
                  <select aria-label={t("Язык")} value={language} onChange={event => changeLanguage(event.target.value as "ru" | "en")}>
                    <option value="ru">Русский</option><option value="en">English</option>
                  </select>
                </label>
                <label className="appearanceSetting">
                  <span>{t("Тема")}</span>
                  <select value={appearance} onChange={(event) => setAppearance(event.target.value as Appearance)}>
                    <option value="system">{t("Системная")}</option>
                    <option value="light">{t("Светлая")}</option>
                    <option value="dark">{t("Тёмная")}</option>
                  </select>
                </label>
              </fieldset>

              <fieldset>
                <legend>{t("Подключение")}</legend>
                <label>
                  <SettingLabel hint={t("Встроенный SSH работает внутри приложения, передаёт TCP и UDP и умеет цепочки серверов. Системный OpenSSH использует ssh из системы и передаёт только TCP.")}>{t("Движок SSH")}</SettingLabel>
                  <select value={settingsDraft.ssh_engine} onChange={(event) => updateSettings("ssh_engine", event.target.value)}>
                    <option value="embedded">{t("Встроенный SSH — TCP и UDP (рекомендуется)")}</option>
                    <option value="system">{t("Системный OpenSSH — только TCP")}</option>
                  </select>
                </label>
                {isMac && <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.bypass_vpn_for_ssh ?? true} onChange={(event) => updateSettings("bypass_vpn_for_ssh", event.target.checked)} />
                  <SettingLabel hint={t("SSH-соединение к серверу идёт напрямую через Wi-Fi или Ethernet, даже если включён другой VPN (Clash, v2RayTun). Иначе туннель окажется внутри чужого VPN: двойное шифрование и ниже скорость. Выключите, если сервер доступен только через тот VPN.")}>{t("SSH мимо других VPN")}</SettingLabel></label>}
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.close_to_tray ?? true} onChange={(event) => updateSettings("close_to_tray", event.target.checked)} />
                  <SettingLabel hint={t("Закрытие окна прячет приложение {0}, туннель продолжает работать. Подключиться, отключиться и выйти можно из меню значка.", isMac ? t("в строку меню") : t("в трей"))}>{isMac ? t("Сворачивать в строку меню при закрытии окна") : t("Сворачивать в трей при закрытии окна")}</SettingLabel></label>
                {isMac && <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.follow_child_processes ?? true} onChange={(event) => updateSettings("follow_child_processes", event.target.checked)} />
                  <SettingLabel hint={t("Программы, которые запускают выбранные приложения (node, python, MCP-серверы, программы обновления), macOS опознаёт по их собственной подписи. Перехват замечает их за пару секунд и добавляет правило, не обрывая соединения. Командные оболочки и мелкие утилиты не добавляются.")}>{t("Подхватывать программы, запущенные выбранными приложениями")}</SettingLabel></label>}
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.auto_reconnect} onChange={(event) => updateSettings("auto_reconnect", event.target.checked)} />
                  <SettingLabel hint={t("Если соединение оборвалось, приложение подключится снова с выбранным сервером и прокси.")}>{t("Переподключаться автоматически")}</SettingLabel></label>
                <label className="inlineNumber">
                  <span>{t("Пауза перед переподключением, с")}</span>
                  <input type="number" min={1} max={120} value={settingsDraft.reconnect_delay_seconds} onChange={(event) => updateSettings("reconnect_delay_seconds", Number(event.target.value))} />
                </label>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.start_tunnel_on_launch} onChange={(event) => updateSettings("start_tunnel_on_launch", event.target.checked)} />
                  <SettingLabel hint={t("При открытии приложения сразу подключает выбранный сервер. Включайте, когда туннель уже проверен.")}>{t("Подключаться при запуске приложения")}</SettingLabel></label>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.verify_proxy_on_start} onChange={(event) => updateSettings("verify_proxy_on_start", event.target.checked)} />
                  <SettingLabel hint={t("Проверяет TCP, UDP, DNS и IPv6 через локальный SOCKS. Результаты видны в разделе «Подключение».")}>{t("Проверять прокси после подключения")}</SettingLabel></label>
              </fieldset>

              <fieldset>
                <legend>{t("Обновления")}</legend>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.check_updates ?? true} onChange={(event) => updateSettings("check_updates", event.target.checked)} />
                  <SettingLabel hint={t("При запуске приложение спрашивает GitHub о новом релизе и предлагает скачать и установить его. Без вашего согласия ничего не скачивается.")}>{t("Проверять обновления при запуске")}</SettingLabel></label>
                <p className="fieldNote">{t("Проверить вручную, обновить агент на сервере и почитать справку — в разделе «О программе».")}</p>
              </fieldset>

              <fieldset>
                <legend>{t("Конфиденциальность")}</legend>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.hide_sensitive} onChange={(event) => updateSettings("hide_sensitive", event.target.checked)} />
                  <SettingLabel hint={t("IP-адреса и токены скрыты, пока не нажать на глаз. Удобно для скриншотов и демонстраций.")}>{t("Скрывать IP-адреса и токены")}</SettingLabel></label>
                <label className="checkboxLine"><input type="checkbox" checked={settingsDraft.clear_logs_on_start} onChange={(event) => updateSettings("clear_logs_on_start", event.target.checked)} />
                  <SettingLabel hint={t("Очищает журнал перед каждым подключением.")}>{t("Очищать журнал при подключении")}</SettingLabel></label>
                <label>
                  <SettingLabel hint={t("Токен IPinfo Lite нужен, чтобы показывать внешний IP и страну выхода. Можно оставить пустым.")}>{t("Токен IPinfo")}</SettingLabel>
                  <div className="secretField">
                    <input type={showIpinfoToken ? "text" : "password"} value={settingsDraft.ipinfo_token} onChange={(event) => updateSettings("ipinfo_token", event.target.value)} placeholder={t("Токен IPinfo Lite")} />
                    <button type="button" className="iconButton" title={showIpinfoToken ? t("Скрыть токен") : t("Показать токен")} aria-label={showIpinfoToken ? t("Скрыть токен") : t("Показать токен")} onClick={() => setShowIpinfoToken((value) => !value)}>
                      {showIpinfoToken ? <EyeOff size={16} /> : <Eye size={16} />}
                    </button>
                    <button type="button" className="iconButton" title={t("Скопировать токен")} aria-label={t("Скопировать токен")} disabled={!settingsDraft.ipinfo_token} onClick={() => copyText(settingsDraft.ipinfo_token, "ipinfo-token")}>
                      {copiedField === "ipinfo-token" ? <Check size={16} /> : <Copy size={16} />}
                    </button>
                  </div>
                </label>
              </fieldset>

              <fieldset>
                <legend>{t("Для опытных: встроенный SSH")}</legend>
                <p className="fieldNote">{t("Применяется при следующем подключении ко всем серверам цепочки.")}</p>
                <label>
                  <span>{t("Шифрование")}</span>
                  <select value={settingsDraft.ssh_tuning?.cipher ?? "auto"} onChange={(event) => updateSettings("ssh_tuning", { ...defaultSshTuning, ...settingsDraft.ssh_tuning, cipher: event.target.value as SshTuning["cipher"] })}>
                    <option value="auto">{t("Авто")}</option>
                    <option value="aes128_gcm">AES-128-GCM</option>
                    <option value="chacha20_poly1305">ChaCha20-Poly1305</option>
                  </select>
                </label>
                <label>
                  <SettingLabel hint={t("Стандартная очередь быстрее на дальних маршрутах. Уменьшенная нужна для сравнения задержек и может ограничить скорость.")}>{t("Очередь SSH")}</SettingLabel>
                  <select value={settingsDraft.ssh_tuning?.channel_buffer_messages === 8 ? "small" : "default"} onChange={(event) => updateSettings("ssh_tuning", { ...defaultSshTuning, ...settingsDraft.ssh_tuning, channel_buffer_messages: event.target.value === "small" ? 8 : 32, channel_receive_window_bytes: event.target.value === "small" ? 1048576 : 4194304 })}>
                    <option value="default">{t("Стандартная · 4 МиБ")}</option>
                    <option value="small">{t("Уменьшенная · 1 МиБ · для тестов")}</option>
                  </select>
                </label>
                <label>
                  <SettingLabel hint={t("Во время звонков туннель придерживает загрузки, чтобы голос не прерывался. «Голос» придерживает сильнее, «Максимальная скорость» не придерживает и отключает контроль очереди.")}>{t("Приоритет звонков")}</SettingLabel>
                  <select value={callPriority === undefined ? balancedTcpBudget : callPriority ?? 0} onChange={(event) => updateSettings("ssh_tuning", { ...defaultSshTuning, ...settingsDraft.ssh_tuning, tcp_interactive_bytes: Number(event.target.value) || null })}>
                    <option value={balancedTcpBudget}>{t("Сбалансированный (рекомендуется)")}</option>
                    <option value={voiceTcpBudget}>{t("Голос")}</option>
                    <option value={0}>{t("Максимальная скорость")}</option>
                  </select>
                </label>
                {redirectSupported && embeddedForProfile && callPriority === voiceTcpBudget && (
                  <>
                    <label className="checkboxLine">
                      <input type="checkbox" checked={settingsDraft.priority_udp_ssh ?? false}
                        onChange={(event) => updateSettings("priority_udp_ssh", event.target.checked)} />
                      <span>{t("Отдельный SSH для UDP")}</span>
                    </label>
                    <p className="fieldNote">{t("Отделяет UDP отмеченных приложений от фонового трафика. Выберите приложения во вкладке «Приложения», затем переподключите туннель.")}</p>
                  </>
                )}
                <UdpWindowField title={t("UDP · загрузка")} value={settingsDraft.udp_receive_window}
                  hint={t("Сколько UDP-данных одного соединения туннель принимает без подтверждения. Больше окно — быстрее видео, звонки и QUIC на дальних маршрутах, но больше памяти. «По умолчанию» — 1 МиБ, у старого агента меньше.")}
                  onChange={(value) => updateSettings("udp_receive_window", value)} />
                <UdpWindowField title={t("UDP · отправка")} value={settingsDraft.agent_udp_receive_window}
                  hint={t("То же для отправки: окно, которое агент на сервере держит для ваших UDP-данных. «По умолчанию» — значение агента.")}
                  onChange={(value) => updateSettings("agent_udp_receive_window", value)} />
              </fieldset>

              <div className="actions">
                <button className="primary" disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const nextConfig = await invoke<AppConfig>("save_settings", { settings: settingsDraft });
                  settingsDirtyRef.current = false;
                  setConfig(nextConfig);
                  setSettingsDraft(nextConfig.settings);
                  return t("Настройки сохранены");
                })}><Save size={15} />{t("Сохранить настройки")}</button>
                <button className="ghost" disabled={busy !== null} onClick={() => runAction("save", async () => {
                  const settings = await invoke<AppSettings>("default_settings");
                  const nextConfig = await invoke<AppConfig>("save_settings", { settings });
                  settingsDirtyRef.current = false;
                  setConfig(nextConfig);
                  setSettingsDraft(nextConfig.settings);
                  return t("Настройки сброшены");
                })}><RefreshCcw size={15} />{t("Вернуть по умолчанию")}</button>
              </div>
            </section>
          )}
        </div>

        {updatePrompt && (
          <div className="modalBackdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) setUpdatePrompt(null); }}>
            <div className="modal" role="dialog" aria-modal="true" aria-labelledby="update-title" onKeyDown={(event) => { if (event.key === "Escape") setUpdatePrompt(null); }}>
              <h2 id="update-title">{t("Доступна версия ")}{updatePrompt.version}</h2>
              <p>{t("Установлена ")}{updatePrompt.current}. {updatePrompt.app && updatePrompt.installable ? t("После загрузки и проверки SHA256 можно установить обновление.") : t("Сборки доступны на странице релиза.")}</p>
              <div className="actions">
                {updatePrompt.app && updatePrompt.installable ? (
                  <button className="primary" autoFocus onClick={() => {
                    setUpdatePrompt(null);
                    invoke("start_update_download", { version: updatePrompt.version })
                      .catch((error) => notify(String(error), true))
                      .finally(() => setActiveTab("about"));
                  }}>{t("Скачать обновление")}</button>
                ) : (
                  <button className="primary" autoFocus onClick={() => {
                    setUpdatePrompt(null);
                    invoke("open_external", { url: updatePrompt.page }).catch((error) => notify(String(error), true));
                  }}>{t("Открыть страницу релиза")}</button>
                )}
                <button onClick={() => setUpdatePrompt(null)}>{t("Позже")}</button>
                <button className="ghost" onClick={() => { try { localStorage.setItem("secret-tunnel.skip-version", updatePrompt.version); } catch { /* asks again next time */ } setUpdatePrompt(null); }}>{t("Пропустить эту версию")}</button>
              </div>
            </div>
          </div>
        )}
        {setupOpen && <SetupWizard serverPurchased={setupPurchased} onClose={() => { setSetupOpen(false); setSetupPurchased(false); }} onSaved={id => {
          void invoke<AppConfig>("get_config").then(next => {
            setConfig(next);
            const saved = next.servers.find(server => server.id === id);
            if (saved) { setServerDraft(saved); serverDraftDirtyRef.current = true; }
          });
          setActiveTab("servers");
        }}/>}
        {transfer && <ProfileTransfer serverId={transfer.serverId} servers={config.servers} onClose={() => setTransfer(null)} onImported={ids => {
          void invoke<AppConfig>("get_config").then(next => {
            setConfig(next);
            const imported = next.servers.find(server => server.id === ids[0]);
            if (imported) { setServerDraft(imported); serverDraftDirtyRef.current = true; }
          });
          setActiveTab("servers");
        }}/>}
        {/* Messages sit above the journal, clear of the buttons at the top right. */}
        <div className="toastLane">
          {message && (
            <div className={message.error ? "toast error" : "toast"} role={message.error ? "alert" : "status"}>
              <span>{translateMessage(message.text)}</span>
              <button onClick={() => setMessage(null)} aria-label={t("Скрыть сообщение")}>×</button>
            </div>
          )}
        </div>
        {activeTab !== "catalog" && <LogDock logs={logs} busy={busy !== null} hideAddresses={config.settings.hide_sensitive}
          onClear={() => runAction("logs", async () => { await invoke<void>("clear_logs"); return t("Журнал очищен"); })}
          onCopied={(text) => copyText(text, "logs")} />}
      </section>
    </main>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
