const api = window.jwic;
const $ = (sel) => document.querySelector(sel);

// Preenchidos a partir dos agentes configurados no processo principal.
const COLORS = {};
const NAMES = {};
const termsEl = $('#terminals');
const terminalsEmptyEl = $('#terminals-empty');
const tabsEl = $('#tabs');
const input = $('#input');
const target = $('#target');

// ---------- Notificações Sonoras (Web Audio API) ----------
let audioCtx = null;
let soundEnabled = true;

function getAudioContext() {
  if (!audioCtx) {
    const AudioContextClass = window.AudioContext || window.webkitAudioContext;
    if (AudioContextClass) audioCtx = new AudioContextClass();
  }
  if (audioCtx && audioCtx.state === 'suspended') {
    audioCtx.resume();
  }
  return audioCtx;
}

function playNotificationSound(force = false) {
  if (!soundEnabled && !force) return;
  try {
    const ctx = getAudioContext();
    if (!ctx) return;
    const now = ctx.currentTime;
    // Chime harmônico de 2 tons suaves (D5 587Hz -> A5 880Hz)
    const osc1 = ctx.createOscillator();
    const osc2 = ctx.createOscillator();
    const gain = ctx.createGain();

    osc1.type = 'sine';
    osc1.frequency.setValueAtTime(587.33, now);
    osc1.frequency.exponentialRampToValueAtTime(880, now + 0.12);

    osc2.type = 'triangle';
    osc2.frequency.setValueAtTime(880, now + 0.06);

    gain.gain.setValueAtTime(0.001, now);
    gain.gain.exponentialRampToValueAtTime(0.2, now + 0.04);
    gain.gain.exponentialRampToValueAtTime(0.001, now + 0.45);

    osc1.connect(gain);
    osc2.connect(gain);
    gain.connect(ctx.destination);

    osc1.start(now);
    osc2.start(now + 0.06);
    osc1.stop(now + 0.45);
    osc2.stop(now + 0.45);
  } catch (e) {
    console.warn('Erro ao reproduzir notificação sonora:', e);
  }
}

// ---------- Abas e terminais ----------

const tabs = new Map(); // ptyId → tab
const early = new Map(); // dados que chegaram antes da aba ser registrada
const order = []; // abas na ordem da barra
let active = null;
let isRestoring = false;
let persistTimer = null;

function persistTabs() {
  if (isRestoring) return;
  if (persistTimer) clearTimeout(persistTimer);
  persistTimer = setTimeout(persistTabsNow, 120);
}

function persistTabsNow() {
  if (isRestoring) return;
  if (persistTimer) clearTimeout(persistTimer);
  const openTabs = order.map((t) => t.kind);
  const activeTabIndex = active ? order.indexOf(active) : -1;
  api.saveConfig({ openTabs, activeTabIndex });
}

function updateEmptyState() {
  if (terminalsEmptyEl) {
    terminalsEmptyEl.hidden = order.length > 0;
  }
}

