const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("installer", {
  checkOpenclaw: () => ipcRenderer.invoke("check-openclaw"),
  getGatewayStatus: () => ipcRenderer.invoke("get-gateway-status"),
  startInstall: (options) => ipcRenderer.invoke("start-install", options),
  cancelTask: () => ipcRenderer.invoke("cancel-task"),
  runOpenclaw: (args) => ipcRenderer.invoke("run-openclaw", { args }),
  openDashboard: () => ipcRenderer.invoke("open-dashboard"),
  openWizard: () => ipcRenderer.invoke("open-wizard"),
  uninstallOpenclaw: () => ipcRenderer.invoke("uninstall-openclaw"),
  openExternal: (url) => ipcRenderer.invoke("open-external", { url }),
  onProgress: (callback) => {
    const listener = (_event, payload) => callback(payload);
    ipcRenderer.on("install-progress", listener);
    return () => ipcRenderer.removeListener("install-progress", listener);
  },
  onLog: (callback) => {
    const listener = (_event, payload) => callback(payload);
    ipcRenderer.on("install-log", listener);
    return () => ipcRenderer.removeListener("install-log", listener);
  },
  onOpenclawLog: (callback) => {
    const listener = (_event, payload) => callback(payload);
    ipcRenderer.on("openclaw-log", listener);
    return () => ipcRenderer.removeListener("openclaw-log", listener);
  }
});
