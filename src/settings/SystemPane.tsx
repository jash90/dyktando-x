import type { PaneProps } from "./SettingsApp";
import { api, type Settings } from "../api";
import { useT } from "../i18n";
import UpdateCard from "./UpdateCard";

const UDEV = `sudo tee /etc/udev/rules.d/70-dyktando-x.rules <<'RULES'
KERNEL=="uinput", TAG+="uaccess"
SUBSYSTEM=="input", KERNEL=="event*", TAG+="uaccess"
RULES
sudo udevadm control --reload && sudo udevadm trigger`;

export default function SystemPane({ settings, update, env, onRefresh }: PaneProps & { onRefresh: () => void }) {
  const { t } = useT();
  return (
    <section>
      <h1>{t("system.title")}</h1>
      <div className="row">
        <label>{t("system.ui_language")}</label>
        <select value={settings.ui_language ?? "system"} onChange={(e) => update({ ui_language: e.target.value as Settings["ui_language"] })}>
          <option value="system">{t("system.ui_language.system")}</option>
          {/* Each language is named in itself. */}
          <option value="en" lang="en">
            English
          </option>
          <option value="pl" lang="pl">
            Polski
          </option>
        </select>
        <p className="hint">{t("system.ui_language_hint")}</p>
      </div>
      <UpdateCard />
      {env.os === "macos" && (
        <div className="card">
          <div className="card-head">
            <div>
              <strong>{t("system.accessibility")}</strong> {env.can_send_keys ? <span className="ok">{t("system.granted")}</span> : <span className="bad">{t("system.missing")}</span>}
              <div className="hint">
                {t("system.accessibility_hint")}
              </div>
            </div>
            <div className="actions">
              <button onClick={() => api.openAccessibilitySettings().then(onRefresh)}>{t("system.open_settings")}</button>
              <button onClick={() => api.reloadHotkeys().then(onRefresh)}>{t("system.check_again")}</button>
            </div>
          </div>
        </div>
      )}
      {env.os === "macos" && (
        <div className="card">
          <strong>{t("system.microphone")}</strong>
          <div className="hint">{t("system.microphone_macos")}</div>
        </div>
      )}
      {env.os === "windows" && (
        <div className="card">
          <strong>{t("system.microphone")}</strong>
          <div className="hint">{t("system.microphone_windows")}</div>
        </div>
      )}
      {env.os === "linux" && (
        <>
          <div className="card">
            <strong>{t("system.session", { session: env.wayland ? "Wayland" : "X11" })}</strong>
            <div className="hint">
              {env.wayland
                ? t("system.paste_wayland")
                : t("system.paste_x11")}
            </div>
          </div>
          <div className="card">
            <strong>{t("system.global_shortcuts")}</strong>
            <div className="hint">{t("system.udev_hint")}</div>
            <pre>{UDEV}</pre>
            <button onClick={() => api.reloadHotkeys().then(onRefresh)}>{t("system.check_again")}</button>
          </div>
        </>
      )}
      <div className="card">
        <strong>{t("system.app_data")}</strong>
        <div className="hint">
          <code>{env.data_dir}</code>
        </div>
      </div>
    </section>
  );
}
