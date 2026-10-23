# AgentDeck: visão geral

Ponto de partida da documentação, para pessoas e para agentes de IA que trabalham no projeto. Ele explica
**como o AgentDeck funciona por dentro** (a "ambientação" em que os agentes rodam), **o que dá para fazer**
com ele e **quais regras cada documento define**. Os detalhes ficam nos outros arquivos desta pasta.

| Documento | Leia quando |
| :-- | :-- |
| [AGENTDECK.md](AGENTDECK.md) | quiser saber para onde o produto vai, os princípios e a ordem do que vem a seguir |
| [GERAL.md](GERAL.md) (este) | quiser o mapa geral |
| [ARCHITECTURE.md](ARCHITECTURE.md) | for mexer na ponte, nas abas, nas instâncias ou no protocolo entre agentes |
| [MANUTENCAO.md](MANUTENCAO.md) | for compilar, testar, achar um arquivo ou seguir uma receita (ex.: novo agente) |
| [pendencias.md](pendencias.md) | quiser saber o que falta desenvolver, na ordem em que será feito, por área, e o que está adiado |
| [historico.md](historico.md) | quiser saber o que já foi feito (o destino de todo item concluído de pendencias.md) |
| [testes.md](testes.md) | for testar à mão, no Windows, com os agentes reais (roteiro com caixas de marcar) |

---

## 1. Ambientação: como o AgentDeck funciona

### O app

Terminal desktop nativo em Rust (winit + egui). São três executáveis, que precisam ficar **na mesma pasta**:

- `agentdeck` (crate `app`): a janela, as abas de terminal e a barra lateral direita (mesa de tarefas, Conversa
  integrada, Git e GitHub);
- `ask-agent` (crate `cli`): o comando que os agentes usam para conversar entre si;
- `ollama-agent` (crate `cli`): o agente local com ferramentas, que roda numa aba.

O "backend" é a crate `core`, sem interface. A interface só fala com ele pelos métodos de `Core` e recebe de
volta eventos `CoreEvent`.

### Pastas

| Pasta | Conteúdo |
| :-- | :-- |
| `workspace/` | o workspace antigo, do tempo em que havia uma pasta global. Hoje cada **projeto** tem a sua pasta (o `cwd` das abas dele); `workspace/` só serve de destino na migração de uma config antiga (`Config::migrate_legacy`) |
| `dados/` | dados de execução: anexos, prompts longos (`prompts/`), hooks do Claude (`hooks/`), as tarefas (`tarefas/`: um `<projeto>.json` e os `<projeto>-<id>-brief.md`/`-feedback.md` entregues aos agentes), sessões, `crash.log`, `agentdeck.log` (o log do app, gira acima de 2 MB) e as travas `.agentdeck.lock` e `.commanddeck.lock` (esta, só durante a migração do nome) |
| `whisper/` | motor e modelos do ditado por voz (whisper.cpp) |
| `%APPDATA%\AgentDeck\config.json` | configuração (na primeira abertura, copiada de `%APPDATA%\CommandDeck\config.json`, se existir). Com `AGENTDECK_HOME`, tudo isso fica dentro dessa pasta |

**Uma janela por pasta de dados:** uma segunda janela usando o mesmo `dados/` é recusada. Para abrir outra
instância isolada, use `AGENTDECK_HOME`.

### Abas são instâncias

Cada aba é uma **instância** com nome `modelo[@cargo[@nome]]`:

- `claude`: a instância principal do modelo;
- `claude@dev`: uma instância com cargo;
- `claude@dev@ana`: a segunda instância do mesmo modelo e cargo, que ganha um nome.

Além do nome, cada instância tem o **modelo de LLM** (`sonnet`, `opus`…), que não faz parte do id: aparece em cinza
ao lado do nome na aba e na barra lateral, e escolhe-se por `ask-agent --model`, pelo menu "+ Novo agente" (a seta
`›` do Claude, do Antigravity e do OpenCode lista os modelos) ou pelo padrão `roleModels` da configuração
(`{"claude@dev": "sonnet", "opencode": "opencode/glm-5.3-flash-free"}`; `modelo@cargo` vale mais que `modelo`).

O nome é o endereço usado pelo `ask-agent`. As abas voltam com o mesmo nome quando o app reabre.

### O ambiente de cada aba

