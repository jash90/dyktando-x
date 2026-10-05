import { invoke } from "@tauri-apps/api/core";
import { Circle } from "lucide-react";

/** Podpowiedź „Wykryto spotkanie w … — nagrać?” (osobne, małe okno bez fokusu). */
export default function Prompt() {
  const app = new URLSearchParams(window.location.hash.split("?")[1] ?? "").get("app") ?? "aplikacji do rozmów";
  const answer = (record: boolean) => invoke("prompt_answer", { record }).catch(() => {});
  return (
    <div className="prompt">
      <Circle className="prompt-icon" size={26} />
      <div className="prompt-body">
        <strong>Wykryto spotkanie w {app}</strong>
        <span>Nagrać je w Dyktando X? Pamiętaj, by poinformować rozmówców.</span>
        <div className="prompt-actions">
          <button className="primary" onClick={() => answer(true)}>
            Nagraj
          </button>
          <button onClick={() => answer(false)}>Nie teraz</button>
        </div>
      </div>
    </div>
  );
}
