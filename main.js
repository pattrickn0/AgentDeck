// CommandDeck Terminal — processo principal.
// Roda os CLIs oficiais (Claude Code, Antigravity CLI) em pseudo-terminais e
// expõe uma ponte HTTP local para que os agentes conversem entre si.
const { app, BrowserWindow, ipcMain, dialog, shell, clipboard } = require('electron');
const path = require('path');
const fs = require('fs');
const os = require('os');
const http = require('http');
const https = require('https');
const crypto = require('crypto');
const { spawn, execFileSync } = require('child_process');
const pty = require('@lydell/node-pty');

const BIN_DIR = path.join(__dirname, 'bin');
// Tudo o que é do CommandDeck (anexos, prompts longos) fica na pasta do app.
const DATA_DIR = path.join(__dirname, 'dados');
const SESSIONS_FILE = path.join(DATA_DIR, 'sessions.json');
const CURRENT_SESSION = Date.now().toString(36);
const DEFAULT_WORKSPACE = path.join(__dirname, 'workspace');

const CONFIG_PATH = () => path.join(app.getPath('userData'), 'config.json');
const BRIDGE_TOKEN = crypto.randomBytes(16).toString('hex');
const MAX_DEPTH = 2; // agente → agente → agente, no máximo

let win = null;
let bridgePort = 0;
// Por padrão os agentes trabalham na pasta workspace/ isolada do app.
let config = { workspace: DEFAULT_WORKSPACE, whisperLang: 'pt', soundEnabled: true };

// ---------- Resolução dos executáveis oficiais ----------

function which(cmd) {
  try {
    return execFileSync('where.exe', [cmd], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).split(/\r?\n/).find(Boolean) || null;
  } catch { return null; }
}

const npmDir = path.join(process.env.APPDATA || '', 'npm');

function resolveClaude() {
  const candidates = [
    path.join(npmDir, 'node_modules', '@anthropic-ai', 'claude-code', 'bin', 'claude.exe'),
    path.join(os.homedir(), '.local', 'bin', 'claude.exe'),
    which('claude.exe'),
  ].filter(Boolean);
  const exe = candidates.find((p) => fs.existsSync(p));
  if (exe) return { file: exe, pre: [] };
  const cmd = which('claude.cmd') || which('claude');
  if (cmd) return { file: 'cmd.exe', pre: ['/d', '/c', 'claude'] };
  return null;
}

// Antigravity CLI (agy).
function resolveAgy() {
  const local = process.env.LOCALAPPDATA || '';
  const candidates = [
    which('agy.exe'),
    path.join(local, 'agy', 'bin', 'agy.exe'), // local padrão do instalador oficial
    path.join(os.homedir(), '.local', 'bin', 'agy.exe'),
    path.join(local, 'Programs', 'agy', 'agy.exe'),
    path.join(local, 'Programs', 'Antigravity CLI', 'agy.exe'),
    path.join(local, 'antigravity-cli', 'bin', 'agy.exe'),
    path.join(os.homedir(), '.antigravity', 'bin', 'agy.exe'),
  ].filter(Boolean);
  const exe = candidates.find((p) => fs.existsSync(p));
  if (exe) return { file: exe, pre: [] };
  return which('agy.cmd') ? { file: 'cmd.exe', pre: ['/d', '/c', 'agy'] } : null;
}

const AGENTS = {
  claude: {
    label: 'Claude',
    color: '#d97757',
    resolve: resolveClaude,
    missing: 'Claude Code CLI (claude) não encontrado. Instale com:\r\n  npm install -g @anthropic-ai/claude-code',
  },
  antigravity: {
    label: 'Antigravity',
    color: '#4f8cf7',
    resolve: resolveAgy,
    missing: 'Antigravity CLI (agy) não encontrado. Instale no PowerShell com:\r\n  irm https://antigravity.google/cli/install.ps1 | iex',
  },
  shell: { label: 'Terminal', color: '#9aa0a6', resolve: () => ({ file: 'powershell.exe', pre: ['-NoLogo'] }) },
};
const ALIASES = { agy: 'antigravity' };
const ALL_PEERS = ['claude', 'antigravity'];
// Só os agentes instalados entram na ponte.
const peers = () => ALL_PEERS.filter((id) => AGENTS[id].resolve());

function bridgeInstructions(self) {
  const others = peers().filter((p) => p !== self).join(', ');
  return [
    self
      ? `Você está rodando dentro do CommandDeck Terminal, um terminal que conecta vários agentes de IA. Seu nome aqui é "${self}".`
      : 'Você está rodando dentro do CommandDeck Terminal, um terminal que conecta vários agentes de IA. Seu nome aqui está na variável de ambiente JWIC_AGENT; nunca pergunte a si mesmo.',
    'Para conversar com outro agente, execute no shell:',
    '  ask-agent <agente> "sua mensagem"',
    `Agentes disponíveis: ${others}. A resposta é impressa no stdout e pode levar alguns minutos (use timeout de até 600000 ms).`,
    'O histórico de cada par de agentes é mantido entre chamadas. Use quando o usuário pedir para consultar ou colaborar com outro agente,',
    'ou quando uma segunda opinião for claramente útil. Sempre resuma ao usuário o que o outro agente respondeu.',
  ].join('\n');
}