function makeTerminal() {
  const term = new Terminal({
    fontFamily: '"Cascadia Mono", Consolas, monospace',
    fontSize: 14,
    cursorBlink: true,
    scrollback: 10000,
    theme: { background: '#0b0b0d', foreground: '#e6e6ea', cursor: '#e6e6ea', selectionBackground: '#3a3a48' },
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.onBell(() => playNotificationSound());
  return { term, fit };
}

async function openTab(kind, opts = {}) {
  if (!COLORS[kind] && (kind.startsWith('ollama:') || kind === 'ollama')) {
    COLORS[kind] = '#f59e0b';
    NAMES[kind] = kind.startsWith('ollama:') ? `Ollama (${kind.slice(7)})` : 'Ollama';
  }

  const el = document.createElement('div');
  el.className = 'term';
  termsEl.append(el);
  const { term, fit } = makeTerminal();
  term.open(el);

  const tab = { kind, el, term, fit, alive: false, id: null, busy: false, idleTimer: null };
  tab.btn = document.createElement('button');
  tab.btn.className = 'tab';
  tab.btn.setAttribute('role', 'tab');
  tab.btn.style.setProperty('--tab-color', COLORS[kind] || '#9aa0a6');
  const sameKind = [...tabs.values()].filter((t) => t.kind === kind).length;
  const labelText = (NAMES[kind] || kind) + (sameKind ? ` ${sameKind + 1}` : '');
  tab.btn.innerHTML = `<span class="dot" style="background:${COLORS[kind] || '#9aa0a6'}"></span><span class="label"></span><span class="close" title="Fechar">×</span>`;
  tab.btn.querySelector('.label').textContent = labelText;
  tab.btn.title = labelText;
  tab.btn.addEventListener('click', (e) => (e.target.classList.contains('close') ? closeTab(tab) : activate(tab)));
  tabsEl.append(tab.btn);
  order.push(tab);

  term.onData((d) => {
    if (tab.alive) {
      if (d === '\r') tab.busy = true;
      api.write(tab.id, d);
    } else if (d === '\r') start(tab);
  });
  term.attachCustomKeyEventHandler((e) => {
    // Ctrl+C copia quando há seleção; Ctrl+V cola.
    if (e.type !== 'keydown' || !e.ctrlKey) return true;
    if (e.key === 'c' && term.hasSelection()) { navigator.clipboard.writeText(term.getSelection()); return false; }
    if (e.key === 'v') {
      navigator.clipboard.readText().then(async (t) => {
        if (t && tab.alive) {
          api.write(tab.id, t);
        } else if (!t && api.pasteClipboardImage) {
          const ref = await api.pasteClipboardImage();
          if (ref) {
            attachments.push(ref);
            renderChips();
            input.focus();
            autoResizeInput();
            updateSendBtn();
          }
        }
      });
      return false;
    }
    return true;
  });

  activate(tab);
  updateEmptyState();
  if (!opts.restoring) persistTabs();
  await start(tab);
  return tab;
}

async function start(tab) {
  if (tab.id != null) tabs.delete(tab.id);
  tab.term.reset();
  fitTab(tab);
  const info = await api.spawn(tab.kind, tab.term.cols, tab.term.rows)
    .catch((e) => ({ error: e.message.replace(/^Error invoking remote method '[^']+': (Error: )?/, '') }));
  if (info.error) {
    tab.alive = false;
    tab.btn.classList.add('dead');
    tab.term.write(`\x1b[33m${info.error.replace(/\r?\n/g, '\r\n')}\x1b[0m\r\n\r\n\x1b[90m[pressione Enter para tentar de novo]\x1b[0m\r\n`);
    return;
  }
  tab.id = info.id;
  tab.alive = true;
  tab.btn.classList.remove('dead');
  tabs.set(tab.id, tab);
  const buffered = early.get(tab.id);
  if (buffered) { tab.term.write(buffered.join('')); early.delete(tab.id); }
}

function fitTab(tab) {
  if (!tab.el.classList.contains('active')) return;
  const dims = tab.fit.proposeDimensions();
  if (!dims || !dims.cols || !dims.rows) return;
  const colsChanged = tab.term.cols !== dims.cols;
  const rowsChanged = tab.term.rows !== dims.rows;
  if (!colsChanged && !rowsChanged) return;

  const buffer = tab.term.buffer.active;
  const wasAtBottom = buffer.viewportY >= buffer.baseY - 1;
  const prevViewportY = buffer.viewportY;

  try { tab.fit.fit(); } catch {}

  if (wasAtBottom) {
    tab.term.scrollToBottom();
  } else {
    tab.term.scrollToLine(Math.min(tab.term.buffer.active.baseY, prevViewportY));
  }

  if (tab.alive && (colsChanged || rowsChanged)) {
    api.resize(tab.id, dims.cols, dims.rows);
  }
}

function activate(tab) {
  for (const t of allTabs()) {
    t.el.classList.toggle('active', t === tab);
    t.btn.classList.toggle('active', t === tab);
  }
  active = tab;
  const activeColor = (tab && COLORS[tab.kind]) ? COLORS[tab.kind] : '#9aa0a6';
  if (activeColor) {
    document.documentElement.style.setProperty('--active-agent-color', activeColor);
  } else {
    document.documentElement.style.removeProperty('--active-agent-color');
  }
  updateSendBtn();
  fitTab(tab);
  tab.term.focus();
  persistTabs();
}

function allTabs() { return order; }

function closeTab(tab) {
  if (tab.alive) api.kill(tab.id);
  tabs.delete(tab.id);
  tab.term.dispose();
  tab.el.remove();
  const idx = order.indexOf(tab);
  order.splice(idx, 1);
  tab.btn.remove();
  if (active === tab) {
    const rest = allTabs();
    if (rest.length) activate(rest[Math.min(idx, rest.length - 1)]);
    else {
      active = null;
      document.documentElement.style.removeProperty('--active-agent-color');
    }
  }
  updateSendBtn();
  updateEmptyState();
  persistTabsNow();
}

api.onData(({ id, data }) => {
  const tab = tabs.get(id);
  if (tab) {
    const wasAtBottom = tab.term.buffer.active.viewportY >= tab.term.buffer.active.baseY - 1;
    tab.term.write(data, () => {
      if (wasAtBottom) {
        tab.term.scrollToBottom();
      }
    });
    if (tab.busy) {
      if (tab.idleTimer) clearTimeout(tab.idleTimer);
      tab.idleTimer = setTimeout(() => {
        if (tab.busy) {
          tab.busy = false;
          playNotificationSound();
        }
      }, 1200);
    }
  } else early.set(id, [...(early.get(id) || []), data]);
});

api.onExit(({ id, exitCode }) => {
  const tab = tabs.get(id);
  if (!tab) return;
  tab.alive = false;
  tab.btn.classList.add('dead');
  tab.term.write(`\r\n\x1b[90m[processo encerrado (código ${exitCode}) — pressione Enter para reiniciar]\x1b[0m\r\n`);
});

let termsResizeTimer = null;
new ResizeObserver(() => {
  if (termsResizeTimer) clearTimeout(termsResizeTimer);
  termsResizeTimer = setTimeout(() => {
    if (active) fitTab(active);
  }, 60);
}).observe(termsEl);

// Menu de nova aba
const menu = $('#new-tab-menu');
$('#new-tab').addEventListener('click', (e) => {
  e.stopPropagation();
  menu.hidden = !menu.hidden;
  if (!menu.hidden) {
    api.listAgents().then((agents) => {
      buildMenus(agents);
    }).catch(() => {});
  }
});

let availableAgents = [];

function renderNewTabMenu(agents) {
  menu.innerHTML = '';

  const aiAgents = agents.filter((a) => a.id !== 'shell' && (a.installed || a.id === 'claude' || a.id === 'antigravity'));
  const shellAgent = agents.find((a) => a.id === 'shell') || { id: 'shell', label: 'PowerShell', color: '#9aa0a6', installed: true };
  const list = [...aiAgents, shellAgent];

  for (const a of list) {
    const b = document.createElement('button');
    b.dataset.kind = a.id;
    b.innerHTML = `<span class="dot" style="background:${a.color}"></span>`;
    const label = a.id === 'shell' ? 'PowerShell' : (a.label + (a.installed ? '' : ' (não instalado)'));
    b.append(label);
    menu.append(b);
  }
}

function renderEmptyStateActions(agents) {
  const emptyActions = $('#empty-state-actions');
  if (!emptyActions) return;
  emptyActions.innerHTML = '';

  const aiAgents = agents.filter((a) => a.id !== 'shell' && (a.installed || a.id === 'claude' || a.id === 'antigravity'));
  const shellAgent = agents.find((a) => a.id === 'shell') || { id: 'shell', label: 'PowerShell', color: '#9aa0a6' };
  const list = [...aiAgents, shellAgent];

  for (const a of list) {
    const b = document.createElement('button');
    b.className = 'empty-agent-btn';
    b.style.setProperty('--btn-agent-color', a.color);
    b.innerHTML = `<span class="dot" style="background:${a.color}"></span>Abrir ${a.id === 'shell' ? 'PowerShell' : a.label}`;
    b.addEventListener('click', () => openTab(a.id));
    emptyActions.append(b);
  }
}

function renderTargetDropdown(agents, mainAgents, ollamaModels) {
  const currentVal = target.value;
  target.innerHTML = '';

  const groupDirect = document.createElement('optgroup');
  groupDirect.label = 'Destinatários Diretos';
  groupDirect.append(new Option('Aba ativa', 'active'));
  groupDirect.append(new Option('Todos os agentes abertos', 'all'));
  target.append(groupDirect);

  const installedMainPeers = mainAgents.filter((a) => a.peer && a.installed);
  const installedOllamaPeers = ollamaModels.filter((a) => a.peer && a.installed);

  const groupTables = document.createElement('optgroup');
  groupTables.label = 'Mesas de Orquestração';

  // 1. Mesas entre agentes principais de nuvem/CLI (ex: Claude ↔ Antigravity)
  for (const a of installedMainPeers) {
    for (const b of installedMainPeers) {
      if (a !== b) {
        groupTables.append(new Option(`Mesa: ${a.label} → ${b.label}`, `table:${a.id},${b.id}`));
      }
    }
  }

  // 2. Mesas com Ollama: utiliza o modelo local preferencial (ex: 9b > 4b)
  const primaryOllama = installedOllamaPeers.find((o) => /(9b|7b|14b|32b)/i.test(o.id)) || installedOllamaPeers[0];
  if (primaryOllama) {
    const oName = primaryOllama.model || 'Local';
    for (const m of installedMainPeers) {
      groupTables.append(new Option(`Mesa: ${m.label} ↔ Ollama (${oName})`, `table:${m.id},${primaryOllama.id}`));
    }
  }

  // 3. Se houver apenas modelos locais sem agentes de nuvem: mesa entre os 2 primeiros modelos
  if (installedMainPeers.length === 0 && installedOllamaPeers.length >= 2) {
    const o1 = installedOllamaPeers[0];
    const o2 = installedOllamaPeers[1];
    groupTables.append(new Option(`Mesa: Ollama (${o1.model}) ↔ Ollama (${o2.model})`, `table:${o1.id},${o2.id}`));
  }

  // 4. Mesa Geral (todos os agentes principais + Ollama)
  const allPeers = primaryOllama ? [...installedMainPeers, primaryOllama] : [...installedMainPeers];
  if (allPeers.length > 2) {
    const labelsSummary = installedMainPeers.map((p) => p.label).join(' + ') + (primaryOllama ? ' + Ollama' : '');
    groupTables.append(new Option(`Mesa Geral (${labelsSummary})`, `table:${allPeers.map((p) => p.id).join(',')}`));
  }

  if (groupTables.children.length > 0) {
    target.append(groupTables);
  }

  if (Array.from(target.options).some((opt) => opt.value === currentVal)) {
    target.value = currentVal;
  } else {
    target.value = 'active';
  }
}

function buildMenus(agents) {
  for (const a of agents) {
    COLORS[a.id] = a.color;
    NAMES[a.id] = a.id === 'shell' ? 'PowerShell' : a.label;
    if (a.id.startsWith('ollama:')) {
      if (!COLORS['ollama']) COLORS['ollama'] = a.color;
      if (!NAMES['ollama']) NAMES['ollama'] = 'Ollama';
    }
  }
  if (!COLORS['shell']) COLORS['shell'] = '#9aa0a6';
  NAMES['shell'] = 'PowerShell';

  availableAgents = agents.filter((a) => a.installed || a.id === 'shell' || a.id === 'claude' || a.id === 'antigravity');
  const mainAgents = availableAgents.filter((a) => !a.id.startsWith('ollama:') && a.id !== 'ollama');
  const ollamaModels = availableAgents.filter((a) => a.id.startsWith('ollama:') || a.id === 'ollama');

  renderNewTabMenu(availableAgents);
  renderEmptyStateActions(availableAgents);
  renderTargetDropdown(agents, mainAgents, ollamaModels);
}

window.addEventListener('beforeunload', () => persistTabsNow());

menu.addEventListener('click', (e) => {
  e.stopPropagation();
  const kind = e.target.closest('button')?.dataset.kind;
  if (kind) { menu.hidden = true; openTab(kind); }
});
document.addEventListener('click', () => (menu.hidden = true));

document.addEventListener('keydown', (e) => {
  if (e.ctrlKey && e.key === 't') {
    e.preventDefault();
    const fallback = (availableAgents.find((a) => a.id !== 'shell') || availableAgents[0])?.id || 'shell';
    openTab(active?.kind || fallback);
  }
  if (e.ctrlKey && e.key === 'b') { e.preventDefault(); toggleSidebar(); }
  if (e.ctrlKey && e.key === 'Tab') {
    e.preventDefault();
    const list = allTabs();
    if (list.length) activate(list[(list.indexOf(active) + (e.shiftKey ? -1 : 1) + list.length) % list.length]);
  }
  if (e.ctrlKey && e.key === 'l' && document.activeElement !== input) { e.preventDefault(); input.focus(); }
});

// ---------- Configurações e Menu ----------

const settingsBtn = $('#settings-btn');
const settingsMenu = $('#settings-menu');
const settingSound = $('#setting-sound');
const testSoundBtn = $('#setting-test-sound');

api.getConfig().then((cfg) => {
  soundEnabled = cfg.soundEnabled !== false;
  settingSound.checked = soundEnabled;
});

settingSound.addEventListener('change', () => {
  soundEnabled = settingSound.checked;
  api.saveConfig({ soundEnabled });
});

testSoundBtn.addEventListener('click', () => {
  playNotificationSound(true);
});

settingsBtn.addEventListener('click', (e) => {
  e.stopPropagation();
  settingsMenu.hidden = !settingsMenu.hidden;
});

document.addEventListener('click', (e) => {
  if (!settingsMenu.contains(e.target) && e.target !== settingsBtn) {
    settingsMenu.hidden = true;
  }
});

// ---------- Explorador de Arquivos (Barra Lateral) ----------

const sidebar = $('#sidebar');
const fileTree = $('#file-tree');
const sidebarToggleBtn = $('#sidebar-toggle-btn');
const sidebarSplitter = $('#sidebar-splitter');
const expandedDirs = new Set(['']); // Raiz começa expandida

function getChevronSvg() {
  return '<svg class="chevron-icon" viewBox="0 0 16 16" width="10" height="10"><path fill="currentColor" d="M6.22 3.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06-1.06L9.94 8 6.22 4.28a.75.75 0 0 1 0-1.06z"/></svg>';
}

function getFolderSvg(expanded) {
  if (expanded) {
    return '<svg class="folder-icon" viewBox="0 0 16 16" width="14" height="14"><path fill="#858585" d="M1.5 2A1.5 1.5 0 0 0 0 3.5v9A1.5 1.5 0 0 0 1.5 14h13a1.5 1.5 0 0 0 1.5-1.5v-7A1.5 1.5 0 0 0 14.5 4H7.414L5.707 2.293A1 1 0 0 0 5 2H1.5zm0 1H5l1.707 1.707A1 1 0 0 0 7.414 5H14.5a.5.5 0 0 1 .5.5v1.25H1V3.5a.5.5 0 0 1 .5-.5z"/></svg>';
  }
  return '<svg class="folder-icon" viewBox="0 0 16 16" width="14" height="14"><path fill="#858585" d="M14.5 3H7.71l-.85-.85L6.51 2h-5l-.5.5v11l.5.5h13l.5-.5v-10l-.5-.5zm-.5 10H2V3h4.29l.86.85.35.15H14v9z"/></svg>';
}

function getFileIconHtml(name) {
  const ext = (name.split('.').pop() || '').toLowerCase();
  switch (ext) {
    case 'js':
    case 'mjs':
    case 'cjs':
      return '<span class="item-icon badge-js">JS</span>';
    case 'json':
      return '<span class="item-icon badge-json">{ }</span>';
    case 'md':
    case 'markdown':
      return '<span class="item-icon badge-md">M↓</span>';
    case 'ts':
      return '<span class="item-icon badge-ts">TS</span>';
    case 'html':
    case 'htm':
      return '<span class="item-icon badge-html">&lt;&gt;</span>';
    case 'css':
      return '<span class="item-icon badge-css">#</span>';
    case 'py':
      return '<span class="item-icon badge-py">Py</span>';
    default:
      return '<svg class="item-icon icon-generic" viewBox="0 0 16 16" width="13" height="13"><path fill="#858585" d="M9 1H3a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7L9 1zm4 12H3V3h5v5h5v5z"/></svg>';
  }
}

async function loadDirItems(relDir, container, depth = 0) {
  container.innerHTML = '<div style="padding: 2px 16px; color: var(--muted); font-size: 11px;">Carregando…</div>';
  const items = await api.readDir(relDir);
  container.innerHTML = '';
  if (!items || !items.length) {
    if (depth === 0) {
      container.innerHTML = '<div style="padding: 6px 16px; color: var(--muted); font-size: 11px;">Pasta vazia</div>';
    }
    return;
  }

  for (const item of items) {
    const row = document.createElement('div');
    const isExpanded = item.isDirectory && expandedDirs.has(item.relPath);
    row.className = 'tree-item' + (isExpanded ? ' expanded' : '');
    row.style.setProperty('--depth', depth);

    const chevron = document.createElement('span');
    chevron.className = 'chevron';
    chevron.innerHTML = item.isDirectory ? getChevronSvg() : '';
    row.append(chevron);

    const icon = document.createElement('span');
    icon.className = 'item-icon';
    icon.innerHTML = item.isDirectory ? getFolderSvg(isExpanded) : getFileIconHtml(item.name);
    row.append(icon);

    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = item.name;
    name.title = item.relPath;
    row.append(name);

    container.append(row);

    if (item.isDirectory) {
      const childWrap = document.createElement('div');
      childWrap.className = 'tree-children';
      childWrap.hidden = !isExpanded;
      container.append(childWrap);

      if (isExpanded) {
        await loadDirItems(item.relPath, childWrap, depth + 1);
      }

      row.addEventListener('click', async (e) => {
        e.stopPropagation();
        const willExpand = !row.classList.contains('expanded');
        if (willExpand) {
          row.classList.add('expanded');
          childWrap.hidden = false;
          icon.innerHTML = getFolderSvg(true);
          expandedDirs.add(item.relPath);
          await loadDirItems(item.relPath, childWrap, depth + 1);
        } else {
          row.classList.remove('expanded');
          childWrap.hidden = true;
          icon.innerHTML = getFolderSvg(false);
          expandedDirs.delete(item.relPath);
        }
      });
    } else {
      let clickTimer = null;
      row.addEventListener('click', (e) => {
        e.stopPropagation();
        if (clickTimer) clearTimeout(clickTimer);
        clickTimer = setTimeout(() => {
          clickTimer = null;
          document.querySelectorAll('.tree-item.active').forEach((el) => el.classList.remove('active'));
          row.classList.add('active');
          const ref = item.relPath.includes(' ') ? `@"${item.relPath}"` : `@${item.relPath}`;
          const val = input.value;
          if (!val) {
            input.value = ref + ' ';
          } else if (val.endsWith(' ')) {
            input.value = val + ref + ' ';
          } else {
            input.value = val + ' ' + ref + ' ';
          }
          autoResizeInput();
          updateSendBtn();
          input.focus();
        }, 220);
      });

      row.addEventListener('dblclick', (e) => {
        e.stopPropagation();
        if (clickTimer) {
          clearTimeout(clickTimer);
          clickTimer = null;
        }
        api.openPath(item.relPath);
      });
    }
  }
}

let isRefreshingTree = false;

async function refreshFileTree() {
  if (isRefreshingTree) return;
  isRefreshingTree = true;
  try {
    const ws = await api.getWorkspace();
    const folderName = ws.split(/[\\/]/).filter(Boolean).pop() || 'ARQUIVOS';
    fileTree.innerHTML = '';

    const rootRow = document.createElement('div');
    const isRootExpanded = expandedDirs.has('');
    rootRow.className = 'tree-item root-item' + (isRootExpanded ? ' expanded' : '');
    rootRow.style.setProperty('--depth', 0);

    const chevronSpan = document.createElement('span');
    chevronSpan.className = 'chevron';
    chevronSpan.innerHTML = getChevronSvg();
    rootRow.append(chevronSpan);

    const nameSpan = document.createElement('span');
    nameSpan.className = 'name';
    nameSpan.textContent = folderName;
    rootRow.append(nameSpan);

    const rootChildren = document.createElement('div');
    rootChildren.className = 'tree-children root-children';
    rootChildren.hidden = !isRootExpanded;

    rootRow.addEventListener('click', async (e) => {
      e.stopPropagation();
      const willExpand = !rootRow.classList.contains('expanded');
      if (willExpand) {
        rootRow.classList.add('expanded');
        rootChildren.hidden = false;
        expandedDirs.add('');
        if (!rootChildren.hasChildNodes()) {
          await loadDirItems('', rootChildren, 1);
        }
      } else {
        rootRow.classList.remove('expanded');
        rootChildren.hidden = true;
        expandedDirs.delete('');
      }
    });

    fileTree.append(rootRow, rootChildren);
    if (isRootExpanded) {
      await loadDirItems('', rootChildren, 1);
    }
  } finally {
    isRefreshingTree = false;
  }
}

function updateSidebarToggleBtn() {
  const isCollapsed = sidebar.classList.contains('collapsed');
  sidebarToggleBtn.title = isCollapsed
    ? 'Expandir barra lateral (Ctrl+B)'
    : 'Recolher barra lateral (Ctrl+B)';
}

function toggleSidebar() {
  sidebar.classList.toggle('collapsed');
  updateSidebarToggleBtn();
  if (active) setTimeout(() => fitTab(active), 50);
}

sidebarToggleBtn.addEventListener('click', toggleSidebar);

if (api.onWorkspaceChanged) {
  api.onWorkspaceChanged(() => refreshFileTree());
}
window.addEventListener('focus', () => refreshFileTree());

sidebarSplitter.addEventListener('pointerdown', (e) => {
  const startX = e.clientX;
  const startW = sidebar.getBoundingClientRect().width;
  sidebarSplitter.classList.add('dragging');
  const move = (ev) => {
    const newW = Math.max(150, Math.min(480, startW + ev.clientX - startX));
    document.documentElement.style.setProperty('--sidebar-w', `${newW}px`);
  };
  const up = () => {
    sidebarSplitter.classList.remove('dragging');
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', up);
    if (active) fitTab(active);
  };
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', up);
});

// ---------- Workspace ----------

const wsBtn = $('#workspace');
const showWs = (p) => {
  wsBtn.textContent = `📁 ${p}`;
  wsBtn.title = `${p}\nClique para trocar (vale para as próximas abas)`;
  refreshFileTree();
};
api.getWorkspace().then(showWs);
wsBtn.addEventListener('click', async () => showWs(await api.pickWorkspace()));

// ---------- Conversa integrada ----------

const feedEl = $('#feed');
const bottom = $('#bottom');
const thinking = new Map();
let unseen = 0;

function whoColor(name) { return COLORS[name] || '#c9c9d1'; }
function whoName(name) { return NAMES[name] || name; }

function addFeed({ kind, from, to, text, ts }) {
  feedEl.querySelector('.empty')?.remove();
  if (from && thinking.has(from)) { thinking.get(from).remove(); thinking.delete(from); }

  const el = document.createElement('article');
  el.className = `msg ${kind}`;
  el.style.setProperty('--who', whoColor(from));
  const head = document.createElement('header');
  if (from) {
    const who = document.createElement('span');
    who.className = 'who';
    who.textContent = whoName(from);
    head.append(who);
  }
  if (to) head.append(` → ${to.split(' + ').map(whoName).join(' + ')}`);
  const time = document.createElement('time');
  time.textContent = new Date(ts || Date.now()).toLocaleTimeString('pt-BR');
  head.append(time);
  const body = document.createElement('div');
  body.className = 'body';
  body.textContent = text;
  el.append(head, body);

  const atBottom = feedEl.scrollHeight - feedEl.scrollTop - feedEl.clientHeight < 40;
  feedEl.append(el);
  if (kind === 'thinking') thinking.set(from, el);
  if (atBottom) feedEl.scrollTop = feedEl.scrollHeight;

  if (bottom.classList.contains('collapsed') && kind !== 'thinking') {
    unseen++;
    $('#feed-badge').hidden = false;
    $('#feed-badge').textContent = unseen;
  }
}

api.onFeed((entry) => {
  addFeed(entry);
  if (entry.kind === 'answer' || (entry.kind === 'system' && entry.text && entry.text.includes('Mesa encerrada'))) {
    playNotificationSound();
  }
});
const system = (text) => addFeed({ kind: 'system', text });

const feedHead = $('#feed-head');
const composer = $('#composer');

function getMinBottomHeight() {
  const headH = feedHead ? feedHead.offsetHeight : 28;
  const compH = composer ? composer.offsetHeight : 115;
  const minFeedH = 60;
  return Math.max(200, headH + compH + minFeedH);
}

function ensureBottomMinHeight() {
  if (bottom.classList.contains('collapsed')) return;
  const currentH = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--bottom-h')) || 310;
  const minH = getMinBottomHeight();
  if (currentH < minH) {
    document.documentElement.style.setProperty('--bottom-h', `${minH}px`);
  }
}

$('#feed-toggle').addEventListener('click', () => {
  const collapsed = bottom.classList.toggle('collapsed');
  $('#feed-toggle').textContent = `${collapsed ? '▸' : '▾'} Conversa integrada`;
  $('#feed-toggle').setAttribute('aria-expanded', String(!collapsed));
  if (!collapsed) {
    unseen = 0;
    $('#feed-badge').hidden = true;
    ensureBottomMinHeight();
  }
  if (active) setTimeout(() => fitTab(active), 60);
});
$('#feed-clear').addEventListener('click', () => { feedEl.innerHTML = ''; thinking.clear(); });

// Divisor arrastável entre terminal e conversa
const hSplitter = $('#splitter');
hSplitter.addEventListener('pointerdown', (e) => {
  const wasCollapsed = bottom.classList.contains('collapsed');
  hSplitter.classList.add('dragging');
  const startY = e.clientY;
  const startH = wasCollapsed ? getMinBottomHeight() : bottom.getBoundingClientRect().height;
  const move = (ev) => {
    if (bottom.classList.contains('collapsed')) {
      if (startY - ev.clientY > 10) {
        bottom.classList.remove('collapsed');
        $('#feed-toggle').textContent = '▾ Conversa integrada';
        $('#feed-toggle').setAttribute('aria-expanded', 'true');
        unseen = 0;
        $('#feed-badge').hidden = true;
      } else {
        return;
      }
    }
    const minH = getMinBottomHeight();
    const maxH = Math.max(minH, Math.min(window.innerHeight * 0.75, window.innerHeight - 140));
    const newH = Math.max(minH, Math.min(maxH, startH + startY - ev.clientY));
    document.documentElement.style.setProperty('--bottom-h', `${newH}px`);
    document.documentElement.style.setProperty('--feed-h', `${Math.max(60, newH - ((feedHead ? feedHead.offsetHeight : 28) + (composer ? composer.offsetHeight : 115)))}px`);
  };
  const up = () => {
    hSplitter.classList.remove('dragging');
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', up);
    if (active) fitTab(active);
  };
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', up);
});

// ---------- Anexos ----------

let attachments = [];
function renderChips() {
  const chips = $('#chips');
  chips.innerHTML = '';
  attachments.forEach((a, i) => {
    const chip = document.createElement('span');
    chip.className = 'chip';
    chip.textContent = a.split('/').pop().replace(/^[a-z0-9]+-/, '');
    chip.title = a;
    const x = document.createElement('button');
    x.textContent = '×';
    x.title = 'Remover';
    x.onclick = () => { attachments.splice(i, 1); renderChips(); };
    chip.append(x);
    chips.append(chip);
  });
  updateSendBtn();
  ensureBottomMinHeight();
}
$('#attach').addEventListener('click', async () => {
  attachments.push(...(await api.pickAttachments()));
  renderChips();
  input.focus();
});

// ---------- Microfone ----------

const micBtn = $('#mic');
const micCancelBtn = $('#mic-cancel');
let recorder = null;
let isRecordingCancelled = false;

async function updateMicTitle() {
  if (!recorder) micBtn.title = `Gravar áudio (transcrição: ${await api.transcribeEngine()})`;
}
updateMicTitle();
micBtn.addEventListener('mouseenter', updateMicTitle);

micCancelBtn.addEventListener('click', (e) => {
  e.stopPropagation();
  if (recorder) {
    isRecordingCancelled = true;
    micCancelBtn.hidden = true;
    recorder.stop();
  }
});

async function toWav(blob) {
  const ctx = new AudioContext();
  const audio = await ctx.decodeAudioData(await blob.arrayBuffer());
  ctx.close();
  const rate = 16000;
  const off = new OfflineAudioContext(1, Math.ceil(audio.duration * rate), rate);
  const src = off.createBufferSource();
  src.buffer = audio;
  src.connect(off.destination);
  src.start();
  const pcm = (await off.startRendering()).getChannelData(0);
  const view = new DataView(new ArrayBuffer(44 + pcm.length * 2));
  const str = (o, s) => [...s].forEach((c, i) => view.setUint8(o + i, c.charCodeAt(0)));
  str(0, 'RIFF'); view.setUint32(4, 36 + pcm.length * 2, true); str(8, 'WAVE');
  str(12, 'fmt '); view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true);
  view.setUint32(24, rate, true); view.setUint32(28, rate * 2, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true);
  str(36, 'data'); view.setUint32(40, pcm.length * 2, true);
  pcm.forEach((s, i) => view.setInt16(44 + i * 2, Math.max(-1, Math.min(1, s)) * 0x7fff, true));
  return new Uint8Array(view.buffer);
}

// Enviar durante a gravação (ou a transcrição) para o áudio, transcreve e envia em seguida.
let sendAfterTranscribe = false;

micBtn.addEventListener('click', async () => {
  if (recorder) {
    recorder.stop();
    updateSendBtn();
    return;
  }
  let stream;
  try { stream = await navigator.mediaDevices.getUserMedia({ audio: true }); }
  catch (e) { system(`Microfone indisponível: ${e.message}`); return; }
  const chunks = [];
  isRecordingCancelled = false;
  recorder = new MediaRecorder(stream);
  recorder.ondataavailable = (e) => chunks.push(e.data);
  recorder.onstop = async () => {
    stream.getTracks().forEach((t) => t.stop());
    recorder = null;
    micBtn.classList.remove('recording');
    micCancelBtn.hidden = true;
    updateSendBtn();

    if (isRecordingCancelled) {
      isRecordingCancelled = false;
      sendAfterTranscribe = false;
      updateSendBtn();
      input.focus();
      return;
    }

    micBtn.classList.add('busy');
    updateSendBtn();
    const prevPlaceholder = input.placeholder;
    input.placeholder = 'Transcrevendo áudio…';
    let transcribed = false;
    try {
      const rel = await api.saveAudio(await toWav(new Blob(chunks)));
      try {
        const { text } = await api.transcribe(rel);
        input.value = (input.value ? input.value + ' ' : '') + text.trim();
        autoResizeInput();
        transcribed = true;
      } catch (e) {
        system(`Não foi possível transcrever: ${e.message.replace(/^Error invoking remote method '[^']+': (Error: )?/, '')}.`);
      }
    } finally {
      micBtn.classList.remove('busy');
      input.placeholder = prevPlaceholder;
      input.focus();
      updateMicTitle();
      const send = sendAfterTranscribe && transcribed;
      sendAfterTranscribe = false;
      updateSendBtn();
      if (send) sendMessage();
    }
  };
  recorder.start();
  micBtn.classList.add('recording');
  micCancelBtn.hidden = false;
  updateSendBtn();
  micBtn.title = 'Parar gravação';
});

// ---------- Envio ----------

const sendBtn = $('#send');
let tableRunning = false;

// Cinza apagado quando não há nada a enviar; com texto, anexo ou gravação/transcrição ativa, assume a cor da aba ativa.
// Envio para vários agentes (ou para o terminal, que já é cinza) usa azul-claro.
function updateSendBtn() {
  const isBusy = micBtn.classList.contains('busy');
  const isRecording = micBtn.classList.contains('recording') || Boolean(recorder);
  const ready = Boolean(input.value.trim() || attachments.length || isRecording || isBusy || sendAfterTranscribe);
  const kind = target.value === 'active' && active?.kind !== 'shell' ? active?.kind : null;
  sendBtn.classList.toggle('ready', ready);
  if (kind && COLORS[kind]) sendBtn.style.setProperty('--send-color', COLORS[kind]);
  else sendBtn.style.removeProperty('--send-color');
  if (sendAfterTranscribe && (isRecording || isBusy)) {
    sendBtn.textContent = 'Enviando…';
  } else if (!tableRunning) {
    sendBtn.textContent = 'Enviar';
  }
}

target.addEventListener('change', () => { $('#rounds-wrap').hidden = !target.value.startsWith('table:'); updateSendBtn(); });

async function sendMessage() {
  if (tableRunning) { api.stopTable(); return; }
  if (recorder || micBtn.classList.contains('busy')) {
    sendAfterTranscribe = true;
    updateSendBtn();
    if (recorder) recorder.stop();
    return;
  }
  const text = input.value.trim();
  if (!text && !attachments.length) return;
  // Anexo sozinho não vai: o agente leria o arquivo sem saber o que fazer, gastando tokens à toa.
  if (!text) {
    system('Escreva uma instrução junto com os anexos (ex.: "analisa essas imagens").');
    input.classList.add('needs-text');
    input.focus();
    return;
  }
  const refs = attachments.map((a) => `@${a}`).join(' ');
  const full = [text, refs].filter(Boolean).join(' ');
  const mode = target.value;

  if (mode === 'active') {
    if (!active?.alive) { system('A aba ativa não tem um processo rodando.'); return; }
    active.busy = true;
    api.submit(active.id, full);
    active.term.focus();
  } else if (mode === 'all') {
    const agents = [...tabs.values()].filter((t) => t.alive && t.kind !== 'shell');
    if (!agents.length) { system('Nenhum agente aberto.'); return; }
    agents.forEach((t) => { t.busy = true; api.submit(t.id, full); });
    addFeed({ kind: 'user', from: 'você', to: agents.map((t) => t.kind).join(' + '), text: full });
  } else {
    const agents = mode.slice('table:'.length).split(',');
    const rounds = Math.max(1, Math.min(6, Number($('#rounds').value) || 1));
    const files = attachments;
    input.value = '';
    attachments = [];
    renderChips();
    autoResizeInput();
    updateSendBtn();
    if (bottom.classList.contains('collapsed')) $('#feed-toggle').click();
    tableRunning = true;
    sendBtn.textContent = 'Parar';
    sendBtn.classList.add('stop');
    try { await api.runTable({ task: text, agents, rounds, attachments: files }); }
    finally {
      tableRunning = false;
      sendBtn.textContent = 'Enviar';
      sendBtn.classList.remove('stop');
      updateSendBtn();
    }
    return;
  }
  input.value = '';
  attachments = [];
  renderChips();
  autoResizeInput();
  updateSendBtn();
}

function autoResizeInput() {
  const feedAtBottom = feedEl.scrollHeight - feedEl.scrollTop - feedEl.clientHeight < 30;
  input.style.height = 'auto';
  if (input.value) {
    input.style.height = `${Math.min(260, input.scrollHeight)}px`;
  } else {
    input.style.height = '';
  }
  ensureBottomMinHeight();
  if (feedAtBottom) {
    feedEl.scrollTop = feedEl.scrollHeight;
  }
}

// ---------- Colar Imagens da Área de Transferência Direto no Chat ----------

async function handlePasteImage(e) {
  const clipboardData = e.clipboardData;
  let handled = false;

  if (clipboardData) {
    const items = Array.from(clipboardData.items || []);
    const imageItems = items.filter((item) => item.type.startsWith('image/'));

    if (imageItems.length > 0) {
      handled = true;
      e.preventDefault();
      for (const item of imageItems) {
        const file = item.getAsFile();
        if (!file) continue;
        const filePath = api.getFilePath(file);
        if (filePath) {
          const added = await api.addDroppedFiles([filePath]);
          if (added && added.length) attachments.push(...added);
        } else {
          const arrayBuffer = await file.arrayBuffer();
          const ext = (file.type.split('/')[1] || 'png').replace('jpeg', 'jpg');
          const name = file.name && file.name !== 'image.png' ? file.name : `screenshot-${Date.now()}.${ext}`;
          const ref = await api.saveAttachmentBuffer({ name, bytes: new Uint8Array(arrayBuffer) });
          if (ref) attachments.push(ref);
        }
      }
    } else if (clipboardData.files && clipboardData.files.length > 0) {
      const files = Array.from(clipboardData.files);
      const imageFiles = files.filter((f) => f.type.startsWith('image/') || /\.(png|jpe?g|gif|webp|bmp|svg)$/i.test(f.name));
      if (imageFiles.length > 0) {
        handled = true;
        e.preventDefault();
        const paths = imageFiles.map((f) => api.getFilePath(f)).filter(Boolean);
        if (paths.length > 0) {
          const added = await api.addDroppedFiles(paths);
          if (added && added.length) attachments.push(...added);
        } else {
          for (const file of imageFiles) {
            const arrayBuffer = await file.arrayBuffer();
            const ref = await api.saveAttachmentBuffer({ name: file.name, bytes: new Uint8Array(arrayBuffer) });
            if (ref) attachments.push(ref);
          }
        }
      }
    }
  }

  // Fallback para clipboard nativo do Electron se não tiver texto e clipboard contiver imagem
  if (!handled && api.pasteClipboardImage) {
    const hasText = clipboardData?.getData('text');
    if (!hasText || document.activeElement !== input) {
      const ref = await api.pasteClipboardImage();
      if (ref) {
        handled = true;
        e.preventDefault();
        attachments.push(ref);
      }
    }
  }

  if (handled) {
    renderChips();
    input.focus();
    autoResizeInput();
    updateSendBtn();
  }
}

window.addEventListener('paste', handlePasteImage);

sendBtn.addEventListener('click', sendMessage);
input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); }
});
input.addEventListener('focus', autoResizeInput);
input.addEventListener('click', autoResizeInput);
input.addEventListener('input', () => {
  input.classList.remove('needs-text');
  updateSendBtn();
  autoResizeInput();
});

