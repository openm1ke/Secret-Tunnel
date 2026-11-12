import React, { useMemo, useState } from "react";
import { ArrowDown, ArrowUp, Trash2 } from "lucide-react";
import { Flag, countryName, formatBytes } from "./components";

export type AppUsage = { name: string; rx: number; tx: number; flows: number };
export type SessionRecord = {
  id: string;
  started_at: number;
  ended_at: number;
  server: string;
  route: string[];
  country: string;
  rx: number;
  tx: number;
  apps: AppUsage[];
  active: boolean;
};
export type KnownApp = { name: string; executable: string; icon?: string; patterns?: string[] };

const UNATTRIBUTED = "Без перехвата (SOCKS)";
type Period = "today" | "week" | "month" | "all";
const periods: { id: Period; label: string }[] = [
  { id: "today", label: "Сегодня" },
  { id: "week", label: "7 дней" },
  { id: "month", label: "30 дней" },
  { id: "all", label: "Всё время" },
];

function periodStart(period: Period) {
  const now = new Date();
  if (period === "today") return new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() / 1000;
  if (period === "week") return Date.now() / 1000 - 7 * 86400;
  if (period === "month") return Date.now() / 1000 - 30 * 86400;
  return 0;
}

export function formatDuration(seconds: number) {
  const s = Math.max(0, Math.round(seconds));
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  if (days) return `${days} д ${hours} ч`;
  if (hours) return `${hours} ч ${minutes} мин`;
  if (minutes) return `${minutes} мин`;
  return `${s} с`;
}

function sessionWhen(session: SessionRecord) {
  const start = new Date(session.started_at * 1000);
  const end = new Date(session.ended_at * 1000);
  const day = start.toLocaleDateString("ru-RU", { day: "numeric", month: "short" });
  const time = (date: Date) => date.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
  return `${day}, ${time(start)}–${session.active ? "сейчас" : time(end)}`;
}

/** Engine process names ("Discord Helper (Renderer)", "firefox") matched to
 * the installed application they belong to, the longest name first. */
export function knownAppMatcher<T extends KnownApp>(known: T[]) {
  const entries = known
    .map((app) => ({ app, keys: [app.name.toLowerCase(), (app.executable.split(/[\\/]/).pop() || "").replace(/\.(app|exe)$/i, "").toLowerCase()] }))
    .sort((a, b) => b.app.name.length - a.app.name.length);
  const find = (name: string) => {
    const lower = name.toLowerCase().replace(/\.exe$/, "");
    return entries.find(({ keys }) => keys.some((key) => key && (lower === key || lower.startsWith(`${key} `) || lower.startsWith(`${key}.`))))?.app
      // A program inside the app signed under its own name: ChatGPT's `codex`.
      ?? entries.find(({ app }) => app.patterns?.some((pattern) => pattern.toLowerCase() === lower))?.app;
  };
  return (name: string): T | undefined => find(name) ?? find(parentName(name));
}

/** Child processes that do not carry their application's name. */
function parentName(name: string) {
  return /^plugin-container$/i.test(name) ? "Firefox" : name.replace(/ Helper( \(.*\))?$/, "").replace(/ \([^()]*\)$/, "");
}

function useAppResolver(known: KnownApp[]) {
  return useMemo(() => {
    const match = knownAppMatcher(known);
    return (name: string): { label: string; icon?: string } => {
      if (name === UNATTRIBUTED) return { label: "Без перехвата" };
      const app = match(name);
      return app ? { label: app.name, icon: app.icon } : { label: parentName(name) };
    };
  }, [known]);
}

function AppGlyph({ label, icon }: { label: string; icon?: string }) {
  if (icon) return <img className="appIcon small" src={icon} alt="" />;
  return <span className="appIcon small letter" aria-hidden="true">{label.slice(0, 1).toUpperCase()}</span>;
}