// ---------- Configuração e workspace ----------

function loadConfig() {
  const legacy = path.join(app.getPath('appData'), 'orquestra', 'config.json');
  const file = fs.existsSync(CONFIG_PATH()) ? CONFIG_PATH() : legacy;
  try {
    const loaded = JSON.parse(fs.readFileSync(file, 'utf8'));
    // Se o workspace estiver configurado para o diretório raiz do usuário, redireciona para o workspace isolado
    if (loaded.workspace && path.resolve(loaded.workspace) === path.resolve(os.homedir())) {
      loaded.workspace = DEFAULT_WORKSPACE;
    }
    Object.assign(config, loaded);
  } catch {}
}

function saveConfig() {
  fs.mkdirSync(path.dirname(CONFIG_PATH()), { recursive: true });
  fs.writeFileSync(CONFIG_PATH(), JSON.stringify(config, null, 2));
}

function dataDir(name) {
  const dir = path.join(DATA_DIR, name);
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}
const attachmentsDir = () => dataDir('anexos');

// Gerenciamento e retenção de até 5 sessões (limpeza de áudios e prompts antigos)
function registerSessionAndCleanup() {
  let sessionHistory = [];
  try {
    if (fs.existsSync(SESSIONS_FILE)) {
      sessionHistory = JSON.parse(fs.readFileSync(SESSIONS_FILE, 'utf8'));
    }
  } catch {}
  if (!Array.isArray(sessionHistory)) sessionHistory = [];

  if (!sessionHistory.includes(CURRENT_SESSION)) {
    sessionHistory.push(CURRENT_SESSION);
  }
  while (sessionHistory.length > 5) {
    sessionHistory.shift();
  }
  try {
    fs.mkdirSync(DATA_DIR, { recursive: true });
    fs.writeFileSync(SESSIONS_FILE, JSON.stringify(sessionHistory, null, 2));
  } catch {}

  const activeSessions = new Set(sessionHistory);

  // 1. Apaga arquivos .wav remanescentes (não há serventia em manter áudio)
  const attDir = attachmentsDir();
  try {
    if (fs.existsSync(attDir)) {
      const files = fs.readdirSync(attDir);
      for (const f of files) {
        const full = path.join(attDir, f);
        if (f.endsWith('.wav')) {
          try { fs.unlinkSync(full); } catch {}
          continue;
        }
        const m = f.match(/^s_([a-z0-9]+)_/i);
        if (m && !activeSessions.has(m[1])) {
          try { fs.unlinkSync(full); } catch {}
        }
      }
    }
  } catch {}

  // 2. Limpa prompts de sessões antigas
  const pDir = path.join(DATA_DIR, 'prompts');
  try {
    if (fs.existsSync(pDir)) {
      const pFiles = fs.readdirSync(pDir);
      for (const f of pFiles) {
        const full = path.join(pDir, f);
        const m = f.match(/^s_([a-z0-9]+)_/i);
        if (m && !activeSessions.has(m[1])) {
          try { fs.unlinkSync(full); } catch {}
        }
      }
    }
  } catch {}
}

// Caminho para citar um arquivo aos agentes: relativo ao workspace quando está dentro dele.
function refPath(abs) {
  const rel = path.relative(config.workspace, abs);
  return (rel && !rel.startsWith('..') && !path.isAbsolute(rel) ? rel : abs).replace(/\\/g, '/');
}

// O Antigravity CLI não tem flag para anexar instruções ao prompt de sistema,
// então mantemos um bloco marcado no AGENTS.md do workspace.
// REGRA: Nunca criar AGENTS.md na pasta do usuário (os.homedir()) ou fora de projetos válidos.
function ensureAgentContext() {
  const resolvedWs = path.resolve(config.workspace);
  const resolvedHome = path.resolve(os.homedir());
  if (resolvedWs === resolvedHome) return; // NUNCA no diretório home do usuário!

  const file = path.join(config.workspace, 'AGENTS.md');
  const isAppWorkspace = resolvedWs.startsWith(path.resolve(__dirname));
  if (!isAppWorkspace && !fs.existsSync(file)) {
    return; // Não cria MD fora da pasta do app se o projeto não tiver AGENTS.md pré-existente
  }

  const block = `<!-- commanddeck:start -->\n${bridgeInstructions(null)}\n<!-- commanddeck:end -->`;
  const re = /<!-- (?:orquestra|jwic|commanddeck):start -->[\s\S]*?<!-- (?:orquestra|jwic|commanddeck):end -->/;
  let text = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : '';
  text = re.test(text) ? text.replace(re, block) : (text ? text.trimEnd() + '\n\n' : '') + block + '\n';
  fs.writeFileSync(file, text);
}

