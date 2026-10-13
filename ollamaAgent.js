// ollamaAgent.js — Motor de Agente Autônomo para Ollama com Ferramentas de Workspace
// Permite que modelos locais (como qwen3.5:9b, qwen2.5-coder, etc.) leiam, editem e
// executem qualquer coisa dentro do workspace configurado no CommandDeck.

const http = require('http');
const fs = require('fs');
const path = require('path');
const { exec } = require('child_process');

const OLLAMA_HOST = '127.0.0.1';
const OLLAMA_PORT = 11434;

// Cache de conversas em memória para manter o histórico entre chamadas
const ollamaSessions = new Map();

// Garante que o caminho acessado esteja estritamente dentro do workspace configurado
function resolveWorkspacePath(relOrAbs, workspace) {
  if (!workspace) throw new Error('Workspace não configurado.');
  const normWs = path.normalize(path.resolve(workspace));
  const target = path.isAbsolute(relOrAbs)
    ? path.normalize(path.resolve(relOrAbs))
    : path.normalize(path.resolve(normWs, relOrAbs));

  // Permite acesso dentro do workspace
  if (target.toLowerCase() !== normWs.toLowerCase() && !target.toLowerCase().startsWith(normWs.toLowerCase() + path.sep)) {
    throw new Error(`Acesso negado: o caminho "${relOrAbs}" está fora do workspace configurado ("${workspace}").`);
  }
  return target;
}

// Definição das ferramentas expostas ao modelo
const WORKSPACE_TOOLS = [
  {
    type: 'function',
    function: {
      name: 'read_file',
      description: 'Lê o conteúdo textual de um arquivo dentro do workspace.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Caminho do arquivo (relativo ao workspace)' }
        },
        required: ['path']
      }
    }
  },
  {
    type: 'function',
    function: {
      name: 'write_file',
      description: 'Cria ou substitui completamente um arquivo no workspace com o novo conteúdo.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Caminho do arquivo a ser gravado (relativo ao workspace)' },
          content: { type: 'string', description: 'Conteúdo completo do arquivo' }
        },
        required: ['path', 'content']
      }
    }
  },
  {
    type: 'function',
    function: {
      name: 'edit_file',
      description: 'Substitui cirurgicamente um trecho específico de texto em um arquivo existente no workspace.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Caminho do arquivo a ser editado' },
          target: { type: 'string', description: 'Trecho exato de código/texto a ser substituído' },
          replacement: { type: 'string', description: 'Novo trecho que substituirá o alvo' }
        },
        required: ['path', 'target', 'replacement']
      }
    }
  },
  {
    type: 'function',
    function: {
      name: 'list_directory',
      description: 'Lista arquivos e diretórios dentro do workspace.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Subdiretório relativo (ou "." para a raiz do workspace)' }
        }
      }
    }
  },
  {
    type: 'function',
    function: {
      name: 'run_command',
      description: 'Executa um comando no terminal (PowerShell) dentro do diretório do workspace.',
      parameters: {
        type: 'object',
        properties: {
          command: { type: 'string', description: 'Comando de terminal a ser executado (ex: git status, npm test, ask-agent <agente> "mensagem", python script.py)' }
        },
        required: ['command']
      }
    }
  },
  {
    type: 'function',
    function: {
      name: 'delete_file',
      description: 'Remove um arquivo ou diretório do workspace.',
      parameters: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Caminho do arquivo ou pasta a ser excluído' }
        },
        required: ['path']
      }
    }
  }
];

