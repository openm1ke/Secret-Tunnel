import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { Download, ExternalLink, RefreshCcw, Search, Server } from "lucide-react";
import bundledMac from "../docs/help/setup-macos.md?raw";
import bundledWindows from "../docs/help/setup-windows.md?raw";
import bundledKnowledge from "../docs/help/knowledge.md?raw";

export type Release = {
  version: string; current: string; newer: boolean; notes: string; page: string; published_at: string;
  app?: { name: string; url: string; size: number } | null; installable: boolean;
};
export type UpdateDownload = { running: boolean; version: string; received: number; total: number; ready: boolean; error: string };

/** Help lives in the repository as Markdown built from the website; the copy
 *  shipped with the app is used when GitHub is out of reach. */
const RAW = "https://raw.githubusercontent.com/openm1ke/Secret-Tunnel/main/docs/help/";
const SITE = "https://openm1ke.github.io/Secret-Tunnel/";

type Doc = "setup" | "knowledge";

export const openExternal = (url: string) => invoke("open_external", { url }).catch(() => undefined);

function formatSize(bytes: number) {
  return bytes >= 1048576 ? `${(bytes / 1048576).toFixed(1)} МБ` : `${Math.max(1, Math.round(bytes / 1024))} КБ`;
}

/** Markdown from GitHub or the app itself, cleaned before it reaches the page. */
function renderMarkdown(source: string) {
  const html = marked.parse(source, { async: false, gfm: true }) as string;
  return DOMPurify.sanitize(html, { ADD_ATTR: ["data-pins"], FORBID_TAGS: ["style", "form", "input", "button"], FORBID_ATTR: ["style"] });
}

function MarkdownView({ source, query, onShown }: { source: string; query: string; onShown?: (count: number) => void }) {
  const box = useRef<HTMLDivElement>(null);
  const html = useMemo(() => renderMarkdown(source), [source]);

  useEffect(() => {
    const root = box.current;
    if (!root) return;
    root.innerHTML = html;
    // Each article — its anchor, h2 and what follows — becomes a section the search can hide.
    let current: HTMLElement | null = null;
    const isAnchor = (element: Element | null) => element?.tagName === "P" && element.childElementCount === 1 && element.firstElementChild?.tagName === "A" && !!element.firstElementChild.id && !element.textContent?.trim();
    for (const node of [...root.childNodes]) {
      const element = node.nodeType === 1 ? (node as HTMLElement) : null;
      if (element?.tagName === "H1") { current = null; continue; }
      const opens = element && ((isAnchor(element) && element.nextElementSibling?.tagName === "H2") || (element.tagName === "H2" && !isAnchor(element.previousElementSibling)));
      if (opens) {
        current = document.createElement("section");
        current.className = "helpSection";
        root.insertBefore(current, node);
      }
      if (current) current.appendChild(node);
    }
    // Numbered marks on screenshots, as on the website.
    root.querySelectorAll<HTMLElement>("figure.shot[data-pins]").forEach((figure) => {
      figure.dataset.pins!.split(";").forEach((pair, index) => {
        const [x, y] = pair.split(",").map(Number);
        const pin = document.createElement("span");
        pin.className = "pin";
        pin.textContent = String(index + 1);
        pin.style.left = `${x}%`;
        pin.style.top = `${y}%`;
        figure.appendChild(pin);
      });
      const legend = figure.nextElementSibling;
      if (legend?.tagName === "OL") legend.classList.add("legend");
    });
    // Commands copy with one click.
    root.querySelectorAll("pre > code").forEach((code) => {
      if (code.className.includes("language-text")) return;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "copyCode";
      button.textContent = "Копировать";
      button.addEventListener("click", () => {
        navigator.clipboard?.writeText(code.textContent ?? "").then(() => {
          button.textContent = "Скопировано";
          window.setTimeout(() => { button.textContent = "Копировать"; }, 1500);
        }).catch(() => undefined);
      });
      code.parentElement!.classList.add("hasCopy");
      code.parentElement!.appendChild(button);
    });
  }, [html]);

  // Search hides the articles without the words.
  useEffect(() => {
    const root = box.current;
    if (!root) return;
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    let shown = 0;
    root.querySelectorAll<HTMLElement>(".helpSection").forEach((section) => {
      const match = words.every((word) => (section.textContent ?? "").toLowerCase().includes(word));
      section.hidden = !match;
      if (match) shown += 1;
    });
    onShown?.(words.length ? shown : -1);
  }, [query, html]);

  return (
    <div className="helpBody" ref={box} onClick={(event) => {
      const link = (event.target as HTMLElement).closest("a");
      if (!link) return;
      event.preventDefault();
      const href = link.getAttribute("href") ?? "";
      if (href.startsWith("#")) {
        box.current?.querySelector(`[id="${CSS.escape(href.slice(1))}"]`)?.scrollIntoView({ behavior: "smooth", block: "start" });
      } else if (/^https:\/\//.test(href)) {
        openExternal(href);
      }
    }} />
  );
}