let workspaceWatcher = null;
let watcherDebounceTimer = null;

function startWorkspaceWatcher() {
  if (workspaceWatcher) {
    try { workspaceWatcher.close(); } catch {}
    workspaceWatcher = null;
  }
  if (!config.workspace || !fs.existsSync(config.workspace)) return;

  try {
    workspaceWatcher = fs.watch(config.workspace, { recursive: true }, (_eventType, filename) => {
      if (filename) {
        const normalized = filename.replace(/\\/g, '/');
        if (
          normalized.startsWith('.git') ||
          normalized.startsWith('node_modules') ||
          normalized.startsWith('.gemini') ||
          normalized.startsWith('dados') ||
          normalized.includes('/.git/') ||
          normalized.includes('/node_modules/')
        ) {
          return;
        }
      }
      if (watcherDebounceTimer) clearTimeout(watcherDebounceTimer);
      watcherDebounceTimer = setTimeout(() => {
        if (win && !win.isDestroyed()) {
          win.webContents.send('workspace:changed');
        }
      }, 300);
    });
    workspaceWatcher.on('error', (err) => {
      console.warn('Workspace watcher error:', err);
    });
  } catch (err) {
    console.warn('Falha ao iniciar workspace watcher:', err);
  }
}

function prepareWorkspace() {
  fs.mkdirSync(config.workspace, { recursive: true });
  ensureAgentContext();
  startWorkspaceWatcher();
}

function agentEnv(agentId, depth = 0) {
  const env = { ...process.env };
  delete env.CLAUDECODE;
  delete env.CLAUDE_CODE_ENTRYPOINT;
  delete env.CLAUDE_CODE_CHILD_SESSION;
  delete env.CLAUDE_CODE_SSE_PORT;
  delete env.ELECTRON_RUN_AS_NODE;
  env.PATH = `${BIN_DIR};${env.PATH || env.Path || ''}`;
  delete env.Path;
  env.JWIC_PORT = String(bridgePort);
  env.JWIC_TOKEN = BRIDGE_TOKEN;
  env.JWIC_AGENT = agentId;
  env.JWIC_DEPTH = String(depth);
  env.JWIC_NODE = process.execPath; // o ask-agent roda no Node do Electron, sem exigir Node instalado
  env.TERM = 'xterm-256color';
  env.COLORTERM = 'truecolor';
  return env;
}

// ---------- Terminais interativos ----------

const ptys = new Map();
let nextPtyId = 1;

function send(channel, payload) {
  if (win && !win.isDestroyed()) win.webContents.send(channel, payload);
}

ipcMain.handle('pty:spawn', (_e, { kind, cols, rows }) => {
  const def = AGENTS[kind];
  if (!def) throw new Error(`Agente desconhecido: ${kind}`);
  const resolved = def.resolve();
  if (!resolved) throw new Error(def.missing);
  const { file, pre } = resolved;
  const args = [...pre];
  if (kind === 'claude') args.push('--append-system-prompt', bridgeInstructions('claude'));
  if (kind !== 'shell') args.push('--add-dir', DATA_DIR);
  const id = nextPtyId++;
  const proc = pty.spawn(file, args, {
    name: 'xterm-256color',
    cols: cols || 120,
    rows: rows || 30,
    cwd: config.workspace,
    env: agentEnv(kind),
    useConpty: true,
  });
  proc._kind = kind;
  proc.onData((data) => send('pty:data', { id, data }));
  proc.onExit(({ exitCode }) => { ptys.delete(id); send('pty:exit', { id, exitCode }); });
  ptys.set(id, proc);
  return { id, kind, label: def.label, color: def.color };
});

ipcMain.on('pty:write', (_e, { id, data }) => ptys.get(id)?.write(data));

// Throttle/debounce de 60ms para evitar inundação de chamadas ResizePseudoConsole no ConPTY
const resizeTimers = new Map();
ipcMain.on('pty:resize', (_e, { id, cols, rows }) => {
  if (resizeTimers.has(id)) clearTimeout(resizeTimers.get(id));
  resizeTimers.set(id, setTimeout(() => {
    resizeTimers.delete(id);
    try { ptys.get(id)?.resize(Math.max(cols, 2), Math.max(rows, 2)); } catch {}
  }, 60));
});

ipcMain.on('pty:kill', (_e, { id }) => { try { ptys.get(id)?.kill(); } catch {} ptys.delete(id); });

