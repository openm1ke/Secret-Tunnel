import { useSyncExternalStore } from "react";
import english from "../locales/en.json";

export type Language = "ru" | "en";
const storageKey = "secret-tunnel.language";
const messages: Record<string, string> = english;
let language: Language = "ru";
try { if (localStorage.getItem(storageKey) === "en") language = "en"; } catch { /* session-only preference */ }
const listeners = new Set<() => void>();

export const getLanguage = () => language;
export const formatLocale = () => language === "en" ? "en-US" : "ru-RU";
export function setLanguage(value: Language) {
  if (value !== "ru" && value !== "en") return;
  language = value;
  try { localStorage.setItem(storageKey, value); } catch { /* keep the current session usable */ }
  document.documentElement.lang = value;
  listeners.forEach(listener => listener());
}
export function useLanguage(): [Language, typeof setLanguage] {
  return [useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, getLanguage), setLanguage];
}

/** Source messages double as the Russian fallback; arguments are never translated. */
export function t(source: string, ...args: unknown[]): string {
  const message = language === "en" ? messages[source] ?? source : source;
  return message.replace(/\{(\d+)\}/g, (placeholder, index: string) => Number(index) < args.length ? String(args[Number(index)]) : placeholder);
}

// Native errors may already contain their arguments. Keep unknown diagnostics intact.
const patterns = Object.entries(messages).filter(([key]) => /\{\d+\}/.test(key)).sort(([a], [b]) => b.replace(/\{\d+\}/g, "").length - a.replace(/\{\d+\}/g, "").length).map(([key, value]) => {
  const indices: number[] = [];
  const expression = key.split(/(\{\d+\})/).map(part => {
    const match = /^\{(\d+)\}$/.exec(part);
    if (match) { indices.push(Number(match[1])); return "([\\s\\S]*?)"; }
    return part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }).join("");
  return { pattern: new RegExp(`^${expression}$`), indices, value };
});
export function translateMessage(source: string): string {
  if (language !== "en" || source.length > 4096 || !/[А-Яа-яЁё]/.test(source)) return source;
  if (messages[source]) return messages[source];
  for (const { pattern, indices, value } of patterns) {
    const match = pattern.exec(source);
    if (!match) continue;
    const args: string[] = [];
    indices.forEach((index, position) => { args[index] = match[position + 1]; });
    return value.replace(/\{(\d+)\}/g, (placeholder, index: string) => args[Number(index)] ?? placeholder);
  }
  return source;
}