| Variável | O que é |
| :-- | :-- |
| `AGENTDECK_AGENT` | o nome da instância (`claude@dev`) |
| `AGENTDECK_ROLE` | o cargo, quando há |
| `AGENTDECK_PORT` | porta da ponte HTTP local (só em `127.0.0.1`) |
| `AGENTDECK_TOKEN` | token **próprio da aba**: é por ele que a ponte sabe quem está perguntando |
| `AGENTDECK_DEPTH` | profundidade da conversa (usada no modo headless) |
| `AGENTDECK_ASK_WAIT` | quantos segundos o `ask-agent` espera a resposta (padrão 90; `0` = até o fim). O usuário pode definir |
| `PATH` | a pasta dos executáveis vem primeiro, então o `ask-agent` sempre é encontrado |

Nenhuma outra variável `AGENTDECK_*` do ambiente do app passa para os agentes.

As instruções da ponte chegam a cada agente por um canal diferente:

- **Claude:** `--append-system-prompt`;
- **Antigravity e outros:** o bloco gerado no `AGENTS.md` do workspace;
- **Ollama:** o prompt de sistema do `ollama-agent`.

### O caminho de uma pergunta entre agentes

1. Um agente roda `ask-agent claude@dev "tarefa"`.
2. A ponte resolve o destino. Se a instância não existe, a aba dela é aberta em segundo plano.
3. A mensagem é **digitada no terminal da aba de destino** quando ela fica livre:
   - nunca logo depois de uma tecla do usuário;
   - com hooks (Claude e Codex), só depois do aviso de fim de turno;
   - sem hooks, quando o terminal fica parado por 1,5 s, ou após 30 s na fila.
4. O agente de destino responde com `ask-agent --reply <n> "resposta"`. Só ele consegue responder àquela pergunta.
5. A resposta volta para quem perguntou:
   - se chegar em até 90 s, sai no stdout do `ask-agent`;
   - se não, o `ask-agent` termina avisando, e a resposta é **digitada depois** no terminal de quem perguntou.
   - Numa aba de terminal comum (`shell`) a resposta nunca é digitada, porque seria executada como comando: lá o `ask-agent` espera até o fim.

Tudo aparece na **Conversa integrada**, e o usuário pode intervir em qualquer aba.

### A janela

| Área | O que tem |
| :-- | :-- |
| Barra de título | própria (a janela não tem a do sistema): ☰ (configurações), recolher a barra esquerda (ícone de painel), "AgentDeck", "+" (nova aba); com a barra lateral recolhida, a **chave Agentes | Documentos** compacta logo depois do logo (ver abaixo); à direita, o **uso de cada agente com cota**, com ou sem janela aberta (logo + barra fina da sessão de 5 h em cima e barra grossa da semana embaixo, com as porcentagens; âmbar a partir de 80 %, vermelho em 100 %; passar o mouse abre o **painel de uso**, com todos os agentes, quanto usaram e quando renova cada janela) e minimizar/maximizar/fechar. Arrastar move a janela, duplo clique maximiza, as bordas redimensionam |
| Barra esquerda | a **chave Agentes | Documentos** no topo, **PROJETOS** (cada projeto com os seus agentes, o modelo de cada um à direita; o ativo lista as abas dele; "+" abre uma pasta como projeto; clique direito renomeia, abre a pasta ou remove), a seção **ORGANIZAÇÃO** (controle Abas | Dividida) e **ARQUIVOS**, o explorador (cinza, com a letra do Git; atualizar, recolher tudo); recolhe com Ctrl+B |
| Centro | duas visões, escolhidas pela **chave** do topo da barra lateral: **Agentes** (a **tela dividida por tipo de agente**: cada tipo, como Claude, Antigravity, Ollama, Codex ou Terminal, tem uma janela com as próprias abas; no cabeçalho de cada janela ficam também o microfone e o anexo) e **Documentos** (diff, arquivos e markdown para ler com a largura toda do centro); ver abaixo |
| Barra direita | **fixa, não recolhe**; a largura muda arrastando a borda. No topo, quatro abas com ícone: **Tarefas** (a primeira e a que abre; o número é o de tarefas esperando a sua revisão ou validação), **Conversa** (balões, **só leitura**: o feed das mensagens entre agentes, sem caixa de mensagem; com o número de mensagens novas enquanto você está em outra aba), **Git** (branch) e **GitHub** (PRs, issues e Actions; com o número de PRs abertos). Para falar com um agente, use o terminal da aba dele; o ditado e o anexo ficam no cabeçalho de cada janela |