// Envia texto como se o usuário tivesse colado e apertado Enter.
ipcMain.on('pty:submit', (_e, { id, text }) => {
  const p = ptys.get(id);
  if (!p) return;
  p.write(`\x1b[200~${text}\x1b[201~`);
  setTimeout(() => p.write('\r'), 250);
  // Se contiver menção a arquivo (@), CLIs interativos (como Antigravity e Claude)
  // abrem o menu de autocomplete de arquivo. O primeiro \r confirma a seleção do autocomplete,
  // e o segundo \r (após o fechamento do menu) submete o prompt de fato.
  if (text.includes('@') && p._kind !== 'shell') {
    setTimeout(() => p.write('\r'), 600);
  }
});

// ---------- Execução headless (modo -p) ----------

const running = new Set();
const sessions = new Map(); // chave do par → id da sessão/conversa headless
const MAX_ARG_PROMPT = 24000; // a linha de comando do Windows tem limite de ~32 mil caracteres
const HEADLESS_TIMEOUT_MS = 10 * 60 * 1000; // 10 minutos de timeout máximo

// O agy -p recebe o prompt pela linha de comando; prompts longos vão para um arquivo na pasta de dados.
function agyPromptArg(prompt) {
  if (prompt.length <= MAX_ARG_PROMPT) return prompt;
  const file = path.join(dataDir('prompts'), `s_${CURRENT_SESSION}_prompt-${Date.now()}.md`);
  fs.writeFileSync(file, prompt);
  return `A mensagem completa está no arquivo ${refPath(file)}. Leia o arquivo inteiro e responda seguindo o que ele pede.`;
}

// Extrai o texto da resposta e o id da conversa da saída --output-format json do agy.
function parseAgyOutput(out) {
  let json;
  try { json = JSON.parse(out); } catch {
    const line = out.trim().split(/\r?\n/).reverse().find((l) => l.trim().startsWith('{'));
    try { json = line && JSON.parse(line); } catch {}
  }
  if (!json || typeof json !== 'object') return { text: out.trim(), id: null };
  let text = null, id = null, status = json.status;
  const walk = (o) => {
    if (!o || typeof o !== 'object') return;
    for (const [k, v] of Object.entries(o)) {
      if (typeof v === 'string') {
        if (!id && /^(conversation|session)[_-]?id$/i.test(k)) id = v;
        if (!text && /^(response|result|text|output|answer)$/i.test(k)) text = v;
      } else walk(v);
    }
  };
  walk(json);
  if (status && /^(ERROR|INVALID|CANCELED)$/i.test(status)) {
    throw new Error(json.error?.message || json.error || text || `agy retornou status ${status}`);
  }
  return { text: (text ?? out).trim(), id };
}

function runHeadless(agent, prompt, { sessionKey, depth = 0, extraSystem } = {}) {
  const resolved = AGENTS[agent].resolve();
  if (!resolved) return Promise.reject(new Error(AGENTS[agent].missing));
  const { file, pre } = resolved;
  const args = [...pre];
  let sid = sessionKey ? sessions.get(sessionKey) : null;
  const isNew = sessionKey && !sid;
  let stdin = prompt;

  if (agent === 'claude') {
    if (isNew) sid = crypto.randomUUID();
    args.push('-p', '--output-format', 'text');
    if (sid) args.push(isNew ? '--session-id' : '--resume', sid);
    args.push('--append-system-prompt', extraSystem || bridgeInstructions('claude'), '--add-dir', DATA_DIR);
  } else if (agent === 'antigravity') {
    args.push('-p', agyPromptArg(prompt), '--output-format', 'json', '--print-timeout', '10m', '--add-dir', DATA_DIR);
    if (sid) args.push('--conversation', sid);
    stdin = '';
  }

  return new Promise((resolve, reject) => {
    const child = spawn(file, args, { cwd: config.workspace, env: agentEnv(agent, depth), windowsHide: true });
    running.add(child);
    let out = '', err = '';

    const timeoutTimer = setTimeout(() => {
      try { child.kill(); } catch {}
      running.delete(child);
      reject(new Error(`agente "${agent}" excedeu o tempo limite de resposta (timeout de 10 minutos)`));
    }, HEADLESS_TIMEOUT_MS);

    child.stdout.on('data', (d) => (out += d));
    child.stderr.on('data', (d) => (err += d));
    child.on('error', (e) => {
      clearTimeout(timeoutTimer);
      running.delete(child);
      reject(e);
    });
    child.on('close', (code) => {
      clearTimeout(timeoutTimer);
      running.delete(child);
      if (code === 0 && out.trim()) {
        if (agent !== 'antigravity') {
          if (isNew) sessions.set(sessionKey, sid);
          return resolve(out.trim());
        }
        try {
          const { text, id } = parseAgyOutput(out);
          if (sessionKey && id) sessions.set(sessionKey, id);
          return resolve(text);
        } catch (e) {
          if (!(sessionKey && !isNew)) return reject(e);
        }
      }
      if (sessionKey && !isNew) {
        sessions.delete(sessionKey);
        return runHeadless(agent, prompt, { sessionKey, depth, extraSystem }).then(resolve, reject);
      }
      const msg = (err || out).split(/\r?\n/).filter((l) => l.trim() && !/EPERM, retrying/.test(l)).slice(-6).join('\n');
      reject(new Error(msg || `processo saiu com código ${code}`));
    });
    child.stdin.end(stdin);
  });
}