export default function AboutPanel({ platform, version, checkUpdates, onCheckUpdates, servers, selectedServerId, release, onRelease }: {
  platform: string; version: string; checkUpdates: boolean; onCheckUpdates: (value: boolean) => void;
  servers: { id: string; name: string }[]; selectedServerId: string;
  release: Release | null; onRelease: (release: Release | null) => void;
}) {
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState("");
  const [download, setDownload] = useState<UpdateDownload | null>(null);
  const [installing, setInstalling] = useState(false);
  const [agentServer, setAgentServer] = useState(selectedServerId);
  const [agent, setAgent] = useState<{ state: "idle" | "busy" | "ok" | "error"; text: string; version?: string }>({ state: "idle", text: "" });
  const [doc, setDoc] = useState<Doc>("setup");
  const [query, setQuery] = useState("");
  const [found, setFound] = useState(-1);
  const windows = platform === "windows";
  const bundled: Record<Doc, string> = { setup: windows ? bundledWindows : bundledMac, knowledge: bundledKnowledge };
  const [docs, setDocs] = useState<Record<Doc, { text: string; online: boolean }>>({
    setup: { text: bundled.setup, online: false }, knowledge: { text: bundled.knowledge, online: false },
  });

  useEffect(() => { if (!agentServer && selectedServerId) setAgentServer(selectedServerId); }, [selectedServerId]);

  // The latest help from the repository; the shipped copy stays if it fails.
  useEffect(() => {
    const files: Record<Doc, string> = { setup: windows ? "setup-windows.md" : "setup-macos.md", knowledge: "knowledge.md" };
    (Object.keys(files) as Doc[]).forEach((key) => {
      fetch(RAW + files[key], { cache: "no-cache" })
        .then((response) => (response.ok ? response.text() : Promise.reject()))
        .then((text) => setDocs((current) => ({ ...current, [key]: { text, online: true } })))
        .catch(() => undefined);
    });
  }, [windows]);

  // Progress of a running download.
  useEffect(() => {
    if (!download?.running) return;
    const timer = window.setInterval(() => {
      invoke<UpdateDownload>("update_status").then(setDownload).catch(() => undefined);
    }, 400);
    return () => window.clearInterval(timer);
  }, [download?.running]);

  useEffect(() => { invoke<UpdateDownload>("update_status").then((state) => { if (state.running || state.ready || state.error) setDownload(state); }).catch(() => undefined); }, []);

  async function check() {
    setChecking(true);
    setError("");
    try {
      onRelease(await invoke<Release>("check_update"));
    } catch (problem) {
      setError(String(problem));
    } finally {
      setChecking(false);
    }
  }

  async function startDownload() {
    if (!release) return;
    setError("");
    try {
      await invoke("start_update_download", { version: release.version });
      setDownload(await invoke<UpdateDownload>("update_status"));
    } catch (problem) {
      setError(String(problem));
    }
  }

  async function install() {
    setError("");
    setInstalling(true);
    try {
      await invoke("install_update");
    } catch (problem) {
      setError(String(problem));
      setInstalling(false);
      invoke<UpdateDownload>("update_status").then(setDownload).catch(() => undefined);
    }
  }

  async function agentCheck(update: boolean) {
    if (!agentServer) return;
    setAgent({ state: "busy", text: update ? "Скачиваем агент, сверяем контрольную сумму и устанавливаем на сервер…" : "Подключаемся к серверу…" });
    try {
      const found = await invoke<string>(update ? "update_agent" : "agent_version", { serverId: agentServer });
      const installed = /^\d+\.\d+\.\d+$/.test(found) ? found : "";
      setAgent({ state: "ok", version: installed,
        text: update ? `Установлен агент ${found}. Новые подключения используют его.`
          : !installed ? "Агент на сервере не найден." : installed === version ? `Агент ${installed} — та же версия, что у приложения.` : `На сервере агент ${installed}, приложение ${version}.` });
    } catch (problem) {
      setAgent({ state: "error", text: String(problem) });
    }
  }

  const progress = download && download.total > 0 ? Math.min(1, download.received / download.total) : 0;
  const agentMatches = agent.state === "ok" && agent.version === version;

  return (
    <div className="about">
      <section className="panel aboutHead">
        <div className="aboutTitle">
          <h2>Secret Tunnel</h2>
          <span className="chip">Версия {version}</span>
        </div>
        <p className="panelIntro">Туннель для выбранных программ через ваш SSH-сервер. Открытый исходный код.</p>
        <div className="aboutLinks">
          <button className="ghost" onClick={() => openExternal(SITE)}><ExternalLink size={14} />Сайт</button>
          <button className="ghost" onClick={() => openExternal("https://github.com/openm1ke/Secret-Tunnel")}><ExternalLink size={14} />GitHub</button>
          <button className="ghost" onClick={() => openExternal("https://github.com/openm1ke/Secret-Tunnel/releases")}><ExternalLink size={14} />Все релизы</button>
          <button className="ghost" onClick={() => openExternal("https://github.com/openm1ke/Secret-Tunnel/issues")}><ExternalLink size={14} />Сообщить о проблеме</button>
        </div>
      </section>

      <div className="aboutColumns">
        <section className="panel">
          <header className="panelHead"><h2>Обновления</h2>
            <button disabled={checking || download?.running} onClick={check}><RefreshCcw size={15} />{checking ? "Проверяем…" : "Проверить"}</button>
          </header>
          {!release && !error && <p className="fieldNote">Нажмите «Проверить», чтобы узнать, есть ли новая версия.</p>}
          {release && !release.newer && <p className="updateLine ok">Установлена последняя версия {release.current}.</p>}
          {release?.newer && (
            <div className="updateBox">
              <p className="updateLine"><strong>Доступна версия {release.version}</strong>{release.published_at && ` · ${new Date(release.published_at).toLocaleDateString("ru-RU")}`}{release.app && ` · ${formatSize(release.app.size)}`}</p>
              {release.notes && (
                <details className="advanced"><summary>Что нового</summary><MarkdownView source={release.notes} query="" /></details>
              )}
              {!release.app || !release.installable ? (
                <>
                  <p className="fieldNote">{!release.app ? "Для этой системы в релизе нет сборки." : windows ? "Это портативная версия: замените папку программы новым архивом." : "Обновление работает в установленном приложении."}</p>
                  <button className="primary" onClick={() => openExternal(release.page)}><ExternalLink size={15} />Открыть страницу релиза</button>
                </>
              ) : download?.ready && download.version === release.version ? (
                <>
                  <p className="fieldNote">Скачано, контрольная сумма совпала. Приложение закроется, {windows ? "установщик обновит его" : "новая версия заменит текущую"} и запустится снова. Туннель переподключится, если включено «Подключаться при запуске».</p>
                  <button className="primary" disabled={installing} onClick={install}><Download size={15} />{installing ? "Устанавливаем…" : "Установить и перезапустить"}</button>
                </>
              ) : download?.running ? (
                <div className="progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(progress * 100)}>
                  <span style={{ width: `${progress * 100}%` }} />
                  <small>Скачиваем {formatSize(download.received)} из {formatSize(download.total)}</small>
                </div>
              ) : (
                <button className="primary" onClick={startDownload}><Download size={15} />Скачать обновление</button>
              )}
            </div>
          )}
          {(error || download?.error) && <p className="updateLine bad">{error || download?.error}</p>}
          <label className="checkboxLine"><input type="checkbox" checked={checkUpdates} onChange={(event) => onCheckUpdates(event.target.checked)} />Проверять обновления при запуске</label>
          <p className="fieldNote">Приложение спрашивает GitHub о последнем релизе. Файлы скачиваются только с вашего согласия и проверяются по SHA256 из релиза.</p>
        </section>

        <section className="panel">
          <header className="panelHead"><h2>Агент на сервере</h2></header>
          <p className="fieldNote">Агент должен быть той же версии, что приложение. Обновление скачивается здесь, проверяется по SHA256 и передаётся на сервер по SSH — серверу не нужен доступ к GitHub.</p>
          <label><span>Сервер</span>
            <select value={agentServer} disabled={agent.state === "busy"} onChange={(event) => { setAgentServer(event.target.value); setAgent({ state: "idle", text: "" }); }}>
              {servers.map((server) => <option key={server.id} value={server.id}>{server.name}</option>)}
            </select>
          </label>
          <div className="actions">
            <button disabled={!agentServer || agent.state === "busy"} onClick={() => agentCheck(false)}><Server size={15} />Проверить версию</button>
            <button className={agentMatches ? undefined : "primary"} disabled={!agentServer || agent.state === "busy" || version === "—"} onClick={() => agentCheck(true)}><Download size={15} />{agentMatches ? "Переустановить агент" : `Установить агент ${version}`}</button>
          </div>
          {agent.state !== "idle" && <div className={`checkResult ${agent.state === "busy" ? "running" : agent.state}`}><strong>{agent.state === "busy" ? "Работаем…" : agent.state === "ok" ? "Готово" : "Не получилось"}</strong><span>{agent.text}</span></div>}
        </section>
      </div>

      <section className="panel help">
        <header className="panelHead">
          <div className="segmented" role="group" aria-label="Справка">
            <button className={doc === "setup" ? "on" : ""} onClick={() => { setDoc("setup"); setQuery(""); }}>Установка</button>
            <button className={doc === "knowledge" ? "on" : ""} onClick={() => { setDoc("knowledge"); setQuery(""); }}>База знаний</button>
          </div>
          <label className="search helpSearch"><Search size={15} />
            <input aria-label="Поиск по справке" placeholder="Поиск: ключ, порт, Discord…" value={query} onChange={(event) => setQuery(event.target.value)} />
          </label>
          <small className="helpSource">{docs[doc].online ? "Свежая версия с GitHub" : "Копия, встроенная в приложение"}</small>
        </header>
        <MarkdownView source={docs[doc].text} query={query} onShown={setFound} />
        {found === 0 && <p className="emptyHint helpNothing">Ничего не нашлось. Попробуйте другое слово или откройте <button className="linkButton" onClick={() => openExternal(SITE + (doc === "setup" ? "guide.html" : "knowledge.html"))}>страницу на сайте</button>.</p>}
      </section>
    </div>
  );
}