**Projetos.** O projeto é a unidade de trabalho: uma pasta com os seus agentes. Cada aba pertence ao projeto em
que foi aberta (o terminal roda na pasta dele) e só aparece com ele na tela; trocar de projeto esconde as abas do
anterior, que **continuam rodando**. A árvore, a aba Git e a Conversa são do projeto ativo; o uso na barra
de título é global (o limite é da conta). Ao abrir o app, voltam só as abas do projeto ativo; as dos outros abrem
na primeira vez que o projeto é selecionado (ou quando um agente chama alguém de lá). Não existe mais workspace
padrão: sem projeto, o centro pede uma pasta.

A **tela dividida** organiza as abas por tipo de agente:
- abas do mesmo tipo ficam na mesma janela (2 Claude = 1 janela com 2 abas; todos os modelos do Ollama juntos;
  `claude@dev` fica na janela do Claude); um tipo novo abre mais uma janela;
- as janelas ficam numa grade automática: 2 lado a lado; 3 = 2 em cima + 1 largo embaixo; 4 = 2×2; 5 = 3+2;
  6 = 3×2. Se não cabem no tamanho mínimo (~260 × 140 px), os tipos que sobram viram abas da última janela;
- cada janela tem o cabeçalho com as abas dela, a alça ⋮⋮ (arrastar sobre outra janela troca as duas de lugar) e,
  à direita, o **microfone** (ditado), o **anexo** (escolher arquivos) e o "+" que abre outra aba do mesmo tipo;
  o microfone e o anexo agem na **aba visível da janela** (`ui/panes.rs`; a aba só mostra o indicador de
  gravação/transcrição). A janela focada tem o contorno na cor do agente e recebe o teclado e o IME;
- quando as abas de uma janela não cabem, o lado com aba escondida ganha um degradê e uma seta ‹ / › que rola a
  faixa até a aba escondida mais próxima, e um botão ▾ no fim abre o menu com todas as abas da janela (`ui/tabs.rs`);
- os divisores entre janelas se arrastam (duplo clique iguala); tamanhos, ordem e o liga/desliga ficam salvos.
  Desligada (botão na barra de título), volta a ser uma janela só com todas as abas;
- uma aba aberta por outro agente (`ask-agent codex …`) entra na janela do tipo dela (ou cria a janela) sem tirar
  o foco de onde você está digitando;
- atalhos: Ctrl+Tab / Ctrl+Shift+Tab trocam de aba dentro da janela focada; Ctrl+Alt+setas mudam o foco para a
  janela vizinha; Ctrl+T abre outra aba do tipo focado; Ctrl+Shift+D alterna entre Agentes e Documentos.

**Documentos (a chave do centro).** O diff que o usuário valida e o que ele lê (pedido, relatório, arquivos) ficavam espremidos
na barra direita (280–400 px). Por isso o **centro inteiro** alterna, em vez de ganhar uma janela ao lado dos agentes (com 3 agentes,
cada janela tem 1/3 da largura). Fase 1: **só leitura**; edição, realce de sintaxe, diff lado a lado e gravar os documentos abertos
ficam para a fase 2 (ver [pendencias.md](pendencias.md)).
- **A chave** fica no topo da barra lateral (com ela recolhida, na barra de título, logo depois do logo): dois segmentos, **Agentes** e **Documentos (n)** (n =
  documentos abertos do projeto ativo). Ctrl+Shift+D alterna (o app o toma mesmo com o terminal focado, como o Ctrl+Shift+M; sem
  documentos abertos não faz nada). Abrir qualquer documento muda para Documentos sozinho; fechar o último volta para Agentes; clicar
  num agente, "Ir para a aba"/"Reabrir aba" da mesa ou abrir uma aba nova também voltam para Agentes.
- **O selo de "Agentes"**: o que acontece nos terminais enquanto se lê. Um número sobre fundo de destaque conta as abas do projeto
  que **terminaram um turno sem você ver** (`Tab.done`; lendo documentos, até a aba ativa conta); sem nenhuma, um ponto que pulsa
  indica que alguma está **trabalhando** (`Tab.busy`), só no modo Documentos. As ações da validação (Mesclar, Devolver, Validar,
  Reprovar) ficam na barra direita, que continua visível.
- **Escondidos, os terminais continuam rodando** e o PTY **não é redimensionado** (nenhum `fit_tab` enquanto o centro mostra
  documentos: ver `claude-code-sem-esc3j-no-windows.md`); ao voltar, o terminal se ajusta ao tamanho de então, com a espera de sempre. O
  teclado, o IME e o arrastar arquivos não chegam ao terminal escondido.