function feed(entry) { send('feed', { ts: Date.now(), ...entry }); }

// ---------- Ponte HTTP: agente → agente ----------

const bridgeQueues = new Map(); // sessionKey -> Promise chain para evitar concorrência desordenada

function startBridge() {
  const server = http.createServer((req, res) => {
    const reply = (status, body) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
    if (req.headers['x-jwic-token'] !== BRIDGE_TOKEN) return reply(403, { error: 'token inválido' });

    // Validação de Host contra DNS Rebinding
    const host = req.headers.host || '';
    if (bridgePort && host && host !== `127.0.0.1:${bridgePort}` && host !== `localhost:${bridgePort}`) {
      return reply(403, { error: 'host inválido' });
    }

    if (req.method === 'GET' && req.url === '/agents') return reply(200, { agents: peers() });
    if (req.method !== 'POST' || req.url !== '/ask') return reply(404, { error: 'rota desconhecida' });

    let body = '';
    req.on('data', (d) => (body += d));
    req.on('end', async () => {
      let msg;
      try { msg = JSON.parse(body); } catch { return reply(400, { error: 'JSON inválido' }); }
      const from = String(msg.from || 'usuário');
      const asked = String(msg.to || '').toLowerCase();
      const to = ALIASES[asked] || asked;
      const depth = Number(msg.depth || 0);
      const available = peers();
      if (!available.includes(to)) return reply(400, { error: `agente desconhecido ou não instalado "${asked}". Disponíveis: ${available.join(', ')}` });
      if (to === from) return reply(400, { error: 'um agente não pode perguntar a si mesmo' });
      if (depth >= MAX_DEPTH) return reply(400, { error: 'profundidade máxima de conversa entre agentes atingida' });

      feed({ kind: 'ask', from, to, text: msg.message });
      const prompt = `[Mensagem do agente "${from}" recebida pelo CommandDeck]\n\n${msg.message}\n\nResponda diretamente; sua resposta será entregue ao agente "${from}".`;

      const sessionKey = `${from}->${to}`;
      const prevPromise = bridgeQueues.get(sessionKey) || Promise.resolve();
      const task = prevPromise.then(async () => {
        return await runHeadless(to, prompt, { sessionKey, depth: depth + 1 });
      });
      bridgeQueues.set(sessionKey, task.catch(() => {}));

      try {
        const answer = await task;
        feed({ kind: 'answer', from: to, to: from, text: answer });
        reply(200, { answer });
      } catch (e) {
        feed({ kind: 'error', from: to, to: from, text: e.message });
        reply(500, { error: e.message });
      }
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => { bridgePort = server.address().port; resolve(); }));
}

// ---------- Mesa de orquestração (vários agentes em rodadas) ----------

let tableAbort = false;

ipcMain.handle('table:run', async (_e, { task, agents, rounds, attachments }) => {
  tableAbort = false;
  const transcript = [];
  const refs = (attachments || []).map((a) => `@${a}`).join(' ');
  feed({ kind: 'user', from: 'você', to: agents.join(' + '), text: task + (refs ? `\n${refs}` : '') });
  for (let r = 1; r <= rounds && !tableAbort; r++) {
    for (const agent of agents) {
      if (tableAbort) break;
      const others = agents.filter((a) => a !== agent).join(', ');
      const history = transcript.map((t) => `### ${t.agent} (rodada ${t.round})\n${t.text}`).join('\n\n');
      const prompt = [
        `Você participa de uma mesa de colaboração com: ${others}. O usuário acompanha a conversa.`,
        `Tarefa do usuário:\n${task}${refs ? `\nArquivos: ${refs}` : ''}`,
        history ? `Conversa até agora:\n\n${history}` : 'Você é o primeiro a falar.',
        `Agora é sua vez (rodada ${r} de ${rounds}). Contribua, critique e construa sobre o que os outros disseram. Seja objetivo.` +
          (r === rounds && agent === agents[agents.length - 1] ? ' Como último a falar, feche com uma síntese final da mesa.' : ''),
      ].join('\n\n');
      feed({ kind: 'thinking', from: agent, text: `rodada ${r}/${rounds}…` });
      try {
        const text = await runHeadless(agent, prompt, { extraSystem: 'Você está numa mesa de colaboração entre agentes de IA no CommandDeck.' });
        transcript.push({ agent, round: r, text });
        feed({ kind: 'table', from: agent, text });
      } catch (e) {
        feed({ kind: 'error', from: agent, text: e.message });
      }
    }
  }
  feed({ kind: 'system', text: tableAbort ? 'Mesa interrompida.' : 'Mesa encerrada.' });
});

ipcMain.on('table:stop', () => {
  tableAbort = true;
  for (const child of running) { try { child.kill(); } catch {} }
});

// ---------- Anexos e áudio ----------

function safeName(name) {
  return name.normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/[^\w.-]+/g, '_');
}

// Cópia assíncrona de arquivos para não bloquear a thread principal
ipcMain.handle('attach:pick', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog(win, { properties: ['openFile', 'multiSelections'] });
  if (canceled) return [];
  const dir = attachmentsDir();
  const results = [];
  for (const src of filePaths) {
    const dest = path.join(dir, `s_${CURRENT_SESSION}_${Date.now().toString(36)}-${safeName(path.basename(src))}`);
    await fs.promises.copyFile(src, dest);
    results.push(refPath(dest));
  }
  return results;
});

