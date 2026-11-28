import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import ts from "typescript";

const source = new URL("../src/i18n.ts", import.meta.url);
const code = ts.transpileModule(readFileSync(source, "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, esModuleInterop: true },
}).outputText;
const storage = new Map();
const localStorage = { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value) };
const document = { documentElement: { lang: "" } };
function load(preference = "ru", store = localStorage) {
  storage.set("secret-tunnel.language", preference);
  const module = { exports: {} };
  new Function("require", "module", "exports", "localStorage", "document", code)(createRequire(source), module, module.exports, store, document);
  return module.exports;
}

const locale = load();
assert.equal(locale.t("Подключить"), "Подключить");
locale.setLanguage("en");
assert.equal(locale.t("Подключить"), "Connect");
assert.equal(locale.formatLocale(), "en-US");
assert.equal(document.documentElement.lang, "en");
assert.equal(storage.get("secret-tunnel.language"), "en");
assert.equal(locale.t("Сервер «{0}» сохранён", "Мой сервер {1} 🔒"), "Server “Мой сервер {1} 🔒” saved");
assert.equal(locale.t("{0} ч {1} мин", 21, 8), "21 h 8 min");
assert.equal(locale.translateMessage("Не удалось проверить сервер «Рабочий». "), "Could not verify server “Рабочий”. ");
assert.equal(locale.translateMessage("Сервер теста скорости вернул HTTP 429."), "The speed test server returned HTTP 429.");
assert.equal(locale.translateMessage("UDP: ошибки DNS 2 · отправка 3 · приём 4\n"), "UDP: DNS errors 2 · send 3 · receive 4\n");
assert.equal(locale.translateMessage("Неизвестная ошибка: E_CUSTOM 203.0.113.1"), "Неизвестная ошибка: E_CUSTOM 203.0.113.1");
assert.equal(load("en").t("Подключить"), "Connect");
assert.equal(load("invalid").getLanguage(), "ru");
locale.setLanguage("ru");
assert.equal(locale.t("{0} ч {1} мин", 21, 8), "21 ч 8 мин");
assert.equal(locale.translateMessage("Сервер теста скорости вернул HTTP 429."), "Сервер теста скорости вернул HTTP 429.");
const noStorage = load("ru", { getItem() { throw Error("disabled"); }, setItem() { throw Error("disabled"); } });
noStorage.setLanguage("en");
assert.equal(noStorage.t("Подключить"), "Connect");
console.log("Localization runtime: language persistence, fallback, dynamic errors and opaque arguments passed");
