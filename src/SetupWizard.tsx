import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, Check, FileKey, KeyRound, LoaderCircle, Server, X } from "lucide-react";

type Step = "server" | "access" | "details" | "trust" | "finish";

export default function SetupWizard({ onClose, onSaved }: { onClose: () => void; onSaved: (id: string) => void }) {
  const [step, setStep] = useState<Step>("server");
  const [hasServer, setHasServer] = useState(true);
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
    log("Подключаемся к SSH-серверу…");
    try {
      const fingerprint = await invoke<string>("wizard_probe", { host: host.trim(), port: Number(port) });
      setPin(fingerprint); log("Сервер ответил. Получен отпечаток ключа."); setStep("trust");
    } catch (e) { setError(String(e)); log("Не удалось проверить адрес сервера."); }
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
    } catch (e) { setError(String(e)); log("Настройка остановлена. Можно исправить данные и повторить."); }
    finally { setBusy(false); }
  }

  function back() {
    setError("");
    if (step === "trust" || (step === "finish" && !saved)) { setStep("details"); setPin(""); }
    else if (step === "details") setStep("access");
    else setStep("server");
  }

  const valid = host.trim() && user.trim() && Number(port) >= 1 && Number(port) <= 65535 && Number.isInteger(Number(port)) && (hasKey ? keyPath : password);
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
      <header className="panelHead"><h2 id="setup-title">Настройка подключения</h2><button className="iconButton" aria-label="Закрыть мастер" disabled={busy} onClick={onClose}><X size={18}/></button></header>
      {step === "server" && <>
        <h3>У вас уже есть сервер?</h3>
        <p>Для подключения нужен ваш VPS с доступом по SSH.</p>
        <div className="setupChoices">
          <button aria-pressed={hasServer} onClick={() => setHasServer(true)}><Server size={20}/><span>Да, сервер уже куплен</span></button>
          <button aria-pressed={!hasServer} onClick={() => setHasServer(false)}><span>Пока нет сервера</span></button>
        </div>
        {!hasServer && <p className="setupNote">Выберите VPS с Linux x86_64 у своего хостера. После покупки понадобятся адрес, порт SSH, логин и пароль или готовый ключ. Вернитесь сюда, когда получите данные.</p>}
        <button className="primary" disabled={!hasServer} onClick={() => setStep("access")}>Продолжить</button>
      </>}
      {step === "access" && <>
        <h3>Как вы входите на сервер?</h3>
        <div className="setupChoices">
          <button aria-pressed={!hasKey} onClick={() => setHasKey(false)}><KeyRound size={20}/><span>Логин и пароль от хостера</span></button>
          <button aria-pressed={hasKey} onClick={() => setHasKey(true)}><FileKey size={20}/><span>У меня есть SSH-ключ</span></button>
        </div>
        <p>{hasKey ? "Выберите приватный ключ, который уже разрешён на сервере." : "Приложение создаст отдельный ключ, добавит его на сервер и проверит вход."}</p>
        <button className="primary" onClick={() => setStep("details")}>Продолжить</button>
      </>}
      {step === "details" && <form className="setupForm" onSubmit={e => { e.preventDefault(); if (valid) void probe(); }}>
        <h3>Данные из письма хостера</h3>
        <div className="formGrid">
          <label><span>Адрес сервера</span><input required autoComplete="off" value={host} onChange={e => setHost(e.target.value)} placeholder="IP или имя хоста" disabled={busy}/></label>
          <label><span>Порт SSH</span><input required type="number" min={1} max={65535} value={port} onChange={e => setPort(e.target.value)} disabled={busy}/></label>
          <label><span>Пользователь</span><input required autoComplete="off" value={user} onChange={e => setUser(e.target.value)} disabled={busy}/></label>
          {!hasKey && <label><span>Пароль сервера</span><input required type="password" autoComplete="off" value={password} onChange={e => setPassword(e.target.value)} disabled={busy}/></label>}
        </div>
        {hasKey && <label><span>Приватный ключ SSH</span><div className="fileField"><input readOnly value={keyPath} placeholder="Файл без .pub"/><button type="button" disabled={busy} onClick={async () => {
          try { const path = await open({ multiple: false, directory: false, title: "Приватный ключ SSH" }); if (typeof path === "string") setKeyPath(path); }
          catch(e) { setError(String(e)); }
        }}>Выбрать файл</button></div></label>}
        <label><span>Название сервера · необязательно</span><input value={name} onChange={e => setName(e.target.value)} placeholder="Мой сервер" disabled={busy}/></label>
        <button className="primary" disabled={!valid || busy} type="submit">{busy ? <><LoaderCircle size={16} className="spin"/>Проверяем адрес…</> : "Проверить адрес"}</button>
      </form>}
      {step === "trust" && <>
        <h3>Подтвердите сервер</h3>
        <p>Сверьте отпечаток ключа SSH в панели хостера. Он помогает убедиться, что вы подключаетесь к своему серверу.</p>
        <code className="setupFingerprint">{pin}</code>
        <p>{hasKey ? "Проверим вход по ключу и серверный агент." : "После подтверждения добавим публичный ключ приложения и проверим вход по нему."}</p>
        <button className="primary" onClick={() => void finish()}>Доверять и настроить</button>
      </>}
      {step === "finish" && <>
        <h3>{done ? "Подключение готово" : busy ? "Настраиваем подключение" : "Настройка не завершена"}</h3>
        {done && <p>Ключ и сервер проверены. Можно выбрать сервер для подключения, затем приложения для туннеля.</p>}
        {done ? <button className="primary" onClick={onClose}><Check size={16}/>Готово</button> : !busy && saved ? <>
          <p>Доступ по SSH сохранён. Агент передаёт TCP и UDP через туннель. Установка добавит его в домашнюю папку пользователя.</p>
          <div className="actions"><button className="primary" onClick={() => void finish(true)}>Установить агент и проверить</button><button onClick={() => void finish(false)}>Повторить проверку</button></div>
        </> : null}
      </>}
      {error && <div className="checkResult error" role="alert">{error}</div>}
      {!!logs.length && <div className="setupLog" role="log" aria-label="Ход настройки" aria-live="polite">{logs.map((line, i) => <div key={i}>{line}</div>)}</div>}
      {step !== "server" && !done && !saved && <button className="ghost" disabled={busy} onClick={back}><ArrowLeft size={15}/>Назад</button>}
    </div>
  </div>;
}