ipcMain.handle('attach:addFiles', async (_e, filePaths) => {
  const dir = attachmentsDir();
  const results = [];
  for (const src of (filePaths || [])) {
    try {
      if (!fs.existsSync(src)) continue;
      const stat = await fs.promises.stat(src);
      if (stat.isDirectory()) continue;
      const dest = path.join(dir, `s_${CURRENT_SESSION}_${Date.now().toString(36)}-${safeName(path.basename(src))}`);
      await fs.promises.copyFile(src, dest);
      results.push(refPath(dest));
    } catch {}
  }
  return results;
});

ipcMain.handle('attach:saveBuffer', async (_e, { name, bytes }) => {
  const dir = attachmentsDir();
  const baseName = name && name !== 'image.png' ? name : `imagem-${Date.now()}.png`;
  const ext = path.extname(baseName) || '.png';
  const cleanBase = path.basename(baseName, ext);
  const safe = `${safeName(cleanBase)}${ext}`;
  const dest = path.join(dir, `s_${CURRENT_SESSION}_${Date.now().toString(36)}-${safe}`);
  await fs.promises.writeFile(dest, Buffer.from(bytes));
  return refPath(dest);
});

ipcMain.handle('attach:pasteClipboardImage', async () => {
  try {
    const image = clipboard.readImage();
    if (!image.isEmpty()) {
      const dir = attachmentsDir();
      const dest = path.join(dir, `s_${CURRENT_SESSION}_${Date.now().toString(36)}-screenshot.png`);
      await fs.promises.writeFile(dest, image.toPNG());
      return refPath(dest);
    }
  } catch (err) {
    console.warn('Erro ao ler imagem da área de transferência:', err);
  }
  return null;
});

ipcMain.handle('audio:save', async (_e, bytes) => {
  const dest = path.join(attachmentsDir(), `audio-${Date.now()}.wav`);
  await fs.promises.writeFile(dest, Buffer.from(bytes));
  return refPath(dest);
});

// ---------- Transcrição local (whisper.cpp) ----------

const WHISPER_DIR = path.join(__dirname, 'whisper');
const WHISPER_MODEL = path.join(WHISPER_DIR, 'models', 'ggml-large-v3-turbo.bin');
const WHISPER_RELEASE = 'https://github.com/ggml-org/whisper.cpp/releases/download/b5454';
const WHISPER_PROMPT = 'CommandDeck Terminal, Claude, Antigravity, agy, Claude Code, terminal, CLI, assistente Claude, Google Antigravity, Vulkan, NVIDIA, AMD.';

function normalizeTranscription(text) {
  if (!text) return '';
  return text
    // Corrige variações fonéticas de Claude em português com contexto de comando ou nome explícito
    .replace(/\b(Cl[áa]udio|Cl[áa]ud[eê]|Cl[áa]udi[ao])\b(?=\s+(Code|CLI|agente|assistente))/gi, 'Claude')
    .replace(/\bCloud\s+(Code|CLI|agente|assistente)\b/gi, 'Claude $1')
    .replace(/\b(com o|para o|pro|ao|chama o|pergunta pro|pergunta ao)\s+Cloud\b/gi, '$1 Claude')
    .replace(/\b(com o|para o|pro|chama o|pergunta pro|pergunta ao)\s+Cl[áa]udio\b/gi, '$1 Claude')
    // Corrige variações fonéticas de Antigravity
    .replace(/\b(Anti-?crafty|Anti-?craft|Anti-?graft(?:ing)?|Anti-?graph|Anti-gravity|Anti\s+gravity|Antigraviti)\b/gi, 'Antigravity');
}

