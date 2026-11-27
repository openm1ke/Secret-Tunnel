# Ежедневное обновление каталога

Данные публикуются отдельно от приложения:
https://openm1ke.github.io/Secret-Tunnel/catalog/hosting-v1.json.
Workflow: `.github/workflows/hosting-catalog.yml` — **Hosting catalog**.

## Запуск

- Ежедневно в 05:25 UTC (08:25 по Москве); GitHub может задерживать запуск.
- Изменения адаптеров/данных в основной ветке запускают обновление.
- Push другой ветки и pull request проверяют данные без публикации.
- Ручной запуск: Actions → Hosting catalog → Run workflow. Без `publish`
  создаётся кандидат; с `publish` публикация разрешена только с основной ветки.
- Приложение/агент не собираются, версия не меняется, тег/релиз не создаются.

Порядок: тесты Python/Rust → последнее состояние `gh-pages` → источники →
валидация клиентским парсером → артефакт → публикация одного JSON →
явная сборка Pages → проверка SHA-256 по публичному HTTPS-адресу.

Стандартный `GITHUB_TOKEN`: чтение в задаче обновления; `contents: write`
и `pages: write` только в задаче публикации. PAT и новые секреты не нужны.
Pages остаётся в режиме ветки `gh-pages`. Коммит Actions сам не запускает
такую сборку ([документация GitHub](https://docs.github.com/en/pages/getting-started-with-github-pages/configuring-a-publishing-source-for-your-github-pages-site));
workflow явно вызывает `POST /repos/{owner}/{repo}/pages/builds`.

Запуски сериализованы. Если сайт изменился во время сбора, публикация
отказывается от перезаписи: нужен повторный запуск с новым состоянием.
Push обычный, без force. `scripts/publish-site.sh` сохраняет историю
и более свежий опубликованный каталог при обновлении остальных страниц.

## Ошибки источников

Исходный JSON задаёт состав карточек, ссылки, локации и тестовые узлы.
По ID подмешиваются более свежие тарифные факты из опубликованного JSON.
Ошибка источника не откатывает цену к старой копии из приложения.
Повреждённый опубликованный JSON останавливает обновление.

При отказе сохраняются прежние характеристики, цена и `verified_at`,
устанавливается `update_error`. Успешные источники публикуются. Если не
обновился ни один, публикации нет. Задача `source-health` в обоих случаях
завершается ошибкой, видимой в Actions и обычных уведомлениях GitHub согласно
настройкам аккаунта. Отдельных писем, Slack и автоматических issues нет.

Артефакт `hosting-catalog-candidate` хранится 30 дней: `hosting-v1.json`
и `hosting-report.json`. Отчёт содержит статус `ok` / `partial` / `failed`
и ошибки источников; Job Summary — результаты и даты. Дата генерации
не заменяет дату проверки отдельного тарифа.

## Источники

Это отобранные предложения, а не поиск минимальной цены во всём интернете.
Неоднозначность тарифа, смена валюты, неполный API-ответ или характеристики
считаются ошибкой.

| Провайдер / локация | Предложение и источник |
| --- | --- |
| OVHcloud / Страсбург | `vps-2025-model1`, [API IE](https://eu.api.ovh.com/1.0/order/catalog/public/vps?ovhSubsidiary=IE), обычное месячное продление, без НДС. |
| ITLDC / Амстердам | AMS NVMe VDS 1G, [Astro props](https://itldc.com/en/vds/), EUR. Скорость порта не указана. |
| Timeweb Cloud / Москва | Cloud MSK 15, [Nuxt-данные](https://timeweb.cloud/services/vds-vps), базовая цена, IPv4 уточняется при заказе. |
| FirstVDS / Москва | VDS Прогрев KVM/SSD, [структурированные данные](https://firstvds.ru/products/vds_vps_hosting), месячная цена с IPv4. |
| IPHoster / Лимбург | VPS-NVE-S, [HTML-карточка](https://iphoster.net/vds), USD из `data-cur`, месяц из `data-m`, не годовая и не зачёркнутая цена. IPv4 включён, общий порт/fair use. |
| Vultr / Амстердам | `vc2-1c-1gb`, [API v2 plans](https://api.vultr.com/v2/plans?per_page=500), on-demand, выбранная локация и её переопределение цены. Квота GB не выдаётся за скорость порта. |
| Hetzner / NBG1 | CPX02, [публичный Price API](https://website-price-api.hetzner.com/), месяц `CLOUD_140` + Primary IPv4 `CLOUD_21`, без НДС. Характеристики из [карточки](https://www.hetzner.com/cloud/regular-performance/). |

У Hetzner резервная цена HTML при исследовании отличалась от API.
Она не участвует в расчёте. Закрытый Cloud API с токеном не используется.
Vultr проверяет локацию: тариф в другой стране не подставляется автоматически.

Скорость порта — заявление провайдера, не гарантия скорости SSH. Нет
пересчёта валют и партнёрских ссылок. Налоги, окончательная цена, наличие
ресурсов, регистрационные требования и оплата проверяются у хостера.

## Замер с устройства

Новые подтверждённые узлы: [Hetzner NBG1](https://nbg1-speed.hetzner.com/)
и [Vultr Amsterdam](https://ams-nl-ping.vultr.com/), TCP 443.
Прежние узлы OVHcloud, ITLDC, Timeweb и FirstVDS сохранены.
CI не измеряет пользовательскую задержку: тест запускается в приложении.

IPHoster публикует [ICMP-адреса](https://wiki.iphoster.net/wiki/KVM_VPS_-_Locations_of_our_shared_hosting_servers,_VPS_KVM_and_dedicated_servers).
Выбранные адреса не ответили на TCP 80/443. Не подменяем их сайтом компании,
который может быть в другой сети. Карточка остаётся с ценой/характеристиками,
но показывает «Нет узла / замер недоступен». В JSON `test_host` и `test_port`
равны `null` или отсутствуют вместе. Rust возвращает ноль попыток без DNS
и подключения. TCP не является ICMP, таймаут не доказывает недоступность VPS.

## Проверки

```sh
python3 -m unittest discover -s scripts/tests -p 'test_hosting*.py' -v
cargo test --manifest-path hosting-catalog/Cargo.toml --locked
actionlint .github/workflows/hosting-catalog.yml
python3 scripts/update-hosting-catalog.py --published /path/to/last-published.json \
  --output /tmp/hosting-v1.json --report /tmp/hosting-report.json --allow-partial
cargo run --manifest-path hosting-catalog/Cargo.toml --locked --example catalog -- validate /tmp/hosting-v1.json
```

Отсутствие `--published`-файла допускается только для первой публикации.
Обычно он извлекается из текущего `gh-pages`. Фикстуры минимальные,
без сессий и токенов сайтов. Publisher проверяется на локальном bare origin;
боевые SSH-серверы и пользовательские ключи не используются.
