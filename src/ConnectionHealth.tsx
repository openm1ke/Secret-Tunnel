import { t, translateMessage, formatLocale, getLanguage } from "./i18n";
import { RefreshCcw } from "lucide-react";

type Outcome =
  | { status: "passed"; ms: number }
  | { status: "failed"; detail: string }
  | { status: "unsupported" };

export type ConnectionChecks = {
  running: boolean;
  report: { tcp: Outcome; udp: Outcome; dns: Outcome; ipv6: Outcome; checked_at: number } | null;
};

type Props = {
  ssh: string;
  connected: boolean;
  interception: string;
  interceptionError: string;
  directWarning: string;
  checks: ConnectionChecks;
  priorityUdp?: { separate: boolean; error: string | null } | null;
  interceptionRunning: boolean;
  onCheck: () => void;
};

export default function ConnectionHealth({
  ssh, connected, interception, interceptionError, directWarning, checks, priorityUdp, interceptionRunning, onCheck,
}: Props) {
  const report = connected ? checks.report : null;
  const running = connected && checks.running;
  const results = [
    ["TCP", report?.tcp], ["UDP", report?.udp], ["DNS", report?.dns], ["IPv6", report?.ipv6],
  ] as const;

  return (
    <section className="connectionHealth" aria-label={t("Состояние соединения")}>
      <div className="connectionLayers" role="status">
        <span>SSH <strong>{ssh}</strong></span>
        <span>{t("Перехват ")}<strong>{interception}</strong></span>
        {priorityUdp && <span>{t("UDP отмеченных приложений ")}<strong>{!interceptionRunning ? t("Включите перехват")
          : priorityUdp.separate ? t("Отдельный SSH") : t("Основной SSH")}</strong></span>}
      </div>
      {priorityUdp?.error && <p className="fieldNote" title={priorityUdp.error}>{t("Второй SSH недоступен. При включённом перехвате новые UDP-соединения отмеченных приложений используют основной туннель.")}</p>}
      {interceptionError && <p className="connectionError">{translateMessage(interceptionError)}</p>}
      {directWarning && <p className="fieldNote">{translateMessage(directWarning)}</p>}
      <div className="connectionProbes">
        <dl aria-label={t("Проверки через SOCKS")}>
          {results.map(([label, outcome]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd className={outcome?.status}>
                {running ? t("Проверяем…") : outcome?.status === "passed" ? t("{0} мс", outcome.ms)
                  : outcome?.status === "failed" ? translateMessage(outcome.detail)
                  : outcome?.status === "unsupported" ? t("Недоступно в OpenSSH") : t("Нет проверки")}
              </dd>
            </div>
          ))}
        </dl>
        <button className="ghost" disabled={!connected || running} onClick={onCheck}>
          <RefreshCcw size={15} />{running ? t("Проверяем…") : t("Проверить соединение")}
        </button>
      </div>
      <p className="fieldNote">
        {t("TCP и IPv6 проверяют открытие соединения, UDP и DNS используют запрос к 1.1.1.1. Проверки идут через локальный SOCKS и не проверяют захват приложений или системный DNS.")}{report && t(" Проверено в {0}.", new Date(report.checked_at * 1000).toLocaleTimeString(formatLocale()))}
      </p>
    </section>
  );
}