const ENGINES = {
  cuda: { dir: 'bin-cuda', label: 'GPU NVIDIA (CUDA)', zip: `${WHISPER_RELEASE}/whisper-bin-win-cuda-12.4.0-x64.zip` },
  vulkan: { dir: 'bin-vulkan', label: 'GPU (Vulkan)' },
  cpu: { dir: 'bin-cpu', label: 'CPU', zip: `${WHISPER_RELEASE}/whisper-bin-x64.zip` },
};

let gpuNames = null;

function detectGpus() {
  if (gpuNames) return gpuNames;
  try {
    gpuNames = execFileSync('powershell.exe', ['-NoProfile', '-Command', '(Get-CimInstance Win32_VideoController).Name'],
      { encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'ignore'] })
      .split(/\r?\n/).map((s) => s.trim()).filter((s) => s && !/basic display|remote display/i.test(s));
  } catch { gpuNames = []; }
  return gpuNames;
}
const hasNvidia = () => detectGpus().some((n) => /nvidia/i.test(n));

function whisperCli(engine) {
  for (const sub of ['', 'Release']) {
    const exe = path.join(WHISPER_DIR, ENGINES[engine].dir, sub, 'whisper-cli.exe');
    if (fs.existsSync(exe)) return exe;
  }
  return null;
}

function whisperEngines() {
  if (!fs.existsSync(WHISPER_MODEL)) return [];
  const order = [...(hasNvidia() ? ['cuda'] : []), ...(detectGpus().length ? ['vulkan'] : []), 'cpu'];
  return order.filter(whisperCli);
}

function runWhisper(engine, wav) {
  const exe = whisperCli(engine);
  const args = ['-m', WHISPER_MODEL, '-f', wav, '-l', config.whisperLang || 'auto', '-nt', '-np',
    '-t', String(Math.max(1, os.cpus().length - 2)), '--prompt', WHISPER_PROMPT,
    '-mc', '0', '--carry-initial-prompt'];
  if (engine === 'cpu') args.push('-ng');
  return new Promise((resolve, reject) => {
    const child = spawn(exe, args, { cwd: path.dirname(exe), windowsHide: true });
    const timer = setTimeout(() => child.kill(), 10 * 60 * 1000);
    let out = '', err = '';
    child.stdout.on('data', (d) => (out += d));
    child.stderr.on('data', (d) => (err += d));
    child.on('error', (e) => { clearTimeout(timer); reject(e); });
    child.on('close', (code) => {
      clearTimeout(timer);
      const text = out.replace(/\[BLANK_AUDIO\]/g, ' ').replace(/\s+/g, ' ').trim();
      if (code === 0) return resolve(normalizeTranscription(text));
      reject(new Error(err.trim().split(/\r?\n/).pop() || `whisper saiu com código ${code}`));
    });
  });
}

function download(url, dest, redirects = 5) {
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { 'user-agent': 'commanddeck' } }, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location && redirects) {
        res.resume();
        return download(new URL(res.headers.location, url).toString(), dest, redirects - 1).then(resolve, reject);
      }
      if (res.statusCode !== 200) { res.resume(); return reject(new Error(`HTTP ${res.statusCode} ao baixar ${url}`)); }
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      const part = `${dest}.part`;
      const out = fs.createWriteStream(part);
      res.pipe(out);
      res.on('error', reject);
      out.on('error', reject);
      out.on('finish', () => out.close(() => { fs.renameSync(part, dest); resolve(); }));
    }).on('error', reject);
  });
}

