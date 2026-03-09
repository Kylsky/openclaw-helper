/* global document, window */

(() => {
  const subtitleEl = document.getElementById("subtitle");
  const bootLogEl = document.getElementById("log");
  const bootLogCard = document.getElementById("logCard");
  const bootShowLogsCheckbox = document.getElementById("showLogs");

  function append(line) {
    if (!bootLogEl) return;
    bootLogEl.textContent += `${line}\n`;
    bootLogEl.scrollTop = bootLogEl.scrollHeight;
  }

  function revealLogs() {
    if (!bootLogCard) return;
    bootLogCard.classList.remove("hidden");
    if (bootShowLogsCheckbox) bootShowLogsCheckbox.checked = true;
  }

  function setSubtitle(text) {
    if (!subtitleEl) return;
    subtitleEl.textContent = text;
  }

  setSubtitle("加载中…");

  if (bootShowLogsCheckbox?.checked && bootLogCard) {
    bootLogCard.classList.remove("hidden");
  }

  window.addEventListener("error", (event) => {
    const message = event?.message || String(event?.error || "unknown error");
    setSubtitle("前端错误");
    revealLogs();
    append(`[前端错误] ${message}`);
  });

  window.addEventListener("unhandledrejection", (event) => {
    const reason = event?.reason?.message || String(event?.reason || "unknown rejection");
    setSubtitle("前端错误");
    revealLogs();
    append(`[Promise 错误] ${reason}`);
  });

  try {
    const hasTauri = Boolean(window.__TAURI__?.core?.invoke);
    append(`[boot] tauri: ${hasTauri ? "yes" : "no"}`);
  } catch {
    // ignore
  }
})();
