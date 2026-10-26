# AgentDeck: visão geral

Ponto de partida da documentação, para pessoas e para agentes de IA que trabalham no projeto. Ele explica
**como o AgentDeck funciona por dentro** (a "ambientação" em que os agentes rodam), **o que dá para fazer**
com ele e **quais regras cada documento define**. Os detalhes técnicos ficam nos outros arquivos desta pasta.

| Documento | Leia quando |
| :-- | :-- |
| [AGENTDECK.md](AGENTDECK.md) | quiser saber para onde o produto vai, os princípios e a ordem do que vem a seguir |
| [GERAL.md](GERAL.md) (este) | quiser o mapa geral e o que dá para fazer no app |
| [ARCHITECTURE.md](ARCHITECTURE.md) | for mexer na ponte, nas abas, nas instâncias, nas tarefas ou no protocolo entre agentes |
| [MANUTENCAO.md](MANUTENCAO.md) | for compilar, testar, achar um arquivo ou seguir uma receita (ex.: novo agente) |
| [pendencias.md](pendencias.md) | quiser saber os próximos passos (no máximo 10, na ordem em que serão feitos) |
| [ideias.md](ideias.md) | quiser ver as ideias sem data e as limitações conhecidas (o que não está entre os próximos passos) |
| [historico.md](historico.md) | quiser saber o que já foi feito (o destino de todo item concluído de pendencias.md) |
| [testes.md](testes.md) | for testar à mão, no Windows, com os agentes reais (roteiro com caixas de marcar) |
| [LINUX.md](LINUX.md) | for compilar, rodar ou testar no Linux (Ubuntu e Fedora) e quiser saber o que já foi verificado e o que falta |

---

## 1. Ambientação: como o AgentDeck funciona

### O app

Terminal desktop nativo em Rust (winit + egui). São dois executáveis, que precisam ficar **na mesma pasta**:

- `agentdeck` (crate `app`): a janela, as abas de terminal e a barra lateral direita (mesa de tarefas, Conversa
  integrada, Git e GitHub);
- `ask-agent` (crate `cli`): o comando que os agentes usam para conversar entre si e operar tarefas.

O "backend" é a crate `core`, sem interface. A interface só fala com ele pelos métodos de `Core` e recebe de
volta eventos `CoreEvent`.

### Pastas

| Pasta | Conteúdo |
| :-- | :-- |
| `workspace/` | o workspace antigo, do tempo em que havia uma pasta global. Hoje cada **projeto** tem a sua pasta (o `cwd` das abas dele); `workspace/` só serve de destino na migração de uma config antiga |
| `dados/` | dados de execução: anexos (`anexos/`), prompts (`prompts/`), hooks (`hooks/`), instruções (`agentdeck-instructions.md`, `agy/<instância>/AGENTS.md`), tarefas (`tarefas/<projeto>.json` e `*-brief.md`/`-feedback.md`), sessões, `usage.json`, `crash.log`, `agentdeck.log` (gira acima de 2 MB), trava `.agentdeck.lock` e `.agentdeck.bridge` |
| `whisper/` | motor e modelos do ditado por voz (whisper.cpp) |
| `%APPDATA%\AgentDeck\config.json` | configuração (na primeira abertura, copiada de `%APPDATA%\CommandDeck\config.json`, se existir). Com `AGENTDECK_HOME`, tudo fica dentro dessa pasta |

**Uma janela por pasta de dados:** abrir o app de novo com o mesmo `dados/` traz a instância viva de volta (`POST /activate`)
e sai em silêncio; o × da janela esconde na bandeja (ou minimiza sem bandeja) e quem encerra é o menu do ícone. Para outra
instância isolada, use `AGENTDECK_HOME`.

### Abas são instâncias

Cada aba é uma **instância** com nome `modelo[@cargo[@nome]]`:

- `claude`: a instância principal do modelo;
- `claude@dev`: uma instância com cargo;
- `claude@dev@ana`: a segunda instância do mesmo modelo e cargo, que ganha um nome.

Além do nome, cada instância tem o **modelo de LLM** (`sonnet`, `opus`…), que não faz parte do id: aparece ao lado
do nome na aba e na barra lateral, e escolhe-se por `ask-agent --model`, pelo menu "+ Novo agente" (a seta `›` do
Claude, do Antigravity e do OpenCode lista os modelos; no OpenCode, por provedor) ou pelo padrão `roleModels` da
configuração (`{"claude@dev": "sonnet"}`; `modelo@cargo` vale mais que `modelo`).