// Execução real das ferramentas
async function executeTool(name, args, workspace) {
  try {
    switch (name) {
      case 'read_file': {
        const fullPath = resolveWorkspacePath(args.path, workspace);
        if (!fs.existsSync(fullPath)) {
          return `Erro: Arquivo "${args.path}" não encontrado no workspace.`;
        }
        const stat = fs.statSync(fullPath);
        if (stat.isDirectory()) {
          return `Erro: "${args.path}" é um diretório, use list_directory.`;
        }

        // Detecção de arquivos binários e imagens
        const ext = path.extname(fullPath).toLowerCase();
        const imageExts = new Set(['.png', '.jpg', '.jpeg', '.gif', '.bmp', '.webp', '.ico', '.svgz']);
        const binaryExts = new Set([
          '.pdf', '.zip', '.tar', '.gz', '.7z', '.rar',
          '.exe', '.dll', '.bin', '.so', '.dylib', '.iso',
          '.mp3', '.mp4', '.wav', '.avi', '.mov', '.webm',
          '.pyc', '.pyd', '.node'
        ]);

        if (imageExts.has(ext)) {
          return `[Arquivo de imagem detectado: "${args.path}" (${stat.size} bytes)]. ` +
            `A ferramenta read_file opera apenas com texto UTF-8 e não decodifica imagens. ` +
            `Para analisar ou inspecionar esta imagem visualmente, DELEGUE a análise para o Antigravity executando: ` +
            `ask-agent antigravity "Analise o arquivo de imagem ${args.path} e descreva seu conteúdo/texto" via run_command. ` +
            `Caso deseje inspecionar dimensões ou metadados localmente, execute um script em Python (Pillow) via run_command.`;
        }
        if (binaryExts.has(ext)) {
          return `[Arquivo binário detectado: "${args.path}" (${stat.size} bytes)]. ` +
            `A ferramenta read_file opera apenas com texto UTF-8. Para inspecionar ou processar este arquivo, execute comandos ou scripts no terminal com run_command.`;
        }

        if (stat.size > 250 * 1024) {
          // Arquivo muito grande, lê primeiras 1000 linhas
          const content = fs.readFileSync(fullPath, 'utf8');
          const lines = content.split(/\r?\n/).slice(0, 1000).join('\n');
          return `[Arquivo grande - exibindo primeiras 1000 linhas]\n${lines}\n... [conteúdo restante truncado]`;
        }
        return fs.readFileSync(fullPath, 'utf8');
      }

      case 'write_file': {
        const fullPath = resolveWorkspacePath(args.path, workspace);
        fs.mkdirSync(path.dirname(fullPath), { recursive: true });
        fs.writeFileSync(fullPath, args.content ?? '', 'utf8');
        return `Arquivo "${args.path}" gravado com sucesso (${Buffer.byteLength(args.content ?? '', 'utf8')} bytes).`;
      }

      case 'edit_file': {
        const fullPath = resolveWorkspacePath(args.path, workspace);
        if (!fs.existsSync(fullPath)) {
          return `Erro: Arquivo "${args.path}" não encontrado para edição.`;
        }
        const content = fs.readFileSync(fullPath, 'utf8');
        if (!content.includes(args.target)) {
          return `Erro: Trecho alvo não encontrado no arquivo "${args.path}". Use read_file para verificar o conteúdo exato antes de editar.`;
        }
        const updated = content.replace(args.target, args.replacement);
        fs.writeFileSync(fullPath, updated, 'utf8');
        return `Arquivo "${args.path}" editado com sucesso.`;
      }

      case 'list_directory': {
        const dirRel = args.path || '.';
        const fullPath = resolveWorkspacePath(dirRel, workspace);
        if (!fs.existsSync(fullPath)) {
          return `Erro: Diretório "${dirRel}" não encontrado.`;
        }
        const entries = fs.readdirSync(fullPath, { withFileTypes: true });
        const list = entries
          .filter((e) => !['.git', 'node_modules', '.gemini'].includes(e.name))
          .map((e) => {
            if (e.isDirectory()) return `[DIR]  ${e.name}/`;
            try {
              const sz = fs.statSync(path.join(fullPath, e.name)).size;
              return `[FILE] ${e.name} (${sz} bytes)`;
            } catch {
              return `[FILE] ${e.name}`;
            }
          });
        return list.length > 0 ? list.join('\n') : '(diretório vazio)';
      }

      case 'run_command': {
        const isAskAgent = /ask-agent/i.test(args.command);
        const timeoutMs = isAskAgent ? 600000 : 180000;
        // Força UTF-8 no console e na saída do PowerShell para evitar substituição de acentos por "?"
        const commandWithUtf8 = `$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(); ${args.command}`;
        return await new Promise((resolve) => {
          exec(commandWithUtf8, { cwd: workspace, shell: 'powershell.exe', encoding: 'utf8', timeout: timeoutMs }, (err, stdout, stderr) => {
            const outStr = (stdout || '').trim();
            const errStr = (stderr || '').trim();
            let result = '';
            if (outStr) result += outStr;
            if (errStr) result += (result ? '\n[stderr]\n' : '') + errStr;
            if (err) {
              if (err.killed) {
                result += (result ? '\n' : '') + `[Erro de execução: comando excedeu o tempo limite (${Math.round(timeoutMs / 1000)}s)]`;
              } else {
                result += (result ? '\n' : '') + `[Erro de execução: código ${err.code ?? 1}]`;
              }
            }
            if (!result) result = '(comando executado sem saída)';
            if (result.length > 20000) {
              result = result.slice(0, 20000) + '\n... [saída truncada]';
            }
            if (isAskAgent) {
              result += `\n\n[INSTRUÇÃO DO SISTEMA: O usuário já acompanha toda esta conversa em tempo real no painel "Conversa integrada" do CommandDeck. NÃO resuma nem relate esta resposta para o usuário e NÃO transfira dúvidas intermediárias a ele. Se a tarefa necessitar de mais discussão, esclarecimentos ou refinamento, CONTINUE conversando diretamente com o agente parceiro executando ask-agent novamente no run_command. Se já estiver alinhado, aplique as ferramentas necessárias no workspace (edit_file, write_file, run_command) até concluir a tarefa.]`;
            }
            resolve(result);
          });
        });
      }

      case 'delete_file': {
        const fullPath = resolveWorkspacePath(args.path, workspace);
        if (!fs.existsSync(fullPath)) {
          return `Erro: "${args.path}" não existe.`;
        }
        fs.rmSync(fullPath, { recursive: true, force: true });
        return `"${args.path}" excluído com sucesso.`;
      }

      default:
        return `Ferramenta desconhecida: "${name}"`;
    }
  } catch (err) {
    return `Falha ao executar ${name}: ${err.message}`;
  }
}