async function downloadEngine(engine) {
  const { dir, zip } = ENGINES[engine];
  const file = path.join(WHISPER_DIR, `${engine}.zip`);
  await download(zip, file);
  const target = path.join(WHISPER_DIR, dir);
  fs.mkdirSync(target, { recursive: true });
  execFileSync(path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe'), ['-xf', file, '-C', target], { windowsHide: true });
  fs.rmSync(file, { force: true });
}

async function ensureWhisper() {
  const steps = [];
  if (!fs.existsSync(WHISPER_MODEL)) steps.push(['modelo large-v3-turbo (1,6 GB)', () => download('https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin', WHISPER_MODEL)]);
  if (!whisperCli('cpu')) steps.push(['whisper.cpp para CPU', () => downloadEngine('cpu')]);
  if (hasNvidia() && !whisperCli('cuda')) steps.push(['whisper.cpp com CUDA (~700 MB)', () => downloadEngine('cuda')]);
  for (const [what, run] of steps) {
    feed({ kind: 'system', text: `Transcrição local: baixando ${what}…` });
    try { await run(); feed({ kind: 'system', text: `Transcrição local: ${what} pronto.` }); }
    catch (e) { feed({ kind: 'system', text: `Transcrição local: falha ao baixar ${what} (${e.message}).` }); }
  }
}

ipcMain.handle('audio:engine', () => ENGINES[whisperEngines()[0]]?.label || 'indisponível');

// Transcreve e descarta o arquivo WAV imediatamente
ipcMain.handle('audio:transcribe', async (_e, ref) => {
  const wav = path.resolve(config.workspace, ref);
  const engines = whisperEngines();
  if (!engines.length) {
    fs.promises.unlink(wav).catch(() => {});
    throw new Error('a transcrição local ainda não está pronta (veja a Conversa integrada)');
  }
  try {
    for (const engine of engines) {
      try {
        const text = await runWhisper(engine, wav);
        if (!text) throw new Error('nenhuma fala detectada');
        return { text, engine: ENGINES[engine].label };
      } catch (e) {
        if (e.message === 'nenhuma fala detectada' || engine === engines.at(-1)) throw e;
        feed({ kind: 'system', text: `Whisper em ${ENGINES[engine].label} falhou (${e.message}); tentando a próxima opção.` });
      }
    }
  } finally {
    // Não há serventia em manter arquivos .wav após transcritos ou processados
    fs.promises.unlink(wav).catch(() => {});
  }
});

ipcMain.handle('agents:list', () =>
  Object.entries(AGENTS).map(([id, a]) => ({ id, label: a.label, color: a.color, installed: !!a.resolve(), peer: ALL_PEERS.includes(id) })));

// ---------- Workspace ----------

ipcMain.handle('workspace:get', () => config.workspace);
ipcMain.handle('workspace:pick', async () => {
  const { canceled, filePaths } = await dialog.showOpenDialog(win, { properties: ['openDirectory', 'createDirectory'], defaultPath: config.workspace });
  if (canceled) return config.workspace;
  config.workspace = filePaths[0];
  saveConfig();
  prepareWorkspace();
  return config.workspace;
});

ipcMain.handle('workspace:readDir', async (_e, relDir = '') => {
  const target = path.resolve(config.workspace, relDir);
  try {
    const entries = await fs.promises.readdir(target, { withFileTypes: true });
    const ignore = new Set(['node_modules', '.git', '.gemini', 'dados', '$RECYCLE.BIN']);
    const items = [];
    for (const entry of entries) {
      if (ignore.has(entry.name)) continue;
      const isDir = entry.isDirectory();
      const itemRel = path.relative(config.workspace, path.join(target, entry.name)).replace(/\\/g, '/');
      const ext = isDir ? '' : path.extname(entry.name).toLowerCase();
      items.push({
        name: entry.name,
        isDirectory: isDir,
        relPath: itemRel,
        ext: ext,
      });
    }
    items.sort((a, b) => {
      if (a.isDirectory !== b.isDirectory) return a.isDirectory ? -1 : 1;
      return a.name.localeCompare(b.name, undefined, { sensitivity: 'base' });
    });
    return items;
  } catch (e) {
    return [];
  }
});

ipcMain.handle('workspace:openPath', async (_e, relPath) => {
  const full = path.resolve(config.workspace, relPath);
  return shell.openPath(full);
});

// ---------- Configurações ----------

ipcMain.handle('config:get', () => config);
ipcMain.handle('config:save', (_e, partial) => {
  Object.assign(config, partial);
  saveConfig();
  return config;
});

// ---------- Ciclo de vida ----------

app.whenReady().then(async () => {
  loadConfig();
  prepareWorkspace();
  registerSessionAndCleanup();
  await startBridge();
  win = new BrowserWindow({
    width: 1280,
    height: 860,
    backgroundColor: '#0b0b0d',
    title: 'CommandDeck Terminal',
    autoHideMenuBar: true,
    webPreferences: { preload: path.join(__dirname, 'preload.js'), contextIsolation: true, nodeIntegration: false },
  });
  win.webContents.session.setPermissionRequestHandler((_wc, permission, cb) => cb(permission === 'media'));
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));
  win.webContents.once('did-finish-load', () => ensureWhisper());

  const smoke = process.env.JWIC_SMOKE;
  if (smoke) {
    fs.writeFileSync(path.join(smoke, 'bridge.json'), JSON.stringify({ port: bridgePort, token: BRIDGE_TOKEN }));
    setTimeout(async () => {
      if (process.env.JWIC_SMOKE_ACTION) {
        try { await win.webContents.executeJavaScript(process.env.JWIC_SMOKE_ACTION); } catch {}
        await new Promise((r) => setTimeout(r, 500));
      }
      fs.writeFileSync(path.join(smoke, 'screen.png'), (await win.webContents.capturePage()).toPNG());
    }, Number(process.env.JWIC_SMOKE_DELAY || 15000));
  }
});

app.on('window-all-closed', () => {
  if (workspaceWatcher) { try { workspaceWatcher.close(); } catch {} }
  for (const p of ptys.values()) { try { p.kill(); } catch {} }
  for (const c of running) { try { c.kill(); } catch {} }
  app.quit();
});