O nome é o endereço usado pelo `ask-agent`. As abas voltam com o mesmo nome e modelo quando o app reabre.

### O ambiente de cada aba

| Variável | O que é |
| :-- | :-- |
| `AGENTDECK_AGENT` | o nome da instância (`claude@dev`) |
| `AGENTDECK_ROLE` | o cargo, quando há |
| `AGENTDECK_PORT` | porta da ponte HTTP local (só em `127.0.0.1`) |
| `AGENTDECK_TOKEN` | token **próprio da aba**: é por ele que a ponte sabe quem está perguntando e de qual projeto |
| `AGENTDECK_DEPTH` | profundidade da conversa (usada no modo headless) |
| `AGENTDECK_ASK_WAIT` | quantos segundos o `ask-agent` espera a resposta (padrão 90; `0` = até o fim). O usuário pode definir |
| `PATH` | a pasta dos executáveis vem primeiro, então o `ask-agent` sempre é encontrado |

Nenhuma outra variável `AGENTDECK_*` do ambiente do app passa para os agentes. As instruções do AgentDeck (com a regra
de não usar subagentes internos do CLI) chegam pelo canal próprio de cada CLI, sem depender de um `AGENTS.md` do usuário
(só Goose e Copilot criam o `AGENTS.md` se a pasta não tiver um), e a confiança da pasta é registrada antes de abrir a
aba. Detalhes em [ARCHITECTURE.md](ARCHITECTURE.md).

### O caminho de uma pergunta entre agentes

1. Um agente roda `ask-agent claude@dev "tarefa"`.
2. A ponte resolve o destino no projeto de quem chamou. Se a instância não existe, a aba dela é aberta em segundo plano.
3. A mensagem é **digitada no terminal da aba de destino** quando ela fica livre (nunca logo após tecla do usuário;
   com hooks no Claude e no Codex, após o fim de turno; sem hooks, após 1,5 s parado ou 30 s na fila).
4. O agente de destino responde com `ask-agent --reply <n> "resposta"`. Só ele consegue responder àquela pergunta.
5. A resposta volta para quem perguntou: em até 90 s sai no stdout do `ask-agent`; passando disso, o `ask-agent` termina
   avisando (HTTP 202) e a resposta é **digitada depois** no terminal de quem perguntou (e fica em `ask-agent --inbox`).
   Numa aba `shell` ou no modo headless, o `ask-agent` espera até o fim.

Tudo aparece na **Conversa integrada**, e o usuário pode intervir em qualquer aba.

### A janela

| Área | O que tem |
| :-- | :-- |
| Barra de título | própria (sem a barra do sistema): ☰ (configurações), recolher barra esquerda, marca e "AgentDeck", `/ <projeto>` e chip da branch (com a barra lateral recolhida, a **chave Agentes \| Documentos** compacta aparece logo depois); à direita, o **uso de cada agente com cota** (logo + barras de sessão de 5 h e semanal; âmbar a partir de 80 %, vermelho em 100 %; passar o mouse abre o **painel de uso** com grupos e horários de renovação) e minimizar/maximizar/fechar |
| Barra esquerda | a **chave Agentes \| Documentos** no topo, **PROJETOS** (cada projeto com os seus agentes em árvore e o modelo à direita; o ativo tem "+ Novo agente"; "+" no cabeçalho abre uma pasta como projeto; clique direito renomeia, abre a pasta ou remove), **ORGANIZAÇÃO** (`Abas \| Dividida \| Destaque`) e **ARQUIVOS**, o explorador (com a letra do Git, atualizar e recolher tudo); recolhe com `Ctrl+B` |
| Centro | duas visões (`Ctrl+Shift+D`): **Agentes** (terminais organizados em Abas, Dividida ou Destaque; no cabeçalho de cada janela ficam as abas, a alça `⋮⋮`, o microfone, o anexo e o "+") e **Documentos** (diff, arquivos editáveis e markdown ocupando todo o centro) |
| Barra direita | **fixa, não recolhe**; largura ajustável pela borda. Quatro abas no topo: **Tarefas** (a primeira; o selo conta revisão + validação), **Conversa** (só leitura: o feed das mensagens entre agentes, com contador de não lidas), **Git** e **GitHub** (com o número de PRs abertos) |

- **Projetos:** cada projeto é uma pasta com os seus agentes. Trocar de projeto esconde as abas do anterior, que
  **continuam rodando**. A árvore, Tarefas, Git, GitHub e Conversa são do projeto ativo; o uso na barra de título é global.
  Remover e readicionar a mesma pasta reaproveita o id do projeto e recupera suas tarefas.