// Faz requisição HTTP à API do Ollama
function requestOllamaChat(payload) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(payload);
    const req = http.request(
      {
        host: OLLAMA_HOST,
        port: OLLAMA_PORT,
        path: '/api/chat',
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(data),
        },
      },
      (res) => {
        let body = '';
        res.on('data', (c) => (body += c));
        res.on('end', () => {
          if (res.statusCode !== 200) {
            return reject(new Error(`Ollama retornou HTTP ${res.statusCode}: ${body}`));
          }
          try {
            resolve(JSON.parse(body));
          } catch (e) {
            reject(new Error(`Resposta inválida do Ollama: ${body}`));
          }
        });
      }
    );
    req.on('error', (err) => {
      if (err.code === 'ECONNREFUSED') {
        reject(new Error('Ollama não está rodando. Inicie o Ollama antes de chamar este agente.'));
      } else {
        reject(err);
      }
    });
    req.write(data);
    req.end();
  });
}

// Processa imagens de anexos para envio em base64 à API do Ollama
function extractBase64Images(attachments, workspace) {
  const images = [];
  if (!Array.isArray(attachments)) return images;
  const imageExtensions = new Set(['.png', '.jpg', '.jpeg', '.webp', '.bmp', '.gif']);

  for (const att of attachments) {
    try {
      const p = path.isAbsolute(att) ? att : path.resolve(workspace, att);
      const ext = path.extname(p).toLowerCase();
      if (imageExtensions.has(ext) && fs.existsSync(p)) {
        images.push(fs.readFileSync(p).toString('base64'));
      }
    } catch {}
  }
  return images;
}

