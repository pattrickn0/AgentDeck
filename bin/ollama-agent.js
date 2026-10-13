#!/usr/bin/env node
// bin/ollama-agent.js — CLI interativo do agente Ollama para o CommandDeck Terminal
const readline = require('readline');
const path = require('path');
const fs = require('fs');
const http = require('http');
const { runOllamaWithTools } = require('../ollamaAgent');

let currentModel = process.argv[2] || process.env.JWIC_OLLAMA_MODEL || 'qwen3.5:9b';
const workspace = process.cwd();
const sessionKey = `interactive-${Date.now()}`;

function fetchInstalledModels() {
  return new Promise((resolve) => {
    const host = process.env.OLLAMA_HOST || '127.0.0.1:11434';
    const parsed = host.replace(/^https?:\/\//, '').split(':');
    const hostname = parsed[0] || '127.0.0.1';
    const port = parseInt(parsed[1] || '11434', 10);
    const req = http.request(
      { host: hostname, port, path: '/api/tags', method: 'GET', timeout: 2500 },
      (res) => {
        let data = '';
        res.on('data', (d) => data += d);
        res.on('end', () => {
          try {
            const json = JSON.parse(data);
            if (Array.isArray(json?.models)) {
              return resolve(json.models.map((m) => m.name || m.model).filter(Boolean));
            }
          } catch {}
          resolve([]);
        });
      }
    );
    req.on('error', () => resolve([]));
    req.on('timeout', () => { req.destroy(); resolve([]); });
    req.end();
  });
}

// Cores ANSI para o terminal xterm
const C = {
  reset: '\x1b[0m',
  bold: '\x1b[1m',
  dim: '\x1b[2m',
  yellow: '\x1b[33m',
  green: '\x1b[32m',
  cyan: '\x1b[36m',
  blue: '\x1b[34m',
  magenta: '\x1b[35m',
  red: '\x1b[31m',
};

console.log(`\n${C.bold}${C.yellow}🦙 Ollama Agent — CommandDeck Terminal${C.reset} ${C.dim}(modelo: ${currentModel})${C.reset}`);
console.log(`${C.dim}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${C.reset}`);
console.log(`📂 ${C.bold}Workspace:${C.reset} ${workspace}`);
console.log(`🛠️  ${C.bold}Ferramentas:${C.reset} read_file, write_file, edit_file, list_directory, run_command, delete_file`);
console.log(`🤝 ${C.bold}Rede de Agentes:${C.reset} ask-agent claude | ask-agent antigravity | ask-agent gemini`);
console.log(`💡 ${C.dim}Dica: Digite '/models' para ver modelos instalados ou '/switch <modelo>' para trocar. Digite 'sair' para encerrar.${C.reset}`);
console.log(`${C.dim}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${C.reset}\n`);

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  prompt: `${C.bold}${C.yellow}ollama (${currentModel}) > ${C.reset}`,
});

let isRunning = false;

rl.prompt();

rl.on('line', async (line) => {
  const text = line.trim();
  if (!text) {
    rl.prompt();
    return;
  }
  if (text.toLowerCase() === 'sair' || text.toLowerCase() === 'exit') {
    console.log(`${C.dim}Encerrando sessão do agente Ollama.${C.reset}`);
    process.exit(0);
  }

  if (text === '/models' || text === '/model') {
    const list = await fetchInstalledModels();
    console.log(`\n${C.bold}Modelos Ollama instalados:${C.reset}`);
    if (list.length === 0) {
      console.log(`  ${C.dim}(Nenhum modelo retornado pela API HTTP)${C.reset}`);
      console.log(`  ${C.green}* ${currentModel}${C.reset} ${C.dim}(em uso)${C.reset}`);
    } else {
      for (const m of list) {
        if (m === currentModel || m.replace(/:latest$/, '') === currentModel) {
          console.log(`  ${C.green}* ${m}${C.reset} ${C.bold}(ativo)${C.reset}`);
        } else {
          console.log(`    ${m}`);
        }
      }
    }
    console.log(`\n${C.dim}Para trocar de modelo digite: /switch <nome-do-modelo>${C.reset}\n`);
    rl.prompt();
    return;
  }

  if (text.startsWith('/switch ') || text.startsWith('/model ')) {
    const target = text.replace(/^\/(?:switch|model)\s+/, '').trim();
    if (!target) {
      console.log(`${C.yellow}Uso: /switch <nome-do-modelo>${C.reset}`);
    } else {
      currentModel = target;
      console.log(`\n${C.green}✓ Modelo alterado para "${currentModel}".${C.reset}\n`);
      rl.setPrompt(`${C.bold}${C.yellow}ollama (${currentModel}) > ${C.reset}`);
    }
    rl.prompt();
    return;
  }

  if (isRunning) return;
  isRunning = true;

  try {
    const answer = await runOllamaWithTools({
      model: currentModel,
      prompt: text,
      workspace,
      sessionKey,
      onFeed: ({ text: toolText }) => {
        // Exibe progresso da ferramenta em tempo real no terminal
        console.log(`\r\x1b[K${C.cyan}${toolText}${C.reset}`);
      },
    });

    console.log(`\n${C.green}${answer}${C.reset}\n`);
  } catch (err) {
    console.log(`\n${C.red}Erro: ${err.message}${C.reset}\n`);
  } finally {
    isRunning = false;
    rl.prompt();
  }
});

rl.on('close', () => {
  process.exit(0);
});
