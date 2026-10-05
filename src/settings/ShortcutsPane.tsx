import type { PaneProps } from "./SettingsApp";
import ShortcutRecorder from "../components/ShortcutRecorder";

export default function ShortcutsPane({ settings, update, env }: PaneProps) {
  const mac = env.os === "macos";
  const modifiers = mac
    ? [
        ["", "Wyłączone"],
        ["CmdRight", "Prawy ⌘"],
        ["OptRight", "Prawy ⌥"],
        ["CtrlRight", "Prawy ⌃"],
        ["ShiftRight", "Prawy ⇧"],
        ["Fn", "fn"],
      ]
    : [
        ["", "Wyłączone"],
        ["CtrlRight", "Prawy Ctrl"],
        ["ShiftRight", "Prawy Shift"],
        ["SuperRight", env.os === "windows" ? "Prawy Win" : "Prawy Super"],
      ];
  return (
    <section>
      <h1>Skróty</h1>
      <div className="row">
        <label>Przytrzymaj, aby mówić</label>
        <ShortcutRecorder
          value={settings.shortcut_push_to_talk}
          onChange={(v) => update({ shortcut_push_to_talk: v })}
          mac={mac}
        />
        <p className="hint">Nagrywa, dopóki trzymasz klawisz; po puszczeniu tekst trafia do aktywnego okna.</p>
      </div>
      <div className="row">
        <label>Naciśnij, aby zacząć / zakończyć</label>
        <ShortcutRecorder value={settings.shortcut_toggle} onChange={(v) => update({ shortcut_toggle: v })} mac={mac} />
      </div>
      <div className="row">
        <label>Sam modyfikator jako „przytrzymaj, aby mówić”</label>
        <select value={settings.modifier_push_to_talk} onChange={(e) => update({ modifier_push_to_talk: e.target.value })}>
          {modifiers.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
        <p className="hint">
          Wciśnięcie innego klawisza w trakcie (np. {mac ? "⌘C" : "Ctrl+C"}) anuluje nagranie — skróty z tym klawiszem działają dalej.
          {!mac && " Prawy Alt to AltGr (polskie znaki), dlatego nie jest dostępny."}
        </p>
      </div>
      <div className="row">
        <label>Nagrywanie spotkania</label>
        <ShortcutRecorder value={settings.shortcut_meeting} onChange={(v) => update({ shortcut_meeting: v })} mac={mac} />
      </div>
      {env.os === "linux" && (
        <p className="hint">
          Na Linuksie skróty czytają klawiaturę z /dev/input (działa też na Waylandzie). Jeśli nie reagują, zobacz zakładkę Uprawnienia.
        </p>
      )}
    </section>
  );
}
