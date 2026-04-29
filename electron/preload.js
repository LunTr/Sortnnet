const { contextBridge, ipcRenderer } = require('electron');

const petApi = Object.freeze({
  sendMessage: (payload) => ipcRenderer.invoke('chat:send', payload),
  searchEverything: (payload) => ipcRenderer.invoke('everything:search', payload),
  getSettings: () => ipcRenderer.invoke('settings:get'),
  nextProgressWord: () => ipcRenderer.invoke('progress:next'),
  getProgressInterval: () => ipcRenderer.invoke('progress:interval'),
  pickFiles: () => ipcRenderer.invoke('files:pick'),
  savePastedDataUrl: (payload) => ipcRenderer.invoke('files:savePastedDataUrl', payload),
  savePastedBlob: (payload) => ipcRenderer.invoke('files:savePastedBlob', payload),
  toggleChatWindow: () => ipcRenderer.invoke('chat:toggle'),
  closeWindow: () => ipcRenderer.invoke('window:close'),
  dragWindow: (payload) => ipcRenderer.send('window:drag', payload),
  resizeWindow: (payload) => ipcRenderer.send('window:resize', payload),
});

contextBridge.exposeInMainWorld('petApi', petApi);