- **Organização (`Abas | Dividida | Destaque`):** em **Dividida**, abas do mesmo tipo de CLI dividem a mesma janela numa
  grade automática (2 lado a lado, 2+1, 2×2, 3+2, 3×2), com divisores arrastáveis e alça `⋮⋮` para trocar janelas de
  lugar; em **Destaque**, a janela do tipo da aba ativa ocupa 2/3 à esquerda e as outras empilham à direita; em **Abas**,
  todas ficam numa janela só. A janela ativa ganha borda e halo na cor do agente e as outras esmaecem; faixas de abas que
  não cabem mostram setas `‹`/`›` e menu `▾`. Atalhos: `Ctrl+Tab` / `Ctrl+Shift+Tab` na janela, `Ctrl+Alt+setas` entre
  janelas, `Ctrl+T` nova aba.
- **Documentos (`Ctrl+Shift+D`):** abrir um diff, arquivo (duplo clique no explorador; clique simples insere `@arquivo` no
  terminal) ou relatório traz **Documentos** ao centro; fechar o último ou clicar num agente volta para **Agentes**. O selo
  de "Agentes" conta turnos concluídos não vistos (ou pulsa se houver agente trabalhando) e os terminais escondidos não são
  redimensionados. Arquivos de texto até 256 KB têm **realce de sintaxe** (9 linguagens), busca (`Ctrl+F`) e **edição**
  (`Ctrl+S` preservando CRLF/LF e BOM, com confirmação ao fechar sem salvar e aviso se o disco mudar por fora); até 1 MB
  abrem só para leitura. Diffs trazem lista de arquivos à esquerda, realce, busca e cópia só do código; `.md`, Pedido e
  Relatório abrem formatados.
- **Aba Tarefas:** cada tarefa roda num agente com branch e worktree próprias (`.agentdeck/worktrees/tN`). Ciclo:
  *Em andamento* → (`--task done`) *Em revisão* → (mesclar) *Aguardando validação* (aba, worktree e branch seguem vivas
  para testar o app) → (validar) *Concluída*; *Reprovar* devolve ao mesmo agente. O formulário **+ Nova** recebe título,
  pedido, **nível** (*Simples*, *Média*, *Complexa*), agente/modelo (recomendados por capacidade + folga de cota), esforço,
  base, **escopo** (se coincidir com outra tarefa ativa, entra **na fila** e começa sozinha quando liberar) e anexos. O
  detalhe mostra pedido, relatório, diff (**Ver diff completo** / **Ver o que foi mesclado**), seção **MENSAGENS** e
  histórico, e oferece botões para resolver recusas de mesclagem ali mesmo (guardar alterações em stash e mesclar, pedir
  commit ou resolução de conflito ao agente).
- **Aba Git e Aba GitHub:** a aba **Git** opera o repositório local (Alterações com stage/discard por arquivo ou por
  trecho/hunk, commit, Histórico paginado, Stash, branches com merge/rebase e resolução de Conflitos), atualizando sozinha
  quando `.git/` muda. A aba **GitHub** lista PRs (com checks), Issues e Actions (com o fim do log de jobs falhos) do
  remoto `origin`, usando o login do Git Credential Manager ou do `gh`, e permite **Enviar para agente**.

### Limites, proteções e modo headless

| Regra | Valor |
| :-- | :-- |
| Profundidade máxima de delegação (A → B → C; a terceira pergunta encadeada é recusada) | 2 |
| Ciclo de espera (A espera B, que espera A) | recusado na hora (HTTP 409) |
| Abas abertas automaticamente pelo `ask-agent` | 8 |
| Tempo máximo de uma pergunta | 10 min |
| Tamanho de uma mensagem / requisições simultâneas / orçamento total de corpos na ponte | 16 MB / 128 / 64 MB |
| Texto de outro agente digitado num terminal | sem ESC nem caracteres de controle |
| Prompt para um shim `.cmd` no Windows | vai por arquivo, nunca na linha de comando |
| Arquivo aberto no modo Documentos | até 1 MB (editável até 256 KB / linhas de 4 000 bytes), sem bytes nulos |

Quando "Abrir abas dos agentes" está desligado nas Configurações, o `ask-agent` roda os agentes em **modo headless**
(`claude -p`, `agy -p`, `codex exec`, `opencode run`…) com token temporário (tarefas exigem abas ligadas). O contrato HTTP
completo da ponte está em [ARCHITECTURE.md](ARCHITECTURE.md).

