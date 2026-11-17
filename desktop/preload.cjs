const { contextBridge, ipcRenderer } = require('electron');
contextBridge.exposeInMainWorld('router', Object.freeze({
  status: () => ipcRenderer.invoke('router:status'),
  refresh: () => ipcRenderer.invoke('router:refresh'),
  start: () => ipcRenderer.invoke('router:start'),
  stop: () => ipcRenderer.invoke('router:stop'),
  copy: (field) => ipcRenderer.invoke('router:copy', field),
  test: () => ipcRenderer.invoke('router:test'),
  chooseBinary: () => ipcRenderer.invoke('router:choose-binary'),
  save: (settings) => ipcRenderer.invoke('router:save', settings),
}));
