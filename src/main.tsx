import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { api } from "./api";
import { browserLocale, I18nProvider, normalizeLocale, setLocale, type Locale } from "./i18n";
// Fonts bundled with the app (no internet needed), with Polish characters.
import "@fontsource-variable/fraunces/opsz.css";
import "@fontsource-variable/fraunces/opsz-italic.css";
import "@fontsource-variable/instrument-sans";
import "@fontsource-variable/jetbrains-mono";
import "./styles.css";

/** Resolved UI locale from the backend before the first render (no flash of the wrong language). */
async function initialLocale(): Promise<Locale> {
  try {
    const timeout = new Promise<never>((_, reject) => setTimeout(() => reject(new Error("timeout")), 1500));
    return normalizeLocale(await Promise.race([api.uiLocale(), timeout]));
  } catch {
    return browserLocale();
  }
}

initialLocale().then((locale) => {
  setLocale(locale);
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <I18nProvider>
        <App />
      </I18nProvider>
    </React.StrictMode>,
  );
});