- **Faixa de abas de documento**: o mesmo desenho das abas dos agentes (setas ‹ › quando não cabem, menu ▾ com todos, × em cada
  aba), um sinal do tipo antes do nome (`±` diff, `≡` arquivo, `¶` markdown). Embaixo, o título do documento e as ações
  (**Recarregar**; **Abrir no programa padrão** nos arquivos). Os documentos são **por projeto** (trocar de projeto troca a lista) e
  **não são gravados**; abrir um documento já aberto só o foca. Ctrl+Tab passa pelos documentos.
- **Diff**: lista de arquivos à esquerda (letra `A`/`M`/`D`/`R`, nome e `+`/`−`; clicar rola o diff até o arquivo) e o diff à direita,
  no mesmo desenho do diff da aba Git, sem as ações por trecho. Em janela estreita (centro < 560 px) a lista vai para cima do diff.
  Vem de: **Ver diff completo** / **Ver o que foi mesclado** do detalhe da tarefa (a barra direita não tem mais a tela do diff), o botão
  **Abrir em Documentos** do detalhe de um commit (Histórico da aba Git) e o do diff de um arquivo alterado (aba Git, que mantém o diff dela
  para preparar por trecho).
- **Arquivo**: duplo clique no explorador abre uma visualização monoespaçada, com número de linha e rolagem, que relê sozinha
  quando o arquivo muda no disco (o watcher da pasta). Acima de 1 MB ou binário, uma explicação no lugar do conteúdo. O clique simples
  continua inserindo `@arquivo` no terminal.
- **Markdown**: os `.md` abertos pelo explorador e, no detalhe da tarefa, o **Pedido** e o **Relatório** (botão **Abrir em Documentos** em
  cada um) aparecem formatados: títulos, listas (com `- [ ]`), negrito e itálico, código, tabelas e citações.

A aba **Git** trabalha no repositório da pasta de trabalho, com o git instalado na máquina:
- topo: branch atual e menu de branches (trocar, criar, **mesclar** ou fazer **rebase** da branch escolhida); buscar, puxar
  (só avanço direto) e enviar ("Publicar" quando a branch ainda não existe no remoto);
- merge, rebase, cherry-pick ou revert em andamento aparecem numa faixa no topo com **Continuar** (só sem conflitos) e **Abortar**;
- quatro visões: **Alterações**, **Histórico**, **Stash** e, só quando há conflito, **Conflitos**;
- Alterações: seções (conflitos, preparadas, alterações, não rastreados) com preparar (+), tirar (−) e descartar (↶, com
  confirmação); clique abre o diff (rola dentro da barra), que tem **Preparar / Tirar / Descartar por trecho** (hunk);
  duplo clique abre o arquivo; **Abrir em Documentos** abre o diff, só para ler, em Documentos; caixa de commit (Ctrl+Enter) com a opção de corrigir o último commit;
- Histórico: commits da branch (hash, autor, data, assunto; "Carregar mais") e, ao clicar, o detalhe do commit (mensagem,
  arquivos e diff, com **Abrir em Documentos**);
- Stash: guardar (com mensagem; opção de incluir não rastreados), ver o diff, aplicar, aplicar e apagar e apagar;
- Conflitos: cada arquivo mostra os blocos com "o nosso", "o deles" ou "ambos" por bloco ou no arquivo inteiro, e "Marcar como resolvido";
- commit com `commit.gpgsign` e chave travada falha na hora, explicando como destravar (o app não desliga a assinatura);
  push/pull por ssh com chave com senha explicam o ssh-agent;
- sem repositório: botão "Inicializar repositório"; sem git: o link para instalar.

Atualiza sozinha quando o `.git/` muda (commit ou checkout feitos pelo terminal ou por um agente), quando a
pasta muda e quando a janela volta a ter foco. Nada de agentes por enquanto: tudo é manual.

