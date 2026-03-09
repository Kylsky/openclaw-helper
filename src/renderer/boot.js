/* global document, window */

const subtitle = document.getElementById("subtitle");
const logEl = document.getElementById("log");
const logCard = document.getElementById("logCard");
const showLogs = document.getElementById("showLogs");

function append(line) {
  if (!logEl) return;
  logEl.textContent += `${line}\n`;
  logEl.scrollTop = logEl.scrollHeight;
}

function revealLogs() {
  if (!logCard) return;
  logCard.classList.remove("hidden");
  if (showLogs) showLogs.checked = true;
}

function setSubtitle(text) {
  if (!subtitle) return;
  subtitle.textContent = text;
}

setSubtitle("加载中…");

if (showLogs?.checked && logCard) {
  logCard.classList.remove("hidden");
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

// Quick environment hint
try {
  const hasTauri = Boolean(window.__TAURI__?.core?.invoke);
  append(`[boot] tauri: ${hasTauri ? "yes" : "no"}`);
} catch {
  // ignore
}
