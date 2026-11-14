(() => {
  const key = "secret-tunnel.appearance";
  const system = window.matchMedia("(prefers-color-scheme: dark)");
  const root = document.documentElement;

  function read() {
    try {
      const value = localStorage.getItem(key);
      if (value === "light" || value === "dark") return value;
    } catch { /* use the system preference if storage is unavailable */ }
    return "system";
  }

  let appearance = read();
  function apply() {
    root.dataset.theme = appearance === "system" ? (system.matches ? "dark" : "light") : appearance;
  }

  // Apply saved colours before the stylesheet is loaded.
  apply();
  system.addEventListener("change", apply);

  document.addEventListener("DOMContentLoaded", () => {
    const select = document.getElementById("appearance");
    if (!select) return;
    select.value = appearance;
    select.addEventListener("change", () => {
      appearance = select.value;
      apply();
      try { localStorage.setItem(key, appearance); } catch { /* retain the choice for this page */ }
    });
    window.addEventListener("storage", (event) => {
      if (event.key === key || event.key === null) {
        appearance = read();
        select.value = appearance;
        apply();
      }
    });
  });
})();