---

## 2. O que dá para fazer

**Como usuário:**

- Abrir abas de agentes (Claude Code, Antigravity, Codex, OpenCode, Aider, Goose, Copilot, Amazon Q, Open Interpreter) e
  de terminal comum, escolhendo o modelo de LLM por instância.
- Organizar o trabalho em **projetos** (uma pasta cada), ver os agentes de todos os projetos **em árvore** na barra lateral
  e reorganizar a **equipe** (chefe e subordinados) arrastando um agente sobre outro.
- Ver vários agentes ao mesmo tempo na **tela dividida** ou em **destaque** (`Abas | Dividida | Destaque`).
- Acompanhar o **uso** de cada agente com cota (sessão e semana) na barra de título e no painel de uso.
- Falar com um agente pelo terminal da aba dele, citar arquivos (`@arquivo`), anexar arquivos/imagens (botão, arrastar e
  soltar ou `Ctrl+V`) e usar **ditado por voz** (`Ctrl+Shift+M` ou ícone de microfone, via whisper.cpp local).
- Acompanhar as conversas entre agentes na Conversa integrada (só leitura) e intervir direto nos terminais.
- Ler e editar arquivos do projeto com realce de sintaxe e busca, ou ler diffs e relatórios em tela cheia no modo
  **Documentos** (`Ctrl+Shift+D`).
- Criar e gerir **tarefas** na aba Tarefas (com nível, modelo, esforço, escopo com fila automática, revisão de diff,
  mesclagem assistida e etapa de validação).
- Operar o **Git** local (stage por arquivo ou trecho, commit, branches, merge/rebase, conflitos, stash) e consultar
  **GitHub** (PRs, issues, Actions e envio de falhas de CI para um agente).
- Fechar a janela no × para continuar rodando os agentes na **bandeja do sistema**, ou encerrar pelo menu da bandeja.

**Como agente, dentro de uma aba:**

| Comando | Faz |
| :-- | :-- |
| `ask-agent <agente> "mensagem"` | pergunta a outro agente ou instância (`claude@dev`), abrindo a aba se não existir |
| `ask-agent --new <agente> "mensagem"` | abre outra instância do mesmo cargo (`claude@dev@ana`) |
| `ask-agent --model <modelo> <agente> "mensagem"` | escolhe o LLM da instância ao abri-la (`sonnet`, `opencode/glm-5.3-flash-free`…); combina com `--new`; erro 409 se a aba já está aberta com outro modelo |
| `ask-agent --reply <n> "resposta"` | responde à pergunta número `n` |
| `ask-agent --file <arquivo> …` | lê a mensagem, o pedido, o relatório ou o feedback de um arquivo (preserva acentos, que o PowerShell 5.1 troca por `?` no stdin `-`) |
| `ask-agent --list` | lista os agentes disponíveis e as instâncias abertas no projeto, com modelo e aviso de cota |
| `ask-agent --team` | mostra o seu chefe e a sua equipe atuais, com modelo e aviso de cota |
| `ask-agent --models [<agente>]` | lista, por agente instalado (os quatro oficiais em ordem de mais folga de cota primeiro), os modelos aceitos em `--model` (`-free` primeiro e marcados "(grátis)"; OpenCode por provedor), a linha `cota:` de cada grupo e a tabela de **capacidade** (`simples`, `media`, `complexa`). A ponte recusa (409) pergunta a modelo esgotado |
| `ask-agent --boss "mensagem"` | pergunta ou reporta ao seu chefe |
| `ask-agent --learn [<arquivo>...]` | sem argumentos, mostra os aprendizados globais (`memory/aprendizados/`), o índice dos demais e a **Situação atual** (tarefas em andamento e últimos commits); com arquivos, mostra os aprendizados que casam com eles e avisa no topo se outra tarefa em andamento mexe neles |
| `ask-agent --outline <arquivo>...` | mostra o esqueleto de cada arquivo (assinaturas e número da linha, sem os corpos, em Rust e Markdown) para ler só o trecho necessário com offset |
| `ask-agent --inbox [--all]` | mostra as respostas às suas perguntas que ainda não foram entregues (ou as 20 últimas com `--all`), marcando-as como entregues |
| `ask-agent --task new "título" [--agent <m>] [--model <m>] [--effort low\|medium\|high] [--level simples\|media\|complexa] [--base <branch>] [--start] -\|"pedido"\|--file <arq> [--scope <caminho>…]` | cria uma tarefa (worktree + branch + agente próprios; sem `--agent`, escolhe entre os modelos capazes do nível o de mais folga de cota); `--start` já a inicia (ou põe na fila se `--scope` sobrepuser outra em andamento/revisão) |
| `ask-agent --task list` / `show <id>` | lista as tarefas do projeto / mostra o detalhe completo de uma tarefa |
| `ask-agent --task start <id>` / `merge <id>` / `validate <id>` / `discard <id>` | inicia rascunho / mescla tarefa em revisão na base (`--no-ff`, indo a aguardando validação) / conclui validação e limpa / descarta e limpa |
| `ask-agent --task done <id> -\|"relatório"\|--file <arq>` | o agente da tarefa entrega o relatório e a põe em revisão (vale também em validação se houver commits novos) |
| `ask-agent --task return <id> "feedback"\|--file <arq>` | devolve a tarefa em revisão ao agente com ajustes (incluindo aprendizados dos arquivos citados no feedback) |
| `ask-agent --task reject <id> "motivo"\|--file <arq>\|-` | reprova a tarefa aguardando validação e a devolve ao mesmo agente na mesma worktree |
| `ask-agent --signal busy\|idle` | uso interno dos hooks de início/fim de turno (sai sempre com 0) |
| `ask-agent --usage` | uso interno da statusline do Claude (envia o uso do plano à ponte) |
| `ask-agent --help` (`-h`) | imprime o resumo de todos os comandos acima |