// ---------- Drag & Drop de Arquivos e Imagens ----------
let dragCounter = 0;
const dropOverlay = $('#drop-overlay');

window.addEventListener('dragenter', (e) => {
  e.preventDefault();
  dragCounter++;
  if (e.dataTransfer && e.dataTransfer.types && Array.from(e.dataTransfer.types).includes('Files')) {
    dropOverlay.hidden = false;
  }
});

window.addEventListener('dragover', (e) => {
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
});

window.addEventListener('dragleave', (e) => {
  e.preventDefault();
  dragCounter--;
  if (dragCounter <= 0) {
    dragCounter = 0;
    dropOverlay.hidden = true;
  }
});

window.addEventListener('drop', async (e) => {
  e.preventDefault();
  dragCounter = 0;
  dropOverlay.hidden = true;
  const files = Array.from(e.dataTransfer?.files || []);
  if (!files.length) return;
  const paths = files.map((f) => api.getFilePath(f)).filter(Boolean);
  if (paths.length) {
    const added = await api.addDroppedFiles(paths);
    if (added && added.length) {
      attachments.push(...added);
      renderChips();
      input.focus();
      autoResizeInput();
      updateSendBtn();
    }
  }
});

// ---------- Início ----------

(async () => {
  const [agents, cfg] = await Promise.all([api.listAgents(), api.getConfig()]);
  buildMenus(agents);

  const savedTabs = Array.isArray(cfg?.openTabs) && cfg.openTabs.length > 0 ? cfg.openTabs : [];
  if (savedTabs.length) {
    isRestoring = true;
    for (const kind of savedTabs) {
      if (COLORS[kind]) {
        await openTab(kind, { restoring: true });
      }
    }
    const targetIdx = typeof cfg?.activeTabIndex === 'number' && cfg.activeTabIndex >= 0 && cfg.activeTabIndex < order.length
      ? cfg.activeTabIndex
      : 0;
    if (order.length) activate(order[targetIdx]);
    isRestoring = false;
  } else {
    const first = await openTab('claude');
    await openTab('antigravity');
    if (first) activate(first);
  }

  refreshFileTree();
  updateSidebarToggleBtn();
})();
