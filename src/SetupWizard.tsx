import { t, translateMessage, formatLocale, getLanguage } from "./i18n";
import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, Check, FileKey, KeyRound, LoaderCircle, Server, X } from "lucide-react";
import HostingCatalog from "./HostingCatalog";

type Step = "server" | "access" | "details" | "trust" | "finish";

export default function SetupWizard({ onClose, onSaved, serverPurchased = false }: { onClose: () => void; onSaved: (id: string) => void; serverPurchased?: boolean }) {
  const [step, setStep] = useState<Step>(serverPurchased ? "access" : "server");
  const [hosting, setHosting] = useState(false);
  const [hasKey, setHasKey] = useState(false);
  const [name, setName] = useState("");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [user, setUser] = useState("root");
  const [password, setPassword] = useState("");
  const [keyPath, setKeyPath] = useState("");
  const [pin, setPin] = useState("");
  const [busy, setBusy] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState("");
  const [done, setDone] = useState(false);
  const [id] = useState(() => `setup-${crypto.randomUUID()}`);
  const dialog = useRef<HTMLDivElement>(null);
  const previousFocus = useRef(document.activeElement as HTMLElement | null);
  const log = (text: string) => setLogs(lines => [...lines.slice(-24), text]);
  const channel = () => { const messages = new Channel<string>(); messages.onmessage = log; return messages; };

  useEffect(() => {
    dialog.current?.focus();
    return () => previousFocus.current?.focus();
  }, []);

  async function probe() {
    setError(""); setBusy(true); setPin("");
    log(t("Подключаемся к SSH-серверу…"));
    try {
      const fingerprint = await invoke<string>("wizard_probe", { host: host.trim(), port: Number(port) });
      setPin(fingerprint); log(t("Сервер ответил. Получен отпечаток ключа.")); setStep("trust");
    } catch (e) { setError(String(e)); log(t("Не удалось проверить адрес сервера.")); }
    finally { setBusy(false); }
  }

  async function finish(install = false) {
    setError(""); setBusy(true); setStep("finish");
    try {
      let serverId = saved;
      if (!serverId) {
        serverId = await invoke<string>("wizard_access", { request: {
          id, name, host: host.trim(), port: Number(port), user: user.trim(), fingerprint: pin,
          key_path: hasKey ? keyPath : null, password: hasKey ? null : password,
        }, progress: channel() });
        setSaved(serverId); setPassword(""); onSaved(serverId);
      }
      await invoke<string>("wizard_finish", { serverId, install, progress: channel() });
      setDone(true);
    } catch (e) { setError(String(e)); log(t("Настройка остановлена. Можно исправить данные и повторить.")); }
    finally { setBusy(false); }
  }

  function back() {
    setError("");
    if (step === "trust" || (step === "finish" && !saved)) { setStep("details"); setPin(""); }
    else if (step === "details") setStep("access");
    else setStep("server");
  }

  const valid = host.trim() && user.trim() && Number(port) >= 1 && Number(port) <= 65535 && Number.isInteger(Number(port)) && (hasKey ? keyPath : password);
  if (hosting) return <HostingCatalog overlay onClose={() => setHosting(false)} onPurchased={() => { setHosting(false); setStep("access"); }}/>;
  return <div className="modalBackdrop">
    <div className="modal setupWizard" role="dialog" aria-modal="true" aria-labelledby="setup-title" ref={dialog} tabIndex={-1}
      onKeyDown={e => {
        if (e.key === "Escape" && !busy) onClose();
        if (e.key === "Tab") {
          const nodes = [...(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), [href]') ?? [])];
          const first = nodes[0], last = nodes[nodes.length - 1];
          if (e.shiftKey && (document.activeElement === first || document.activeElement === dialog.current)) { e.preventDefault(); last?.focus(); }
          else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
        }
      }}>
      <header className="panelHead"><h2 id="setup-title">{t("Настройка подключения")}</h2><button className="iconButton" aria-label={t("Закрыть мастер")} disabled={busy} onClick={onClose}><X size={18}/></button></header>
      {step === "server" && <>
        <h3>{t("У вас уже есть сервер?")}</h3>
        <p>{t("Для подключения нужен ваш VPS с доступом по SSH.")}</p>
        <div className="setupChoices">
          <button onClick={() => setStep("access")}><Server size={20}/><span>{t("Да, сервер уже куплен")}</span></button>
          <button onClick={() => setHosting(true)}><span>{t("Пока нет сервера")}</span></button>
        </div>
      </>}
      {step === "access" && <>
        <h3>{t("Как вы входите на сервер?")}</h3>
        <div className="setupChoices">
          <button aria-pressed={!hasKey} onClick={() => setHasKey(false)}><KeyRound size={20}/><span>{t("Логин и пароль от хостера")}</span></button>
          <button aria-pressed={hasKey} onClick={() => setHasKey(true)}><FileKey size={20}/><span>{t("У меня есть SSH-ключ")}</span></button>
        </div>
        <p>{hasKey ? t("Выберите приватный ключ, который уже разрешён на сервере.") : t("Приложение создаст отдельный ключ, добавит его на сервер и проверит вход.")}</p>
        <button className="primary" onClick={() => setStep("details")}>{t("Продолжить")}</button>
      </>}
      {step === "details" && <form className="setupForm" onSubmit={e => { e.preventDefault(); if (valid) void probe(); }}>
        <h3>{t("Данные из письма хостера")}</h3>
        <div className="formGrid">
          <label><span>{t("Адрес сервера")}</span><input required autoComplete="off" value={host} onChange={e => setHost(e.target.value)} placeholder={t("IP или имя хоста")} disabled={busy}/></label>
          <label><span>{t("Порт SSH")}</span><input required type="number" min={1} max={65535} value={port} onChange={e => setPort(e.target.value)} disabled={busy}/></label>
          <label><span>{t("Пользователь")}</span><input required autoComplete="off" value={user} onChange={e => setUser(e.target.value)} disabled={busy}/></label>
          {!hasKey && <label><span>{t("Пароль сервера")}</span><input required type="password" autoComplete="off" value={password} onChange={e => setPassword(e.target.value)} disabled={busy}/></label>}
        </div>
        {hasKey && <label><span>{t("Приватный ключ SSH")}</span><div className="fileField"><input readOnly value={keyPath} placeholder={t("Файл без .pub")}/><button type="button" disabled={busy} onClick={async () => {
          try { const path = await open({ multiple: false, directory: false, title: t("Приватный ключ SSH") }); if (typeof path === "string") setKeyPath(path); }
          catch(e) { setError(String(e)); }
        }}>{t("Выбрать файл")}</button></div></label>}
        <label><span>{t("Название сервера · необязательно")}</span><input value={name} onChange={e => setName(e.target.value)} placeholder={t("Мой сервер")} disabled={busy}/></label>
        <button className="primary" disabled={!valid || busy} type="submit">{busy ? <><LoaderCircle size={16} className="spin"/>{t("Проверяем адрес…")}</> : t("Проверить адрес")}</button>
      </form>}
      {step === "trust" && <>
        <h3>{t("Подтвердите сервер")}</h3>
        <p>{t("Сверьте отпечаток ключа SSH в панели хостера. Он помогает убедиться, что вы подключаетесь к своему серверу.")}</p>
        <code className="setupFingerprint">{pin}</code>
        <p>{hasKey ? t("Проверим вход по ключу и серверный агент.") : t("После подтверждения добавим публичный ключ приложения и проверим вход по нему.")}</p>
        <button className="primary" onClick={() => void finish()}>{t("Доверять и настроить")}</button>
      </>}
      {step === "finish" && <>
        <h3>{done ? t("Подключение готово") : busy ? t("Настраиваем подключение") : t("Настройка не завершена")}</h3>
        {done && <p>{t("Ключ и сервер проверены. Можно выбрать сервер для подключения, затем приложения для туннеля.")}</p>}
        {done ? <button className="primary" onClick={onClose}><Check size={16}/>{t("Готово")}</button> : !busy && saved ? <>
          <p>{t("Доступ по SSH сохранён. Агент передаёт TCP и UDP через туннель. Установка добавит его в домашнюю папку пользователя.")}</p>
          <div className="actions"><button className="primary" onClick={() => void finish(true)}>{t("Установить агент и проверить")}</button><button onClick={() => void finish(false)}>{t("Повторить проверку")}</button></div>
        </> : null}
      </>}
      {error && <div className="checkResult error" role="alert">{translateMessage(error)}</div>}
      {!!logs.length && <div className="setupLog" role="log" aria-label={t("Ход настройки")} aria-live="polite">{logs.map((line, i) => <div key={i}>{translateMessage(line)}</div>)}</div>}
      {step !== "server" && !done && !saved && <button className="ghost" disabled={busy} onClick={back}><ArrowLeft size={15}/>{t("Назад")}</button>}
    </div>
  </div>;
}