A aba **Tarefas** é a mesa de tarefas do projeto ativo (o projeto precisa ser um repositório git com ao menos um
commit, e "Abrir abas dos agentes" ligado). Uma tarefa é um pedido que um agente executa numa branch e numa pasta
(worktree) só dela. O ciclo: em andamento → quando o agente entrega o relatório, **em revisão** → ao mesclar, **aguardando
validação** (o agente, a aba e a pasta continuam, para você testar o que foi mesclado) → ao validar, **concluída**; reprovar
na validação devolve a tarefa ao mesmo agente:
- **Lista** agrupada por estado: *Em revisão*, *Aguardando validação*, *Em andamento*, *Rascunho*, *Falhou*, *Concluídas* e
  *Canceladas* (as concluídas e as descartadas, nessa ordem, recolhidas até você abrir). Cada cartão mostra o estado (o ponto), `tN`, o título, o agente com o
  modelo e o tempo ("pronta há 3 min"); na revisão, também o resumo do diff ("3 arquivos, +120 −8");
- **+ Nova** abre o formulário: título, pedido, agente (os instalados), modelo ("Padrão" ou os conhecidos do
  agente), base (a branch atual), anexos ("Anexar arquivos", ou arrastar arquivos sobre o formulário: os caminhos vão ao
  final do pedido) e "Iniciar agora". Ctrl+Enter cria; sem "Iniciar agora" a tarefa fica em rascunho;
- **Detalhe** (clicar no cartão; "←" volta): estado, `branch → base`, agente, o pedido, o relatório, as alterações
  (resumo por arquivo e **Ver diff completo**, que abre o diff no centro, em Documentos; aguardando validação e concluída, com o hash curto da mesclagem ao lado de `branch → base`, o botão vira **Ver o que foi mesclado** e abre o que a mesclagem trouxe) e o histórico. Os
  botões dependem do estado: rascunho **Iniciar** e **Descartar**; em andamento **Ir para a aba** (ou **Reabrir
  aba**, se você a fechou) e **Descartar**; em revisão **Mesclar em \<base>**, **Devolver** (escreve o que mudar e
  envia ao agente), a aba e **Descartar**; aguardando validação **Validar** (limpa a aba, a pasta e a branch),
  **Reprovar** (escreve o que falhou no teste e volta ao mesmo agente), a aba e **Descartar** (o que já foi mesclado
  continua na base); com erro **Tentar de novo** e **Descartar**. Mesclar e descartar pedem
  confirmação no painel (validar não pede); enquanto a operação roda os botões ficam desligados, e uma recusa (repositório com
  alterações sem commit, conflito…) aparece numa faixa vermelha com a explicação do core;
- **Memória**: ao começar (e a cada devolução ou reprovação), o agente recebe o que o projeto já aprendeu
  (`memory/aprendizados/`) e a "Situação atual" (as outras tarefas em andamento, os arquivos que elas mexem e os
  últimos commits da base); ver "Memória em duas camadas" em [ARCHITECTURE.md](ARCHITECTURE.md).

A aba **GitHub** mostra o repositório do remoto `origin` do projeto ativo (sem remoto do GitHub, ela só explica):
- **PRs** abertos (número, título, autor, branch, rascunho e o estado dos checks), **Issues** abertas (sem os PRs) e
  as últimas execuções do **Actions**; clicar abre o detalhe embaixo, com os checks/jobs e, para o job vermelho, o
  fim do log (rola dentro da barra);
