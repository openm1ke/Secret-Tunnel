(() => {
  const body = document.body;
  const store = {
    get(key) { try { return localStorage.getItem(key); } catch { return null; } },
    set(key, value) { try { localStorage.setItem(key, value); } catch {} },
  };

  // Which computer: remembered, otherwise guessed from the browser.
  const guess = () => {
    const ua = navigator.userAgent;
    if (/Windows/i.test(ua)) return "win";
    if (/Linux|X11/i.test(ua) && !/Android/i.test(ua)) return "linux";
    return "mac";
  };
  const keyPath = { mac: "/Users/user/.ssh/secret-tunnel", linux: "/home/user/.ssh/secret-tunnel", win: "C:\\Users\\user\\.ssh\\secret-tunnel" };
  const setOs = (os) => {
    body.dataset.os = os;
    document.querySelectorAll("[data-set-os]").forEach((button) => button.setAttribute("aria-pressed", String(button.dataset.setOs === os)));
    document.querySelectorAll(".keypath").forEach((el) => { el.textContent = keyPath[os]; });
    store.set("guide.os", os);
  };
  document.querySelectorAll("[data-set-os]").forEach((button) => button.addEventListener("click", () => setOs(button.dataset.setOs)));
  setOs(["mac", "win", "linux"].includes(store.get("guide.os")) ? store.get("guide.os") : guess());

  // The server address and login go into every command. They stay in this tab.
  const ipInput = document.getElementById("v-ip");
  const loginInput = document.getElementById("v-login");
  const clean = (value, fallback, pattern) => {
    const text = value.trim();
    return text && pattern.test(text) ? text : fallback;
  };
  const apply = () => {
    const ip = clean(ipInput.value, "203.0.113.10", /^[A-Za-z0-9.:\-\[\]]+$/);
    const login = clean(loginInput.value, "root", /^[a-z_][a-z0-9_.-]*$/i);
    document.querySelectorAll(".v-ip").forEach((el) => { el.textContent = ip; });
    document.querySelectorAll(".v-login").forEach((el) => { el.textContent = login; });
    document.querySelectorAll(".v-akeys").forEach((el) => { el.textContent = login === "root" ? "/root/.ssh/authorized_keys" : `/home/${login}/.ssh/authorized_keys`; });
    body.dataset.login = login === "root" ? "root" : "other";
    ipInput.toggleAttribute("aria-invalid", Boolean(ipInput.value.trim()) && ip !== ipInput.value.trim());
  };
  ipInput.addEventListener("input", apply);
  loginInput.addEventListener("input", apply);
  apply();

  // Copy buttons. innerText skips hidden parts such as `sudo -i` for root.
  document.querySelectorAll("pre.cmd").forEach((pre) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "copy";
    button.textContent = "Копировать";
    button.addEventListener("click", async () => {
      const text = pre.querySelector("code").innerText.replace(/\n+$/, "");
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        const area = document.createElement("textarea");
        area.value = text;
        document.body.append(area);
        area.select();
        document.execCommand("copy");
        area.remove();
      }
      button.textContent = "Скопировано";
      button.classList.add("done");
      setTimeout(() => { button.textContent = "Копировать"; button.classList.remove("done"); }, 1600);
    });
    const box = document.createElement("div");
    box.className = "cmdbox";
    if (pre.dataset.os) { box.dataset.os = pre.dataset.os; pre.removeAttribute("data-os"); }
    pre.replaceWith(box);
    box.append(pre, button);
  });

  // Screenshots open full size, which matters on a phone.
  document.querySelectorAll("figure.shot img").forEach((img) => {
    const link = document.createElement("a");
    link.href = img.getAttribute("src");
    link.target = "_blank";
    link.rel = "noopener";
    link.title = "Открыть в полном размере";
    img.replaceWith(link);
    link.append(img);
  });

  // Numbered marks on screenshots, matched to the list under each one.
  document.querySelectorAll("figure.shot[data-pins]").forEach((figure) => {
    const legend = figure.nextElementSibling?.matches("ol.legend") ? figure.nextElementSibling : null;
    const items = legend ? [...legend.children] : [];
    figure.dataset.pins.split(";").forEach((pair, index) => {
      const [x, y] = pair.split(",").map(Number);
      const pin = document.createElement("span");
      pin.className = "pin";
      pin.textContent = String(index + 1);
      pin.style.left = `${x}%`;
      pin.style.top = `${y}%`;
      pin.setAttribute("aria-hidden", "true");
      figure.append(pin);
      const item = items[index];
      if (item) {
        const on = () => { pin.classList.add("hot"); item.classList.add("hot"); };
        const off = () => { pin.classList.remove("hot"); item.classList.remove("hot"); };
        item.addEventListener("mouseenter", on);
        item.addEventListener("mouseleave", off);
      }
    });
  });

  // Table of contents follows the reader.
  const links = new Map([...document.querySelectorAll(".toc a")].map((a) => [a.getAttribute("href").slice(1), a]));
  if ("IntersectionObserver" in window) {
    const seen = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        links.forEach((a) => a.removeAttribute("aria-current"));
        links.get(entry.target.id)?.setAttribute("aria-current", "true");
      });
    }, { rootMargin: "-90px 0px -70% 0px" });
    document.querySelectorAll("article.doc[id]").forEach((article) => seen.observe(article));
  }
})();
