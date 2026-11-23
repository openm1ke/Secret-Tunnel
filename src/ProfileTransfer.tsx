import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Check, Copy, Download, LoaderCircle, X } from "lucide-react";

type Exported = { link: string; code: string; qr_svg: string | null; server_count: number; skipped: string[] };
type Preview = { roots: string[]; servers: { id: string; name: string; host: string; port: number; user: string; host_key_sha256: string; jump_id: string | null }[] };
type Props = { serverId: string | null; servers: { id: string; name: string }[]; onClose: () => void; onImported: (ids: string[]) => void };

export default function ProfileTransfer({ serverId, servers, onClose, onImported }: Props) {
  const [all, setAll] = useState(false);
  const [exported, setExported] = useState<Exported | null>(null);
  const [tab, setTab] = useState<"qr" | "link" | "file">("qr");
  const [showCode, setShowCode] = useState(false);
  const [payload, setPayload] = useState("");
  const [code, setCode] = useState("");
  const [preview, setPreview] = useState<Preview | null>(null);
  const [done, setDone] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const dialog = useRef<HTMLDivElement>(null);
  const previousFocus = useRef(document.activeElement as HTMLElement | null);
  useEffect(() => { dialog.current?.focus(); return () => previousFocus.current?.focus(); }, []);
  async function action(work: () => Promise<void>) {
    setError(""); setNotice(""); setBusy(true);
    try { await work(); } catch (error) { setError(String(error)); } finally { setBusy(false); }
  }
  async function copy(text: string, label: string) {
    try { if (navigator.clipboard?.writeText) { await navigator.clipboard.writeText(text); setNotice(label); return; } } catch { /* WKWebView can require the selection-based API. */ }
    const field = document.createElement("textarea"); field.value = text; field.readOnly = true;
    field.style.position = "fixed"; field.style.opacity = "0"; document.body.appendChild(field);
    try { field.select(); if (!document.execCommand("copy")) throw Error("Не удалось скопировать. Выделите текст вручную."); setNotice(label); }
    finally { field.remove(); }
  }
  return <div className="modalBackdrop"><div className="modal setupWizard transferDialog" role="dialog" aria-modal="true" aria-labelledby="transfer-title" tabIndex={-1} ref={dialog}
    onKeyDown={event => {
      if (event.key === "Escape" && !busy) onClose();
      if (event.key === "Tab") {
        const nodes = [...(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),textarea:not(:disabled),[href]') ?? [])];
        if (event.shiftKey && (document.activeElement === nodes[0] || document.activeElement === dialog.current)) { event.preventDefault(); nodes[nodes.length - 1]?.focus(); }
        else if (!event.shiftKey && document.activeElement === nodes[nodes.length - 1]) { event.preventDefault(); nodes[0]?.focus(); }
      }
    }}>
    <header className="panelHead"><h2 id="transfer-title">{serverId ? "Поделиться серверами" : "Импортировать серверы"}</h2><button className="iconButton" aria-label="Закрыть перенос" disabled={busy} onClick={onClose}><X size={18}/></button></header>
    {serverId ? <>
      {!exported ? <>
        <p>Передайте сохранённое подключение на телефон, компьютер или Android TV. Оно включает ключ доступа: получатель сможет войти на ваш сервер.</p>
        <strong>{servers.find(server => server.id === serverId)?.name}</strong>
        <label className="checkboxLine"><input type="checkbox" checked={all} disabled={busy} onChange={event => setAll(event.target.checked)}/>Все настроенные серверы</label>
        <p>Промежуточные серверы цепочки добавятся автоматически. Настройки приложений останутся на этом устройстве.</p>
        <button className="primary" disabled={busy} onClick={() => void action(async () => {
          setExported(await invoke<Exported>("transfer_export", { serverIds: all ? servers.map(server => server.id) : [serverId] }));
        })}>Подготовить перенос</button>
      </> : <>
        <p>Серверов в переносе: {exported.server_count}. На принимающем устройстве откройте «Серверы» → «Импортировать».</p>
        {!!exported.skipped?.length && <p className="setupNote">Не добавлены незаполненные профили или профили без файла ключа: {exported.skipped.join(", ")}.</p>}
        <div className="transferTabs" role="tablist" aria-label="Способ переноса">
          {([["qr", "QR-код"], ["link", "Ссылка"], ["file", "Файл"]] as const).map(([value, title]) => <button role="tab" aria-selected={tab === value} key={value} onClick={() => setTab(value)}>{title}</button>)}
        </div>
        {tab === "qr" && (exported.qr_svg ? <div className="transferQr"><img width="320" height="320" src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(exported.qr_svg)}`} alt="Зашифрованные настройки серверов"/><p>В мобильном приложении нажмите «Сканировать QR-код».</p></div> : <p className="setupNote">Эти настройки слишком велики для одного QR-кода. Передайте ссылку или файл.</p>)}
        {tab === "link" && <><label><span>Зашифрованная ссылка</span><textarea className="transferPayload" readOnly rows={3} value={exported.link}/></label><button disabled={busy} onClick={() => void action(() => copy(exported.link, "Ссылка скопирована"))}><Copy size={16}/>Скопировать ссылку</button></>}
        {tab === "file" && <><p>Сохраните файл .stprofile на флешку. На Android TV выберите «Серверы» → «Импортировать» → «Открыть файл».</p><button className="primary" disabled={busy} onClick={() => void action(async () => {
          const path = await save({ defaultPath: "SecretTunnel-servers.stprofile", filters: [{ name: "Серверы Secret Tunnel", extensions: ["stprofile"] }] });
          if (path) { await invoke("transfer_write_file", { path, payload: exported.link }); setNotice("Зашифрованный файл сохранён"); }
        })}><Download size={16}/>Сохранить файл</button></>}
        <div className="transferCode">
          <strong>Код переноса</strong><p>Сохраните его отдельно от ссылки или файла. После закрытия окна код этого переноса нельзя восстановить.</p>
          {showCode && <code className="setupFingerprint">{exported.code}</code>}
          <div className="actions"><button onClick={() => setShowCode(!showCode)}>{showCode ? "Скрыть код" : "Показать код"}</button><button onClick={() => void action(() => copy(exported.code, "Код переноса скопирован"))}><Copy size={16}/>Скопировать код</button></div>
        </div>
      </>}
    </> : done ? <><h3>Серверы добавлены</h3><p>Существующие подключения сохранены. Выберите нужный сервер и проверьте соединение.</p><button className="primary" onClick={onClose}><Check size={16}/>Готово</button></> : <>
      {!preview ? <>
        <p>Вставьте ссылку или откройте зашифрованный файл .stprofile. Код переноса получите у отправителя отдельно.</p>
        <button disabled={busy} onClick={() => void action(async () => {
          const path = await open({ multiple: false, filters: [{ name: "Серверы Secret Tunnel", extensions: ["stprofile"] }] });
          if (typeof path === "string") setPayload(await invoke<string>("transfer_read_file", { path }));
        })}>Открыть файл</button>
        <label><span>Ссылка переноса</span><textarea className="transferPayload" autoComplete="off" spellCheck={false} rows={3} maxLength={1048576} value={payload} disabled={busy} onChange={event => setPayload(event.target.value)}/></label>
        <label><span>Код переноса</span><input className="transferCodeInput" autoComplete="off" spellCheck={false} maxLength={128} value={code} disabled={busy} onChange={event => setCode(event.target.value)} placeholder="ABCD-EFGH-JKLM-NPQR-STUV"/></label>
        <button className="primary" disabled={busy || !payload.trim() || !code.trim()} onClick={() => void action(async () => setPreview(await invoke<Preview>("transfer_preview", { payload, code })))}>Расшифровать и проверить</button>
      </> : <>
        <h3>Будут добавлены серверы</h3>
        <div className="transferPreview">{preview.servers.map(server => <div key={server.id}><strong>{server.name}</strong><p>{server.user}@{server.host}:{server.port}{server.jump_id && ` · через ${preview.servers.find(hop => hop.id === server.jump_id)?.name}`}</p><details><summary>Отпечаток сервера</summary><code>{server.host_key_sha256}</code></details></div>)}</div>
        <p>Добавляйте только настройки от доверенного отправителя. Существующие серверы не будут заменены.</p>
        <button className="primary" disabled={busy} onClick={() => void action(async () => {
          const ids = await invoke<string[]>("transfer_import", { payload, code });
          setPayload(""); setCode(""); setPreview(null); setDone(true); onImported(ids);
        })}>Добавить серверы</button>
        <button disabled={busy} onClick={() => setPreview(null)}>Назад</button>
      </>}
    </>}
    {busy && <div role="status"><LoaderCircle className="spin" size={16}/> Обрабатываем настройки…</div>}
    {error && <div className="checkResult error" role="alert">{error}</div>}
    {notice && <p role="status">{notice}</p>}
  </div></div>;
}
