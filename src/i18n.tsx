import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import en from "./locales/en.json";
import pl from "./locales/pl.json";

/** UI language (independent of the dictation / speech-recognition language). */
export type Locale = "en" | "pl";

/** Every translation key — English is the reference set. */
export type Key = keyof typeof en;
/** Base keys of plural entries (`foo_one`, `foo_other`, … → `foo`). */
export type PluralKey = { [K in Key]: K extends `${infer B}_other` ? B : never }[Key];
/** Keys that are not plural forms. */
export type PlainKey = Exclude<Key, `${string}_${"zero" | "one" | "two" | "few" | "many" | "other"}`>;
type Params = Record<string, string | number>;

// Compile-time parity: Polish must have every key English has (plural keys need `_other` in both).
const plChecked: Record<Key, string> = pl;

const DICTS: Record<Locale, Record<string, string>> = { en, pl: plChecked };

let current: Locale = "en";
const plural: Record<Locale, Intl.PluralRules> = { en: new Intl.PluralRules("en"), pl: new Intl.PluralRules("pl") };

/** Active UI locale (also usable outside React, e.g. in formatting helpers). */
export function getLocale(): Locale {
  return current;
}

/** BCP 47 tag for Intl formatters. */
export function intlLocale(): string {
  return current === "pl" ? "pl-PL" : "en-US";
}

function fill(text: string, params?: Params): string {
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (m, name: string) => (name in params ? String(params[name]) : m));
}

/** Translates a key in the active locale; falls back to English, then to the key itself. */
export function t(key: PlainKey, params?: Params): string {
  return fill(DICTS[current][key] ?? DICTS.en[key] ?? key, params);
}

/** Plural-aware translation: picks `<key>_<form>` via `Intl.PluralRules`; `{count}` is filled automatically. */
export function tn(key: PluralKey, count: number, params?: Params): string {
  const form = plural[current].select(count);
  const dict = DICTS[current];
  const text = dict[`${key}_${form}`] ?? dict[`${key}_other`] ?? DICTS.en[`${key}_other`] ?? key;
  return fill(text, { count, ...params });
}

export function normalizeLocale(value: unknown): Locale {
  return typeof value === "string" && value.toLowerCase().startsWith("pl") ? "pl" : "en";
}

/** Fallback when the backend can't be asked (e.g. plain browser during development). */
export function browserLocale(): Locale {
  return normalizeLocale(typeof navigator !== "undefined" ? navigator.language : "en");
}

const listeners = new Set<(l: Locale) => void>();

export function setLocale(locale: Locale) {
  current = locale;
  document.documentElement.lang = locale;
  listeners.forEach((f) => f(locale));
}

interface I18n {
  locale: Locale;
  t: typeof t;
  tn: typeof tn;
}

const Ctx = createContext<I18n>({ locale: current, t, tn });

/** Keeps the tree in sync with the locale: re-renders on `setLocale` and on the backend `ui-locale-changed` event. */
export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setState] = useState<Locale>(current);
  useEffect(() => {
    listeners.add(setState);
    setState(current);
    let un: (() => void) | undefined;
    let dead = false;
    listen<string>("ui-locale-changed", (e) => setLocale(normalizeLocale(e.payload)))
      .then((f) => (dead ? f() : (un = f)))
      .catch(() => {});
    return () => {
      dead = true;
      listeners.delete(setState);
      un?.();
    };
  }, []);
  // A new object per locale so every `useT()` consumer re-renders.
  const value = { locale, t, tn };
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** `const { t, tn, locale } = useT();` — subscribes the component to locale changes. */
export function useT(): I18n {
  return useContext(Ctx);
}
