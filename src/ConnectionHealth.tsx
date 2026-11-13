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
  onCheck: () => void;
};

export default function ConnectionHealth({
  ssh, connected, interception, interceptionError, directWarning, checks, onCheck,
}: Props) {
  const report = connected ? checks.report : null;
  const running = connected && checks.running;
  const results = [
    ["TCP", report?.tcp], ["UDP", report?.udp], ["DNS", report?.dns], ["IPv6", report?.ipv6],
  ] as const;

  return (
    <section className="connectionHealth" aria-label="Состояние соединения">
      <div className="connectionLayers" role="status">
        <span>SSH <strong>{ssh}</strong></span>
        <span>Перехват <strong>{interception}</strong></span>
      </div>
      {interceptionError && <p className="connectionError">{interceptionError}</p>}
      {directWarning && <p className="fieldNote">{directWarning}</p>}
      <div className="connectionProbes">
        <dl aria-label="Проверки через SOCKS">
          {results.map(([label, outcome]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd className={outcome?.status}>
                {running ? "Проверяем…" : outcome?.status === "passed" ? `${outcome.ms} мс`
                  : outcome?.status === "failed" ? outcome.detail
                  : outcome?.status === "unsupported" ? "Недоступно в OpenSSH" : "Нет проверки"}
              </dd>
            </div>
          ))}
        </dl>
        <button className="ghost" disabled={!connected || running} onClick={onCheck}>
          <RefreshCcw size={15} />{running ? "Проверяем…" : "Проверить соединение"}
        </button>
      </div>
      <p className="fieldNote">
        TCP и IPv6 проверяют открытие соединения, UDP и DNS используют запрос к 1.1.1.1.
        Проверки идут через локальный SOCKS и не проверяют захват приложений или системный DNS.
        {report && ` Проверено в ${new Date(report.checked_at * 1000).toLocaleTimeString("ru-RU")}.`}
      </p>
    </section>
  );
}