- **Abrir no navegador** e **Enviar para agente**: um menu com os agentes abertos do projeto e a opção de abrir (ou
  reaproveitar) uma instância com cargo (`claude@review`); a mensagem já vem pronta ("Revise o PR #12…", "Corrija o
  job … que falhou…, com o fim do log") e entra na aba como qualquer mensagem de outro agente;
- sem guardar token: usa o login do Git Credential Manager (o mesmo do `git push`) ou o `gh`, se estiver instalado;
  sem login só lê repositórios públicos (60 consultas por hora). Atualiza ao abrir a aba (cache de 60 s) e pelo botão.

### A ponte HTTP em resumo

O `ask-agent` é um cliente da ponte local; o contrato completo (corpos, códigos HTTP, permissões) está em
[ARCHITECTURE.md](ARCHITECTURE.md).

| Rota | Para que serve | Comando |
| :-- | :-- | :-- |
| `GET /agents` | agentes instalados e instâncias abertas, com o modelo | `--list` |
| `GET /models[?agent=]` | inventário dos modelos de cada agente | `--models` |
| `GET /team` | o seu chefe e a sua equipe | `--team` |
| `POST /ask` | pergunta a um agente (abre a aba se preciso) | `<agente> "msg"`, `--new`, `--model`, `--boss` |
| `POST /reply` | responde a uma pergunta pendente | `--reply <n>` |
| `POST /signal` | início e fim de turno (hooks) | `--signal` |
| `POST /usage` | uso do plano (statusline do Claude) | `--usage` |
| `POST /learn` | o que o projeto já aprendeu | `--learn` |
| `GET /inbox` | as respostas recebidas por quem chama (só token de aba) | `--inbox` |
| `GET /tasks`, `GET /tasks/<id>` | lista e detalhe das tarefas | `--task list`, `--task show` |
| `POST /tasks` | cria uma tarefa (e a inicia, com `start`) | `--task new` |
| `POST /tasks/<id>/start`, `/merge`, `/validate`, `/discard` | inicia, mescla, valida e descarta | `--task start`, `merge`, `validate`, `discard` |
| `POST /tasks/<id>/done` | o agente entrega o relatório | `--task done` |
| `POST /tasks/<id>/return`, `/reject` | devolve (em revisão) ou reprova (aguardando validação) com feedback | `--task return`, `--task reject` |

### Limites e proteções

| Regra | Valor |
| :-- | :-- |
| Profundidade máxima de delegação (A → B → C; a terceira pergunta encadeada é recusada) | 2 |
| Ciclo de espera (A espera B, que espera A) | recusado na hora (HTTP 409) |
| Abas abertas automaticamente pelo `ask-agent` | 8 |
| Tempo máximo de uma pergunta | 10 min |
| Tamanho de uma mensagem / requisições simultâneas na ponte | 16 MB / 128 |
| Texto de outro agente digitado num terminal | sem ESC nem caracteres de controle |
| Prompt para um shim `.cmd` no Windows | vai por arquivo, nunca na linha de comando |
| Arquivo aberto no modo Documentos (`Core::read_text`) | até 1 MB, sem bytes nulos; acima ou binário, só uma explicação |

### Modo headless

Quando "Abrir abas dos agentes" está desligado, os agentes rodam sem janela (`claude -p`,
`agy -p`, `codex exec`…) e a resposta é capturada. Cada execução tem um token próprio, que vale só enquanto
ela dura.

---

## 2. O que dá para fazer

**Como usuário:**

- Abrir abas de agentes: Claude Code, Antigravity, Codex, OpenCode, Aider, Goose, Copilot, Amazon Q,
  Open Interpreter, modelos Ollama e um terminal comum.
- Organizar o trabalho em **projetos** (uma pasta cada): cada projeto tem as suas abas, que continuam rodando
  escondidas quando se troca de projeto.
- Ver os agentes de todos os projetos **em árvore** na barra lateral e reorganizar a **equipe** arrastando um
  agente sobre outro.
- Ver vários agentes ao mesmo tempo na **tela dividida** (uma janela por tipo de agente), com a chave na seção
  ORGANIZAÇÃO da barra lateral.
- Acompanhar o **uso** de cada agente com cota (sessão e semana) na barra de título, com o painel de uso ao passar o mouse.
- Falar com um agente pelo terminal da aba dele, com **anexos** (botão no cabeçalho da janela, arrastar arquivos sobre ela
  ou colar imagem) e **ditado por voz** (microfone no cabeçalho da janela).
- Acompanhar as conversas entre agentes pela Conversa integrada (só leitura) e intervir pelas próprias abas.
- Ler com conforto, no centro (chave **Agentes | Documentos**, Ctrl+Shift+D), o diff de uma tarefa, de um commit ou de um arquivo,
  o pedido e o relatório de uma tarefa e os arquivos do projeto (duplo clique no explorador), sem tirar os agentes de funcionar.
- Dar **tarefas** a agentes pela aba Tarefas (cada uma numa branch e pasta próprias), revisar o relatório e o diff e
  **mesclar**, **devolver** com ajustes ou **descartar**; depois de mesclar, **validar** ou **reprovar** (a tarefa volta
  ao mesmo agente).
- Usar o **git** do projeto pela aba Git: alterações e diffs (inclusive por trecho), commit, histórico, stash,
  branches, merge/rebase com resolução de conflitos, buscar, puxar e enviar.
- Ver **PRs, issues e execuções do Actions** do GitHub pela aba GitHub e mandar um PR, uma issue ou um job que
  falhou (com o fim do log) para um agente.
- Citar arquivos do workspace (`@arquivo`) e ditar por voz (whisper.cpp local) direto em cada aba de CLI (`Ctrl+Shift+M` ou o microfone do cabeçalho da janela, que digita no terminal da aba visível sem enviar).

**Como agente, dentro de uma aba:**

| Comando | Faz |
| :-- | :-- |
| `ask-agent <agente> "mensagem"` | pergunta a outro agente (ou instância: `claude@dev`) |
| `ask-agent --new <agente> "mensagem"` | abre outra instância do mesmo cargo |
| `ask-agent --model <modelo> <agente> "mensagem"` | escolhe o LLM da instância ao abri-la (`sonnet`, `opencode/glm-5.3-flash-free`…); combina com `--new`; erro 409 se a aba já está aberta com outro modelo |
| `ask-agent --reply <n> "resposta"` | responde à pergunta número `n` |
| `cat arquivo \| ask-agent <agente> -` | manda a mensagem pela entrada padrão (`--file <arquivo>` lê de um arquivo; preserva acentos, que o PowerShell troca por `?` no stdin) |
| `ask-agent --list` | lista os agentes disponíveis e as instâncias abertas, com o modelo: `claude@dev (sonnet)` |
| `ask-agent --team` | mostra o seu chefe e a sua equipe (como estão agora), com o modelo de cada um |
| `ask-agent --models [<agente>]` | lista, por agente instalado, os modelos como o `--model` aceita (`-free` primeiro e marcados "(grátis)"; OpenCode por provedor; Ollama como `ollama:<modelo>`), ou só os de um agente; antes de escolher `--agent`/`--model` |
| `ask-agent --boss "mensagem"` | pergunta/reporta ao seu chefe |
| `ask-agent --learn [<arquivo>...]` | mostra o que o projeto já aprendeu (`memory/aprendizados/`) sobre esses arquivos; sem argumentos, os aprendizados globais por inteiro e o índice dos demais. O agente de uma tarefa roda antes de mexer num arquivo pela primeira vez |
| `ask-agent --inbox [--all]` | mostra as respostas às suas perguntas que ainda não chegaram a você (a que demorou e não foi digitada, a que o `ask-agent` não leu); `--all` mostra as 20 últimas. Ler as pendentes as marca como entregues |
| `ask-agent --task new "título" [--agent <modelo>] [--model <m>] [--base <branch>] [--start] -\|"pedido"\|--file <arq>` | cria uma tarefa (o pedido vem pela entrada padrão, como último argumento ou de um arquivo): uma worktree + branch + agente próprios; `--start` já a inicia |
| `ask-agent --task list` / `show <id>` | lista as tarefas (id, estado, agente com modelo, título) / mostra uma, com relatório e diff |
| `ask-agent --task start <id>` / `merge <id>` / `validate <id>` / `discard <id>` | inicia o rascunho / mescla a tarefa em revisão na base (`--no-ff`) e a deixa **aguardando validação** (o agente e a worktree continuam) / conclui a tarefa aguardando validação e limpa / descarta e limpa |
| `ask-agent --task done <id> -\|"relatório"\|--file <arq>` | o agente da tarefa entrega o relatório (pela entrada padrão, como argumento ou de um arquivo) e a põe em revisão; também vale aguardando validação, se a branch tem commits novos (o usuário corrigiu algo na aba) |
| `ask-agent --task return <id> "feedback"` | devolve a tarefa em revisão ao agente com ajustes a fazer (o feedback também vem de um arquivo: `--file <arq>`; o agente também recebe os aprendizados dos arquivos que o feedback cita) |
| `ask-agent --task reject <id> "motivo"\|--file <arq>\|-` | reprova a tarefa aguardando validação: volta ao mesmo agente, na mesma worktree, dizendo que o trabalho já foi mesclado e o que falhou no teste |
| `ask-agent --signal busy\|idle` | uso interno dos hooks (sai sempre com 0) |
| `ask-agent --usage` | uso interno: a statusline do Claude, que manda o uso do plano à ponte (`POST /usage`) |
| `ask-agent --help` (`-h`) | imprime o uso resumido de todos os comandos acima |

---

## 3. Documentos e as regras de cada um

Cada documento cuida de um assunto. Mudou o assunto? Atualize o documento **no mesmo commit**.

| Arquivo | Para que serve | Regras que define / como manter |
| :-- | :-- | :-- |
| `memory/GERAL.md` | visão geral e este mapa | atualizar quando surgir um documento, uma pasta, um comando ou um limite novo |
| `memory/AGENTDECK.md` | visão e rumo do produto | quem decide a ordem do que vem a seguir é o orquestrador; atualizar quando o rumo mudar |
| `memory/aprendizados/` | a memória de longo prazo: um arquivo por armadilha, decisão ou convenção que não é óbvia no código | formato e quando criar em `aprendizados/README.md`; os `touches` precisam existir (o teste `every_learning_in_the_repository_is_valid` confere) |
| `memory/ARCHITECTURE.md` | stack, crates, protocolo da ponte, instâncias, entrega nas abas | **contrato**: rotas, códigos HTTP, variáveis de ambiente e limites da ponte. Mudou o protocolo? Atualize aqui |
| `memory/MANUTENCAO.md` | guia prático: Rust, comandos, mapa do código, receitas | mapa do código e receitas (ex.: novo agente: prompt nunca cru na linha de comando, hooks se o CLI tiver) |
| `memory/pendencias.md` | o que falta desenvolver: o bloco "Próximo" no topo e depois por área, com os adiados no fim | item concluído sai de `pendencias.md` e vai para `historico.md` com uma linha do que foi feito; limitação nova vira item; só desenvolvimento (testes manuais ficam em `testes.md`); o arquivo não guarda `[x]` |
| `memory/historico.md` | o que já foi feito, do mais recente para o mais antigo (inclui a auditoria de 15/10) | só recebe o que saiu de `pendencias.md`; o texto de cada entrada fica como foi escrito na época |
| `memory/testes.md` | roteiro de testes manuais (Windows, agentes reais), por área (as mesmas de `pendencias.md`) | marcar `[x]` o que passou; o que falhar vira item em `pendencias.md`; funcionalidade nova que só dá para validar à mão ganha um item na área dela; função removida leva os itens dela embora |
| `README.md` (raiz) | apresentação, como compilar e rodar, exemplos de `ask-agent` | fica na raiz porque o GitHub mostra na página do repositório |
| `CONTRIBUTING.md` (raiz) | como contribuir e os **princípios do projeto** | rodar `cargo fmt`, `clippy -D warnings` e `cargo test` antes do PR; privacidade; UI nunca trava; textos em pt-BR. Fica na raiz porque o GitHub o mostra ao abrir PRs |
| `AGENTS.md` (raiz) e `workspace/AGENTS.md` | instruções lidas pelos CLIs de agentes | **gerados pelo app**: o bloco entre `<!-- agentdeck:start -->` e `<!-- agentdeck:end -->` é reescrito a cada início; edite só fora dele. Ficam na raiz da pasta de trabalho porque é lá que os CLIs os procuram |
| `workspace/README.md` | o que é a pasta `workspace/` (o workspace antigo e o `AGENTS.md` do app) | isolamento: os agentes trabalham na pasta do projeto ou da tarefa, não nos arquivos do app |
| `dados/README.md` | o que vai em `dados/` (anexos, prompts, tarefas, hooks, `crash.log`) | privacidade: áudio apagado após transcrever, retenção só das últimas sessões, nada versionado |
| `whisper/README.md`, `whisper/models/README.md` | motor e modelos do ditado | ficam ao lado dos binários e modelos que descrevem |
| `LICENSE` | licença MIT | não editar |

Arquivos de regra que não são `.md`: `rustfmt.toml` (estilo do código, conferido na CI),
`.github/workflows/ci.yml` (o que precisa passar em todo push) e `Cargo.toml` (`rust-version` 1.89 é a versão
mínima do Rust).

### Regras gerais, em resumo

1. A interface (`app`) só fala com o backend pelos métodos de `Core`; o backend responde com `CoreEvent`.
   (ARCHITECTURE, MANUTENCAO)
2. Antes de concluir qualquer mudança: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`. (CONTRIBUTING)
3. A ponte só escuta em `127.0.0.1`, identifica quem chama pelo token e nunca confia no `from` do corpo.
   Texto de outro agente nunca vai cru para um terminal nem para a linha de comando. (ARCHITECTURE, CONTRIBUTING)
4. Nada que bloqueia roda na thread da interface. (CONTRIBUTING)
5. Privacidade: áudio apagado logo após a transcrição; anexos e prompts guardados só para as últimas 5 sessões;
   nunca criar `AGENTS.md` na pasta pessoal do usuário. (CONTRIBUTING, dados/README)
6. Textos para o usuário e comentários de código em português. (CONTRIBUTING)
7. Documentação junto com o código: mudou comportamento, protocolo ou pendência, atualize o `.md`
   correspondente em `memory/` no mesmo commit; item concluído de `pendencias.md` vai para `historico.md`. (este documento)