// Loop completo de agente: executa até resolver todas as chamadas de ferramentas
async function runOllamaWithTools({
  model = 'qwen3.5:9b',
  prompt,
  workspace,
  sessionKey,
  onFeed,
  extraSystem = '',
  attachments = [],
  maxSteps = 25,
}) {
  const baseSystem = [
    `Você é o Ollama Agent, um agente autônomo de inteligência artificial operando localmente dentro do CommandDeck Terminal.`,
    `Você NÃO é um chatbot isolado e NÃO está em um CLI genérico ou sem contexto. Você tem ferramentas ativas e integração profunda com o sistema.`,
    ``,
    `=== AMBIENTE E CONTEXTO OPERACIONAL ===`,
    `* Aplicação: CommandDeck Terminal (projeto "jwic") — terminal multi-agente desktop baseado em Electron e Node.js.`,
    `* Execução: Modelo local rodando no servidor Ollama (127.0.0.1:11434). Você opera com total privacidade, segurança e sem custo de nuvem.`,
    `* Workspace Atual: "${workspace}". Você tem autorização completa do usuário para ler, criar, editar e testar qualquer arquivo nesta pasta.`,
    `* Sistema Operacional: Windows (shell PowerShell).`,
    ``,
    `=== SUAS FERRAMENTAS INTEGRADAS (FUNCTION CALLING) ===`,
    `Você possui ferramentas reais ativas no workspace. Sempre que precisar realizar uma ação ou verificar o ambiente, EXECUTE a ferramenta:`,
    `- list_directory: Inspeciona diretórios e lista arquivos. Se você não sabe o que há no projeto, execute list_directory(".") para mapear a estrutura.`,
    `- read_file: Lê arquivos de texto e código (código-fonte, markdown, configs JSON/YAML, etc.).`,
    `- write_file: Cria novos arquivos ou sobrescreve arquivos inteiros.`,
    `- edit_file: Faz alterações pontuais e cirúrgicas em arquivos existentes (substituindo trechos específicos).`,
    `- run_command: Executa qualquer comando no PowerShell no diretório do workspace (npm, node, git, python, scripts, testes, etc.).`,
    `- delete_file: Exclui arquivos ou diretórios quando explicitamente instruído.`,
    ``,
    `=== ARQUIVOS BINÁRIOS E IMAGENS (PNG, JPG, PDF, ETC.) ===`,
    `* A ferramenta read_file é estritamente textual (UTF-8). Tentar ler imagens (.png, .jpg) ou binários com read_file causará erro ou conteúdo corrompido.`,
    `* Para analisar imagens, diagramas, prints de tela ou PDFs visualmente, você deve DELEGAR a um agente multimodal com visão ativa:`,
    `    Execute via run_command:`,
    `    ask-agent antigravity "Analise a imagem <caminho.png> e descreva o conteúdo/texto contido nela."`,
    `* Para extrair metadados ou fazer OCR localmente, você pode executar scripts Python (Pillow/tesseract) ou comandos PowerShell via run_command.`,
    ``,
    `=== REDE DE AGENTES E COLABORAÇÃO (ask-agent) ===`,
    `Você faz parte de uma equipe de agentes de elite no CommandDeck. Você pode consultar e colaborar com outros agentes a qualquer momento:`,
    `- antigravity: Google Antigravity CLI — parceiro de ponta para visão multimodal (imagens), planejamento arquitetural, auditoria e tarefas complexas.`,
    `- claude: Claude Code CLI — parceiro de ponta para refatoração e implementação profunda.`,
    `- gemini: Gemini CLI — parceiro multimodal e rápido para buscas e análises.`,
    `Como chamar outro agente:`,
    `  run_command: ask-agent <agente> "sua pergunta ou instrução detalhada"`,
    `Exemplo: ask-agent antigravity "Preciso de uma revisão de segurança neste script."`,
    ``,
    `=== REGRAS OBRIGATÓRIAS DE COLABORAÇÃO COM OUTROS AGENTES (CRÍTICO) ===`,
    `1. O USUÁRIO JÁ TEM ACESSO À CONVERSA INTEGRADA (NÃO PRECISA ME FALAR):`,
    `   O usuário acompanha em tempo real todas as mensagens trocadas entre os agentes na janela/painel "Conversa integrada" do CommandDeck.`,
    `   Por essa razão, você NUNCA deve relatar, retransmitir, resumir ou papagaiar para o usuário o que o outro agente (Antigravity, Claude, etc.) respondeu. O usuário já leu diretamente na interface!`,
    `2. NÃO PARE PARA PERGUNTAR AO USUÁRIO — CONTINUE CONVERSANDO COM O AGENTE (ANTIGRAVITY):`,
    `   Quando você estiver colaborando com o Antigravity (ou outro agente), NÃO pare a execução para ficar perguntando ao usuário coisas como: "o que você acha?", "como você prefere fazer?", "devo aplicar o que ele falou?", "qual opção você escolhe?".`,
    `   Em vez de fazer perguntas ao usuário, CONTINUE A CONVERSA DIRETAMENTE COM O ANTIGRAVITY! Use ask-agent antigravity novamente no run_command para tirar dúvidas, debater a solução, pedir complementos ou alinhar a implementação. Resolvam entre vocês!`,
    `3. EXECUÇÃO AUTÔNOMA NO WORKSPACE:`,
    `   Assim que você e o agente parceiro chegarem ao alinhamento técnico, aplique o código ou alterações diretamente com write_file, edit_file ou run_command. Não transfira para o usuário dúvidas técnicas que os agentes podem resolver juntos.`,
    `4. QUANDO FALAR COM O USUÁRIO:`,
    `   Fale com o usuário APENAS no final de tudo, de forma concisa e focada, avisando que a tarefa foi concluída com sucesso. Não reconte a conversa que o usuário já acompanhou na Conversa integrada.`,
    ``,
    `=== DIRETRIZES DE POSTURA E AUTONOMIA ===`,
    `1. NUNCA diga "sou apenas um modelo de linguagem", "não tenho acesso ao sistema de arquivos" ou "não posso executar comandos". Você é um agente operacional com ferramentas ativas. Use-as!`,
    `2. SEJA PROATIVO: Se o usuário pedir algo sobre o projeto e você não souber onde está, use list_directory ou leia arquivos como README.md, package.json, etc. Nunca fique perdido ou passivo.`,
    `3. AÇÃO SOBRE CONVERSAÇÃO: Quando o usuário pedir para criar, modificar código ou verificar o ambiente, execute a ferramenta imediatamente em vez de apenas explicar o que faria.`,
    `4. ANTES DE EDITAR: Leia o arquivo primeiro com read_file para garantir que o trecho a ser alterado esteja correto.`,
    `5. CONCISÃO E FOCO NO RESULTADO: Seja direto, técnico e objetivo ao responder ao usuário. Jamais resuma a conversa inter-agente já exibida na Conversa integrada.`,
    `6. PERSISTÊNCIA EM EQUIPE: Ao interagir com o Antigravity ou outro agente, continue o loop de colaboração com ele até solucionar o problema por completo. Jamais transfira o diálogo de volta para o usuário com perguntas intermediárias.`,
  ].join('\n');

  let workspaceAgentsMd = '';
  try {
    const agentsMdPath = path.join(workspace, 'AGENTS.md');
    if (fs.existsSync(agentsMdPath)) {
      workspaceAgentsMd = fs.readFileSync(agentsMdPath, 'utf8').trim();
    }
  } catch {}

  let systemContent = baseSystem;
  if (workspaceAgentsMd) {
    systemContent += `\n\n=== REGRAS DO WORKSPACE (AGENTS.md) ===\n${workspaceAgentsMd}`;
  }
  if (extraSystem) {
    systemContent += `\n\n=== INSTRUÇÃO ADICIONAL ===\n${extraSystem}`;
  }

  let messages = [];
  if (sessionKey && ollamaSessions.has(sessionKey)) {
    messages = ollamaSessions.get(sessionKey);
    // Atualiza o system prompt da sessão em memória com as diretrizes mais recentes
    if (messages.length > 0 && messages[0].role === 'system') {
      messages[0].content = systemContent;
    }
  } else {
    messages = [{ role: 'system', content: systemContent }];
  }

  // Prepara mensagem do usuário
  const userMsg = { role: 'user', content: prompt };
  const imgs = extractBase64Images(attachments, workspace);
  if (imgs.length > 0) {
    userMsg.images = imgs;
  }
  messages.push(userMsg);

  let step = 0;
  let finalContent = '';

  while (step < maxSteps) {
    step++;

    const res = await requestOllamaChat({
      model,
      stream: false,
      messages,
      tools: WORKSPACE_TOOLS,
    });

    const assistantMsg = res.message;
    if (!assistantMsg) break;

    messages.push(assistantMsg);

    // Se o modelo chamou ferramentas
    if (assistantMsg.tool_calls && assistantMsg.tool_calls.length > 0) {
      for (const call of assistantMsg.tool_calls) {
        const toolName = call.function.name;
        const toolArgs = call.function.arguments || {};
        const summaryArg = toolArgs.path || toolArgs.command || JSON.stringify(toolArgs);

        if (typeof onFeed === 'function') {
          onFeed({
            kind: 'thinking',
            from: 'ollama',
            text: `⚙️ [${toolName}] ${summaryArg}`,
          });
        }

        const toolResult = await executeTool(toolName, toolArgs, workspace);

        messages.push({
          role: 'tool',
          name: toolName,
          content: String(toolResult),
        });
      }
    } else {
      // O modelo concluiu a resposta final
      finalContent = (assistantMsg.content || '').trim();
      break;
    }
  }

  // Mantém no máximo as últimas 30 mensagens na sessão para não estourar contexto
  if (sessionKey) {
    if (messages.length > 30) {
      messages = [messages[0], ...messages.slice(-29)];
    }
    ollamaSessions.set(sessionKey, messages);
  }

  return finalContent || '(tarefa concluída)';
}

module.exports = {
  WORKSPACE_TOOLS,
  executeTool,
  resolveWorkspacePath,
  runOllamaWithTools,
  ollamaSessions,
};
