import { invoke } from "@tauri-apps/api/core";
import { Circle } from "lucide-react";
import { useT } from "./i18n";

/** The „Wykryto spotkanie w … — nagrać?” ("Meeting detected in … — record?") prompt (a separate small window without focus). */
export default function Prompt() {
  const { t } = useT();
  const app = new URLSearchParams(window.location.hash.split("?")[1] ?? "").get("app") ?? t("prompt.default_app");
  const answer = (record: boolean) => invoke("prompt_answer", { record }).catch(() => {});
  return (
    <div className="prompt">
      <Circle className="prompt-icon" size={26} />
      <div className="prompt-body">
        <strong>{t("prompt.title", { app })}</strong>
        <span>{t("prompt.body")}</span>
        <div className="prompt-actions">
          <button className="primary" onClick={() => answer(true)}>
            {t("prompt.record")}
          </button>
          <button onClick={() => answer(false)}>{t("prompt.not_now")}</button>
        </div>
      </div>
    </div>
  );
}
