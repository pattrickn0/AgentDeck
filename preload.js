const { contextBridge, ipcRenderer, webUtils } = require('electron');

const on = (channel) => (cb) => ipcRenderer.on(channel, (_e, payload) => cb(payload));

contextBridge.exposeInMainWorld('jwic', {
  spawn: (kind, cols, rows) => ipcRenderer.invoke('pty:spawn', { kind, cols, rows }),
  write: (id, data) => ipcRenderer.send('pty:write', { id, data }),
  submit: (id, text) => ipcRenderer.send('pty:submit', { id, text }),
  resize: (id, cols, rows) => ipcRenderer.send('pty:resize', { id, cols, rows }),
  kill: (id) => ipcRenderer.send('pty:kill', { id }),
  onData: on('pty:data'),
  onExit: on('pty:exit'),
  onFeed: on('feed'),
  onWorkspaceChanged: on('workspace:changed'),
  runTable: (opts) => ipcRenderer.invoke('table:run', opts),
  stopTable: () => ipcRenderer.send('table:stop'),
  pickAttachments: () => ipcRenderer.invoke('attach:pick'),
  addDroppedFiles: (paths) => ipcRenderer.invoke('attach:addFiles', paths),
  saveAttachmentBuffer: (data) => ipcRenderer.invoke('attach:saveBuffer', data),
  pasteClipboardImage: () => ipcRenderer.invoke('attach:pasteClipboardImage'),
  getFilePath: (file) => (webUtils && webUtils.getPathForFile ? webUtils.getPathForFile(file) : file.path),
  saveAudio: (bytes) => ipcRenderer.invoke('audio:save', bytes),
  transcribe: (relPath) => ipcRenderer.invoke('audio:transcribe', relPath),
  transcribeEngine: () => ipcRenderer.invoke('audio:engine'),
  listAgents: () => ipcRenderer.invoke('agents:list'),
  getWorkspace: () => ipcRenderer.invoke('workspace:get'),
  pickWorkspace: () => ipcRenderer.invoke('workspace:pick'),
  readDir: (relDir) => ipcRenderer.invoke('workspace:readDir', relDir),
  openPath: (relPath) => ipcRenderer.invoke('workspace:openPath', relPath),
  getConfig: () => ipcRenderer.invoke('config:get'),
  saveConfig: (partial) => ipcRenderer.invoke('config:save', partial),
});
