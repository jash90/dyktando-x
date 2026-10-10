import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "../components/Markdown";
import { api, formatBytes, type UpdateInfo, type UpdateProgress } from "../api";
import { t as translate, useT } from "../i18n";

const RELEASES = "https://github.com/jash90/dyktando-x/releases/latest";

type Status =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "available"; info: UpdateInfo }
  | { kind: "installing"; info: UpdateInfo; done: number; total: number }
  | { kind: "error"; message: string; info?: UpdateInfo };

export default function UpdateCard() {
  const { t } = useT();
  const [version, setVersion] = useState("");
  const [status, setStatus] = useState<Status>({ kind: "idle" });

  useEffect(() => {
    getVersion().then(setVersion);
    api.knownUpdate().then((info) => info && setStatus({ kind: "available", info }));
    // Result of the check at startup or from the tray.
    const unStatus = listen<UpdateInfo | null>("update-status", (e) => {
      setStatus((s) => (s.kind === "installing" ? s : e.payload ? { kind: "available", info: e.payload } : { kind: "latest" }));
    });
    const unProgress = listen<UpdateProgress>("update-progress", (e) => {
      const p = e.payload;
      setStatus((s) => (s.kind === "installing" && !p.finished ? { ...s, done: p.done, total: p.total } : s));
    });
    const unFailed = listen<string>("update-check-failed", (e) => {
      setStatus((s) => (s.kind === "installing" ? s : { kind: "error", message: translate("update.check_failed", { error: e.payload }) }));
    });
    return () => {
      unStatus.then((f) => f());
      unFailed.then((f) => f());
      unProgress.then((f) => f());
    };
  }, []);

  const check = () => {
    setStatus({ kind: "checking" });
    api
      .checkUpdate()
      .then((info) => setStatus(info ? { kind: "available", info } : { kind: "latest" }))
      .catch((e) => setStatus({ kind: "error", message: t("update.check_failed", { error: String(e) }) }));
  };

  const install = (info: UpdateInfo) => {
    setStatus({ kind: "installing", info, done: 0, total: 0 });
    // On success the app restarts — we only get back here on error.
    api.installUpdate().catch((e) => setStatus({ kind: "error", message: String(e), info }));
  };

  const info = status.kind === "available" || status.kind === "installing" || status.kind === "error" ? status.info : undefined;
  const fraction = status.kind === "installing" && status.total > 0 ? status.done / status.total : 0;

  return (
    <div className="card">
      <div className="card-head">
        <div>
          <strong>{t("update.title")}</strong>{" "}
          {status.kind === "latest" && <span className="ok">{t("update.latest")}</span>}
          {info && status.kind !== "error" && <span className="badge">{t("update.available", { version: info.version })}</span>}
          <div className="hint">
            {t("update.installed", { version })}
            {info?.date && ` · ${t("update.release", { version: info.version, date: info.date })}`}
          </div>
        </div>
        <div className="actions">
          {status.kind === "available" && (
            <button className="primary" onClick={() => install(status.info)}>
              {t("update.install")}
            </button>
          )}
          {status.kind === "error" && <button onClick={() => openUrl(RELEASES)}>{t("update.open_release")}</button>}
          {status.kind !== "installing" && status.kind !== "available" && (
            <button disabled={status.kind === "checking"} onClick={check}>
              {status.kind === "checking" ? t("update.checking") : t("update.check")}
            </button>
          )}
        </div>
      </div>
      {status.kind === "installing" && (
        <>
          <div className="progress">
            <span style={{ width: `${Math.round(fraction * 100)}%` }} />
            <em>{status.total > 0 ? `${Math.round(fraction * 100)}%` : formatBytes(status.done)}</em>
          </div>
          <div className="hint">{t("update.installing")}</div>
        </>
      )}
      {status.kind === "error" && <div className="error">{status.message}</div>}
      {info?.notes && status.kind !== "installing" && (
        <div className="markdown hint">
          <Markdown text={info.notes} />
        </div>
      )}
    </div>
  );
}