---

## 3. Documentos e as regras de cada um

Cada documento cuida de um assunto. Mudou o assunto? Atualize o documento **no mesmo commit**.

| Arquivo | Para que serve | Regras que define / como manter |
| :-- | :-- | :-- |
| `memory/GERAL.md` | visão geral e este mapa | atualizar quando surgir um documento, uma pasta, um comando ou um limite novo |
| `memory/AGENTDECK.md` | visão e rumo do produto | quem decide a ordem do que vem a seguir é o orquestrador; atualizar quando o rumo mudar |
| `memory/aprendizados/` | a memória de longo prazo: um arquivo por armadilha, decisão ou convenção que não é óbvia no código | formato e quando criar em `aprendizados/README.md`; os `touches` precisam existir (o teste `every_learning_in_the_repository_is_valid` confere) |
| `memory/ARCHITECTURE.md` | stack, crates, protocolo da ponte, instâncias, entrega nas abas, tarefas e memória | **contrato**: rotas, códigos HTTP, variáveis de ambiente e limites da ponte. Mudou o protocolo? Atualize aqui |
| `memory/MANUTENCAO.md` | guia prático: Rust, comandos, mapa do código, receitas | mapa do código e receitas (ex.: novo agente: prompt nunca cru na linha de comando, hooks se o CLI tiver) |
| `memory/pendencias.md` | os próximos passos, numerados, no máximo 10 ("Agora" e "Depois") | item concluído sai de `pendencias.md` e vai para `historico.md` com uma linha do que foi feito; um item novo só entra se couber nos 10 (senão vai para `ideias.md`); só desenvolvimento (testes manuais ficam em `testes.md`); o arquivo não guarda `[x]` |
| `memory/ideias.md` | ideias sem data, por área, e as limitações conhecidas | ideia que entra em `pendencias.md` sai daqui; limitação nova entra aqui; não guarda item concluído |
| `memory/historico.md` | o que já foi feito, do mais recente para o mais antigo (inclui a auditoria de 15/10) | só recebe o que saiu de `pendencias.md`; uma linha por item; o detalhe fica no commit |
| `memory/testes.md` | roteiro de testes manuais (Windows, agentes reais), por área (as mesmas de `pendencias.md`) | marcar `[x]` o que passou; o que falhar vira item em `pendencias.md`; funcionalidade nova que só dá para validar à mão ganha um item na área dela; função removida leva os itens dela embora |
| `memory/LINUX.md` | dependências, o que foi verificado e o roteiro manual do Linux (Ubuntu e Fedora) | atualizar no mesmo commit em que mudar dependência de sistema, caminho de fonte/pasta de dados ou suporte a GPU; falha de Linux vira item em `pendencias.md` |
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
6. Code comments, docs and commit messages in English; user-facing text through `t!` with a pt-BR entry; agent-facing
   text in English only. (CONTRIBUTING, aprendizados/i18n-english-key-and-area-files.md)
7. Documentação junto com o código: mudou comportamento, protocolo ou pendência, atualize o `.md`
   correspondente em `memory/` no mesmo commit; item concluído de `pendencias.md` vai para `historico.md`. (este documento)
