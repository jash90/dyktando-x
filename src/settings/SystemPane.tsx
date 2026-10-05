import type { PaneProps } from "./SettingsApp";
import { api } from "../api";

const UDEV = `sudo tee /etc/udev/rules.d/70-dyktando-x.rules <<'RULES'
KERNEL=="uinput", TAG+="uaccess"
SUBSYSTEM=="input", KERNEL=="event*", TAG+="uaccess"
RULES
sudo udevadm control --reload && sudo udevadm trigger`;

export default function SystemPane({ env, onRefresh }: PaneProps & { onRefresh: () => void }) {
  return (
    <section>
      <h1>Uprawnienia i system</h1>
      {env.os === "macos" && (
        <div className="card">
          <div className="card-head">
            <div>
              <strong>Dostępność</strong> {env.can_send_keys ? <span className="ok">nadana</span> : <span className="bad">brak</span>}
              <div className="hint">
                Potrzebna do skrótów globalnych i wklejania (⌘V). Bez niej tekst trafia tylko do schowka. Po nadaniu kliknij „Sprawdź ponownie”.
              </div>
            </div>
            <div className="actions">
              <button onClick={() => api.openAccessibilitySettings().then(onRefresh)}>Otwórz ustawienia</button>
              <button onClick={() => api.reloadHotkeys().then(onRefresh)}>Sprawdź ponownie</button>
            </div>
          </div>
        </div>
      )}
      {env.os === "macos" && (
        <div className="card">
          <strong>Mikrofon</strong>
          <div className="hint">
            macOS zapyta o zgodę przy pierwszym nagraniu. Gdy dyktowanie zgłasza „cyfrową ciszę”, włącz Dyktando X w Ustawieniach systemowych → Prywatność → Mikrofon.
          </div>
        </div>
      )}
      {env.os === "windows" && (
        <div className="card">
          <strong>Mikrofon</strong>
          <div className="hint">Ustawienia → Prywatność → Mikrofon → „Zezwalaj aplikacjom klasycznym na dostęp do mikrofonu” musi być włączone.</div>
        </div>
      )}
      {env.os === "linux" && (
        <>
          <div className="card">
            <strong>Sesja: {env.wayland ? "Wayland" : "X11"}</strong>
            <div className="hint">
              {env.wayland
                ? "Wklejanie używa po kolei: wtype (Sway, Hyprland), dotool, ydotool. Na GNOME i KDE zainstaluj dotool albo ydotool (wymagają dostępu do /dev/uinput); bez nich tekst zostaje w schowku."
                : "Wklejanie działa przez XTest (Ctrl+V)."}
            </div>
          </div>
          <div className="card">
            <strong>Skróty globalne</strong>
            <div className="hint">Skróty czytają klawiaturę z /dev/input. Jednorazowo dodaj regułę udev (działa od razu, bez wylogowania):</div>
            <pre>{UDEV}</pre>
            <button onClick={() => api.reloadHotkeys().then(onRefresh)}>Sprawdź ponownie</button>
          </div>
        </>
      )}
      <div className="card">
        <strong>Dane aplikacji</strong>
        <div className="hint">
          <code>{env.data_dir}</code>
        </div>
      </div>
    </section>
  );
}