export default function StatsPanel({ sessions, known, onClear }: { sessions: SessionRecord[]; known: KnownApp[]; onClear: () => void }) {
  const [period, setPeriod] = useState<Period>("week");
  const [confirming, setConfirming] = useState(false);
  const resolve = useAppResolver(known);
  const from = periodStart(period);
  const shown = sessions.filter((session) => session.ended_at >= from || session.active).sort((a, b) => b.started_at - a.started_at);
  const totals = shown.reduce((sum, s) => ({ rx: sum.rx + s.rx, tx: sum.tx + s.tx, time: sum.time + Math.max(0, s.ended_at - s.started_at) }), { rx: 0, tx: 0, time: 0 });

  const apps = useMemo(() => {
    const byApp = new Map<string, { label: string; icon?: string; rx: number; tx: number; flows: number; sessions: number; raw: Set<string> }>();
    for (const session of shown) {
      const seen = new Set<string>();
      for (const usage of session.apps) {
        const { label, icon } = resolve(usage.name);
        const entry = byApp.get(label) ?? { label, icon, rx: 0, tx: 0, flows: 0, sessions: 0, raw: new Set<string>() };
        entry.rx += usage.rx;
        entry.tx += usage.tx;
        entry.flows += usage.flows;
        entry.raw.add(usage.name);
        if (!seen.has(label)) {
          entry.sessions += 1;
          seen.add(label);
        }
        byApp.set(label, entry);
      }
    }
    return [...byApp.values()].sort((a, b) => b.rx + b.tx - (a.rx + a.tx));
  }, [shown.map((s) => `${s.id}:${s.rx}:${s.tx}`).join("|"), resolve]);
  const topApp = apps.find((app) => app.label !== "Без перехвата");
  const maxApp = Math.max(1, ...apps.map((app) => app.rx + app.tx));

  return (
    <section className="statsPage">
      <header className="panelHead">
        <div>
          <h2>Статистика</h2>
          <p className="panelIntro">Сеансы туннеля и трафик, который через них прошёл. Хранится только на этом компьютере: время, серверы, объёмы и названия программ.</p>
        </div>
        <div className="segmented" role="group" aria-label="Период">
          {periods.map((item) => (
            <button key={item.id} className={period === item.id ? "on" : ""} onClick={() => setPeriod(item.id)}>{item.label}</button>
          ))}
        </div>
      </header>

      <div className="metrics statsTotals">
        <div className="metric"><span>Скачано</span><strong className="rx">{formatBytes(totals.rx)}</strong></div>
        <div className="metric"><span>Отправлено</span><strong className="tx">{formatBytes(totals.tx)}</strong></div>
        <div className="metric"><span>Сеансов</span><strong>{shown.length}</strong></div>
        <div className="metric"><span>Время в туннеле</span><strong>{formatDuration(totals.time)}</strong></div>
      </div>

      <div className="statsColumns">
        <section className="panel">
          <header className="panelHead">
            <h3>Приложения</h3>
            {topApp && <span className="chip">Больше всего: {topApp.label}</span>}
          </header>
          {apps.length === 0 ? (
            <p className="emptyHint">За этот период трафика по приложениям нет. Он появится, когда программы пойдут через туннель: с перехватом приложений или через прокси в их настройках.</p>
          ) : (
            <table className="statsTable">
              <thead><tr><th>Программа</th><th className="num">Скачано</th><th className="num">Отправлено</th><th className="num">Соединений</th></tr></thead>
              <tbody>
                {apps.map((app) => (
                  <tr key={app.label} title={[...app.raw].join(", ")}>
                    <td>
                      <span className="statsApp">
                        <AppGlyph label={app.label} icon={app.icon} />
                        <span className="statsAppText">
                          <strong>{app.label}</strong>
                          <i className="share" style={{ width: `${Math.max(2, ((app.rx + app.tx) / maxApp) * 100)}%` }} aria-hidden="true" />
                          {app.label === "Без перехвата" && <small>программы с прокси в своих настройках: браузер, curl</small>}
                        </span>
                      </span>
                    </td>
                    <td className="num rx">{formatBytes(app.rx)}</td>
                    <td className="num tx">{formatBytes(app.tx)}</td>
                    <td className="num">{app.flows}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>

        <section className="panel">
          <header className="panelHead">
            <h3>Сеансы</h3>
            {sessions.some((session) => !session.active) && (
              confirming ? (
                <span className="confirmRow">
                  <span>Удалить историю?</span>
                  <button className="danger" onClick={() => { setConfirming(false); onClear(); }}><Trash2 size={14} />Удалить</button>
                  <button className="ghost" onClick={() => setConfirming(false)}>Отмена</button>
                </span>
              ) : (
                <button className="ghost" onClick={() => setConfirming(true)}><Trash2 size={14} />Очистить историю</button>
              )
            )}
          </header>
          {shown.length === 0 ? (
            <p className="emptyHint">За этот период сеансов не было.</p>
          ) : (
            <ol className="sessionList">
              {shown.map((session) => {
                const grouped = new Map<string, { label: string; icon?: string; rx: number; tx: number }>();
                for (const usage of session.apps) {
                  if (usage.name === UNATTRIBUTED) continue;
                  const { label, icon } = resolve(usage.name);
                  const entry = grouped.get(label) ?? { label, icon, rx: 0, tx: 0 };
                  entry.rx += usage.rx;
                  entry.tx += usage.tx;
                  grouped.set(label, entry);
                }
                const top = [...grouped.values()].sort((a, b) => b.rx + b.tx - (a.rx + a.tx)).slice(0, 3);
                const route = session.route.length > 1 ? session.route.join(" → ") : session.server;
                const country = countryName(session.country, "");
                return (
                  <li key={session.id} className={session.active ? "session live" : "session"}>
                    <div className="sessionTop">
                      <span className="sessionWhen">{sessionWhen(session)}</span>
                      {session.active && <span className="chip good">идёт сейчас</span>}
                      <span className="sessionDuration">{formatDuration(session.ended_at - session.started_at)}</span>
                    </div>
                    <div className="sessionRoute">
                      {route}{session.country && <> · <Flag code={session.country} label={country} /> {country}</>}
                    </div>
                    <div className="sessionBottom">
                      <span className="totals"><span><ArrowDown size={12} />{formatBytes(session.rx)}</span><span><ArrowUp size={12} />{formatBytes(session.tx)}</span></span>
                      <span className="sessionApps">
                        {top.map((app) => (
                          <span key={app.label} className="sessionApp" title={`${app.label}: ↓ ${formatBytes(app.rx)} ↑ ${formatBytes(app.tx)}`}><AppGlyph label={app.label} icon={app.icon} />{app.label}</span>
                        ))}
                      </span>
                    </div>
                  </li>
                );
              })}
            </ol>
          )}
        </section>
      </div>
    </section>
  );
}
