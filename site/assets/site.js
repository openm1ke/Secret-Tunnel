(() => {
  const REPO = "openm1ke/Secret-Tunnel";

  // Phone menu.
  const menu = document.querySelector(".menu");
  const nav = document.getElementById("nav");
  if (menu && nav) {
    menu.addEventListener("click", () => {
      const open = nav.classList.toggle("open");
      menu.setAttribute("aria-expanded", String(open));
    });
    nav.addEventListener("click", (event) => {
      if (event.target.closest("a")) {
        nav.classList.remove("open");
        menu.setAttribute("aria-expanded", "false");
      }
    });
  }

  // The route diagram is the page's only motion; it stops for reduced motion.
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    document.querySelectorAll(".route svg").forEach((svg) => svg.pauseAnimations && svg.pauseAnimations());
  }

  // Screenshots.
  const shots = {
    desktop: [
      ["desktop-connect.png", "Подключение", "Маршрут от компьютера до тестового сервера, скорость и журнал соединений."],
      ["desktop-apps.png", "Приложения", "Галочки у нужных программ. Процессы, которые они запускают, подхватываются сами."],
      ["desktop-speed.png", "Скорость", "Тесты через туннель и без него, история замеров с маршрутом и страной выхода."],
      ["desktop-stats.png", "Статистика", "Сеансы и трафик каждой программы за выбранный период."],
      ["desktop-servers.png", "Серверы", "Серверы и цепочки: промежуточное звено выбирается в профиле."],
    ],
    android: [
      ["android-home.png", "Главная", "Профиль подключения, текущая скорость и трафик приложений."],
      ["android-apps.png", "Приложения", "Список программ, для которых используется туннель."],
      ["android-servers.png", "Серверы", "SSH-профиль с адресом, портом и ключом."],
      ["android-settings.png", "Настройки", "Тема, проверка подключения и показ адресов. Настройки общие для всех профилей."],
      ["android-journal.png", "Журнал", "Диагностика транспорта: очереди, UDP, задержки."],
    ],
  };
  const stage = document.querySelector(".stage");
  const thumbs = document.querySelector(".thumbs");
  if (stage && thumbs) {
    const image = stage.querySelector("img");
    const caption = stage.querySelector("figcaption");
    let set = "desktop";
    const show = (index) => {
      const [file, title, text] = shots[set][index];
      image.src = `assets/screens/${file}`;
      image.alt = `Экран «${title}»`;
      caption.innerHTML = `<b>${title}.</b> ${text}`;
      stage.classList.toggle("phone", set === "android");
      thumbs.querySelectorAll("button").forEach((button, i) => button.setAttribute("aria-current", String(i === index)));
    };
    const render = () => {
      thumbs.innerHTML = shots[set].map(([, title, text]) => `<li><button type="button"><b>${title}</b><span>${text}</span></button></li>`).join("");
      thumbs.querySelectorAll("button").forEach((button, index) => button.addEventListener("click", () => show(index)));
      show(0);
    };
    document.querySelectorAll(".screens .tabs button").forEach((tab) => {
      tab.addEventListener("click", () => {
        set = tab.dataset.set;
        document.querySelectorAll(".screens .tabs button").forEach((other) => other.setAttribute("aria-selected", String(other === tab)));
        render();
      });
    });
    render();
  }

  // Knowledge base search: filters articles and the contents, highlights matches.
  const search = document.getElementById("kb-search");
  if (search) {
    const articles = [...document.querySelectorAll("article.doc")];
    const links = [...document.querySelectorAll(".toc a")];
    const parts = [...document.querySelectorAll(".kb .part")];
    const count = document.querySelector(".results");
    const empty = document.querySelector(".empty-search");
    const originals = new Map(articles.map((article) => [article, article.innerHTML]));
    const normalize = (text) => text.toLowerCase().replace(/ё/g, "е");
    const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const highlight = (root, words) => {
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
        acceptNode: (node) => (node.parentElement.closest("pre, code, h2 a") ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT),
      });
      const nodes = [];
      while (walker.nextNode()) nodes.push(walker.currentNode);
      const pattern = new RegExp(`(${words.map((word) => escape(word).replace(/е/g, "[её]")).join("|")})`, "gi");
      nodes.forEach((node) => {
        if (!pattern.test(node.textContent)) return;
        const span = document.createElement("span");
        span.innerHTML = node.textContent.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]).replace(pattern, "<mark>$1</mark>");
        node.replaceWith(...span.childNodes);
      });
    };
    const run = () => {
      const words = normalize(search.value).split(/\s+/).filter((word) => word.length > 1);
      let shown = 0;
      articles.forEach((article) => {
        article.innerHTML = originals.get(article);
        const text = normalize(article.textContent + " " + (article.dataset.keywords || ""));
        const match = words.every((word) => text.includes(word));
        article.hidden = !match;
        if (match) {
          shown += 1;
          if (words.length) highlight(article, words);
        }
      });
      links.forEach((link) => {
        const target = document.getElementById(link.getAttribute("href").slice(1));
        link.classList.toggle("hidden", Boolean(target && target.hidden));
      });
      // A group heading stays only while one of its links does.
      document.querySelectorAll(".toc .group").forEach((group) => {
        let next = group.nextElementSibling;
        let any = false;
        while (next && !next.classList.contains("group")) {
          if (next.querySelector("a:not(.hidden)")) any = true;
          next = next.nextElementSibling;
        }
        group.hidden = !any;
      });
      parts.forEach((part) => {
        let next = part.nextElementSibling;
        let any = false;
        while (next && !next.classList.contains("part")) {
          if (next.matches("article.doc") && !next.hidden) any = true;
          next = next.nextElementSibling;
        }
        part.hidden = !any;
      });
      if (count) count.textContent = words.length ? (shown ? `Найдено статей: ${shown}` : "") : "";
      if (empty) empty.hidden = shown > 0;
      const params = new URLSearchParams(location.search);
      if (search.value) params.set("q", search.value); else params.delete("q");
      history.replaceState(null, "", `${location.pathname}${params.toString() ? `?${params}` : ""}${location.hash}`);
    };
    search.addEventListener("input", run);
    document.addEventListener("keydown", (event) => {
      if (event.key === "/" && document.activeElement !== search && !event.target.closest("input, textarea")) {
        event.preventDefault();
        search.focus();
      }
      if (event.key === "Escape" && document.activeElement === search) {
        search.value = "";
        run();
      }
    });
    const initial = new URLSearchParams(location.search).get("q");
    if (initial) {
      search.value = initial;
      run();
    }
  }

  // Releases from GitHub, newest first.
  const list = document.getElementById("release-list");
  if (list) {
    const size = (bytes) => (bytes > 1048576 ? `${(bytes / 1048576).toFixed(1)} МБ` : `${Math.max(1, Math.round(bytes / 1024))} КБ`);
    const date = (iso) => new Date(iso).toLocaleDateString("ru-RU", { day: "numeric", month: "long", year: "numeric" });
    const esc = (text) => String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
    const platform = (name) => {
      const n = name.toLowerCase();
      if (/\.apk$|android/.test(n)) return "Android";
      if (/\.dmg$|macos|darwin|\.app\.zip$/.test(n)) return "macOS";
      if (/\.exe$|\.msi$|windows/.test(n)) return "Windows";
      if (/server|agent|linux/.test(n)) return "Сервер";
      return "Прочее";
    };
    const order = ["macOS", "Windows", "Android", "Сервер", "Прочее"];
    const label = (name) => {
      const n = name.toLowerCase();
      if (n.endsWith(".dmg")) return "Образ DMG";
      if (/macos.*\.zip$/.test(n)) return "Архив ZIP";
      if (/setup\.exe$/.test(n)) return "Установщик";
      if (/portable.*\.zip$/.test(n)) return "Переносная версия";
      if (n.endsWith(".apk")) return "Приложение APK";
      if (/server.*\.tar\.gz$/.test(n)) return "Агент для Linux x86_64";
      if (/sha256sums/.test(n)) return "Контрольные суммы";
      return name;
    };
    const render = (releases) => {
      const published = releases.filter((release) => !release.draft);
      if (!published.length) {
        list.innerHTML = '<p class="state">Опубликованных релизов пока нет. Они появятся здесь сразу после публикации на GitHub.</p>';
        return;
      }
      list.innerHTML = published.map((release, index) => {
        const groups = {};
        // Installers first: setup.exe and the DMG are what most people want.
        const rank = (name) => (/setup\.exe$|\.dmg$/i.test(name) ? 0 : 1);
        [...release.assets].sort((a, b) => rank(a.name) - rank(b.name)).forEach((asset) => (groups[platform(asset.name)] ||= []).push(asset));
        const downloads = order.filter((key) => groups[key]).map((key) => `
          <section><h3>${key}</h3>${groups[key].map((asset) => `<a href="${esc(asset.browser_download_url)}" title="${esc(asset.name)}">${esc(label(asset.name))}<small>${size(asset.size)}</small></a>`).join("")}</section>`).join("");
        const latest = index === 0 && !release.prerelease;
        return `<article class="release${latest ? " latest" : ""}" id="${esc(release.tag_name)}">
          <header>
            <h2>${esc(release.tag_name)}</h2>
            ${latest ? '<span class="badge">Последний</span>' : ""}${release.prerelease ? '<span class="badge pre">Пробный</span>' : ""}
            <span class="when">${date(release.published_at || release.created_at)}</span>
          </header>
          ${downloads ? `<div class="downloads">${downloads}</div>` : '<p class="state">Файлы ещё загружаются.</p>'}
          <a href="${esc(release.html_url)}">Релиз на GitHub</a>
        </article>`;
      }).join("");
    };
    fetch(`https://api.github.com/repos/${REPO}/releases?per_page=30`, { headers: { Accept: "application/vnd.github+json" } })
      .then((response) => {
        if (!response.ok) throw new Error(`GitHub ответил ${response.status}`);
        return response.json();
      })
      .then(render)
      .catch((error) => {
        list.innerHTML = `<p class="state">Не удалось загрузить список релизов (${esc(error.message)}). Откройте <a href="https://github.com/${REPO}/releases">релизы на GitHub</a>.</p>`;
      });
  }
})();
