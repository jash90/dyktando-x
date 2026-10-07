import Hud from "./Hud";
import SettingsApp from "./settings/SettingsApp";
import MeetingsApp from "./meetings/MeetingsApp";
import LiveWindow from "./meetings/LiveWindow";
import Prompt from "./Prompt";

export default function App() {
  // Jedno wejście dla wszystkich okien; dymek ładuje `index.html#hud`.
  if (window.location.hash === "#hud") {
    document.documentElement.classList.add("hud-root");
    return <Hud />;
  }
  if (window.location.hash === "#meetings") return <MeetingsApp />;
  if (window.location.hash === "#live") {
    document.documentElement.classList.add("hud-root");
    return <LiveWindow />;
  }
  if (window.location.hash.startsWith("#prompt")) {
    document.documentElement.classList.add("hud-root");
    return <Prompt />;
  }
  return <SettingsApp />;
}
