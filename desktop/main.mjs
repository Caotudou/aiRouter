import { app, BrowserWindow, ipcMain, dialog, clipboard, safeStorage } from 'electron';
import { fileURLToPath } from 'node:url';
import { randomBytes } from 'node:crypto';
import { mkdir, readFile, writeFile, rename } from 'node:fs/promises';
import path from 'node:path';
import { Controller } from '../src/controller.mjs';
import { DEFAULT_MODEL, DEFAULT_REASONING_EFFORT } from '../src/defaults.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
let controller, window, quitting = false;
const single = app.requestSingleInstanceLock();
if (!single) app.quit();
else {
  app.on('second-instance', () => { window?.show(); window?.focus(); });
  boot().catch(error => { dialog.showErrorBox('AI Router 无法启动', error.message); app.exit(1); });
}
// Do not await app.whenReady at ESM module top-level: Electron needs module
// evaluation to finish before it can deliver the ready event.
async function boot() {
  await app.whenReady();
  const dataDir = app.getPath('userData');
  await mkdir(dataDir, { recursive: true });
  const configFile = path.join(dataDir, 'settings.json');
  let stored = {};
  try { stored = JSON.parse(await readFile(configFile, 'utf8')); }
  catch (e) { if (e.code !== 'ENOENT') await dialog.showMessageBox({ type: 'warning', message: '设置文件无法读取，将创建新的本机接入密钥。' }); }
  let token;
  if (stored.encryptedToken) {
    try { token = (await safeStorage.decryptStringAsync(Buffer.from(stored.encryptedToken, 'base64'))).result; }
    catch { await dialog.showMessageBox({ type: 'warning', message: '本机密钥无法解密，将生成新密钥。请重新复制到 WorkBuddy。' }); }
  }
  token ||= randomBytes(32).toString('hex');
  controller = new Controller({ dataDir, token, port: stored.port || 18321, binary: stored.binary || 'codex' });
  async function save() {
    if (!await safeStorage.isAsyncEncryptionAvailable()) throw new Error('系统凭据加密不可用，无法保存设置。');
    await writeFile(configFile + '.tmp', JSON.stringify({ port: controller.port, binary: controller.binary,
      encryptedToken: (await safeStorage.encryptStringAsync(token)).toString('base64') }, null, 2));
    await rename(configFile + '.tmp', configFile);
  }
  await save();
  function handle(channel, fn) {
    ipcMain.handle(channel, async (event, ...args) => {
      if (event.sender !== window?.webContents || event.senderFrame !== window.webContents.mainFrame) throw new Error('Invalid IPC sender.');
      return fn(...args);
    });
  }
  handle('router:status', () => controller.status());
  handle('router:refresh', () => controller.refresh());
  handle('router:start', () => controller.start());
  handle('router:stop', () => controller.stop());
  handle('router:test', () => controller.selfTest());
  handle('router:save', async settings => { const status = await controller.update(settings); await save(); return status; });
  handle('router:choose-binary', async () => {
    const result = await dialog.showOpenDialog(window, { title: '选择 codex.exe', properties: ['openFile'], filters: [{ name: 'Codex', extensions: ['exe'] }] });
    return result.canceled ? null : result.filePaths[0];
  });
  handle('router:copy', field => {
    const state = controller.status();
    const values = { url: state.baseUrl, key: token, model: DEFAULT_MODEL,
      all: `Base URL: ${state.baseUrl}\nAPI Key: ${token}\nModel: ${DEFAULT_MODEL}\nReasoning effort: ${DEFAULT_REASONING_EFFORT}` };
    if (!Object.hasOwn(values, field)) throw new Error('Unknown copy field.');
    clipboard.writeText(values[field]);
  });
  window = new BrowserWindow({ width: 1152, height: 860, minWidth: 840, minHeight: 680,
    title: 'AI Router', backgroundColor: '#f7f8fa', autoHideMenuBar: true,
    webPreferences: { preload: path.join(here, 'preload.cjs'), sandbox: true, contextIsolation: true, nodeIntegration: false } });
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  window.webContents.on('will-navigate', event => event.preventDefault());
  await window.loadFile(path.join(here, 'ui/index.html'));
  app.on('window-all-closed', () => app.quit());
  app.on('before-quit', event => {
    if (quitting) return;
    event.preventDefault(); quitting = true;
    controller.stop().finally(() => app.quit());
  });
}
