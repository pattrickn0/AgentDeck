# CommandDeck: visão geral

Ponto de partida da documentação, para pessoas e para agentes de IA que trabalham no projeto. Ele explica
**como o CommandDeck funciona por dentro** (a "ambientação" em que os agentes rodam), **o que dá para fazer**
com ele e **quais regras cada documento define**. Os detalhes ficam nos outros arquivos desta pasta.

| Documento | Leia quando |
| :-- | :-- |
| [GERAL.md](GERAL.md) (este) | quiser o mapa geral |
| [ARCHITECTURE.md](ARCHITECTURE.md) | for mexer na ponte, nas abas, nas instâncias ou no protocolo entre agentes |
| [MANUTENCAO.md](MANUTENCAO.md) | for compilar, testar, achar um arquivo ou seguir uma receita (ex.: novo agente) |
| [pendencias.md](pendencias.md) | quiser saber o que falta desenvolver, o que está adiado e o que já foi feito |
| [testes.md](testes.md) | for testar à mão, no Windows, com os agentes reais (roteiro com caixas de marcar) |

---

## 1. Ambientação: como o CommandDeck funciona

### O app

Terminal desktop nativo em Rust (winit + egui). São três executáveis, que precisam ficar **na mesma pasta**:

- `commanddeck` (crate `app`): a janela, as abas de terminal e a barra lateral direita (Conversa integrada,
  Mapa de agentes e compositor);
- `ask-agent` (crate `cli`): o comando que os agentes usam para conversar entre si;
- `ollama-agent` (crate `cli`): o agente local com ferramentas, que roda numa aba.

O "backend" é a crate `core`, sem interface. A interface só fala com ele pelos métodos de `Core` e recebe de
volta eventos `CoreEvent`.

### Pastas

| Pasta | Conteúdo |
| :-- | :-- |
| `workspace/` | pasta de trabalho padrão dos agentes (o `cwd` de cada aba). Dá para trocar nas Configurações |
| `dados/` | dados de execução: anexos, prompts longos (`prompts/`), hooks do Claude (`hooks/`), sessões, `crash.log` e a trava `.commanddeck.lock` |
| `whisper/` | motor e modelos do ditado por voz (whisper.cpp) |
| `%APPDATA%\CommandDeck\config.json` | configuração. Com `COMMANDDECK_HOME`, tudo isso fica dentro dessa pasta |

**Uma janela por pasta de dados:** uma segunda janela usando o mesmo `dados/` é recusada. Para abrir outra
instância isolada, use `COMMANDDECK_HOME`.

### Abas são instâncias

Cada aba é uma **instância** com nome `modelo[@cargo[@nome]]`:

- `claude`: a instância principal do modelo;
- `claude@dev`: uma instância com cargo;
- `claude@dev@ana`: a segunda instância do mesmo modelo e cargo, que ganha um nome.

O nome é o endereço usado pelo `ask-agent`. As abas voltam com o mesmo nome quando o app reabre.

### O ambiente de cada aba

| Variável | O que é |
| :-- | :-- |
| `COMMANDDECK_AGENT` | o nome da instância (`claude@dev`) |
| `COMMANDDECK_ROLE` | o cargo, quando há |
| `COMMANDDECK_PORT` | porta da ponte HTTP local (só em `127.0.0.1`) |
| `COMMANDDECK_TOKEN` | token **próprio da aba**: é por ele que a ponte sabe quem está perguntando |
| `COMMANDDECK_DEPTH` | profundidade da conversa (usada no modo headless) |
| `COMMANDDECK_ASK_WAIT` | quantos segundos o `ask-agent` espera a resposta (padrão 90; `0` = até o fim). O usuário pode definir |
| `PATH` | a pasta dos executáveis vem primeiro, então o `ask-agent` sempre é encontrado |

Nenhuma outra variável `COMMANDDECK_*` do ambiente do app passa para os agentes.

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
| Barra de título | própria (a janela não tem a do sistema): ☰ (configurações), recolher a barra esquerda (ícone de painel), "CommandDeck", "+" (nova aba); à direita, o **uso de cada agente aberto** (logo + círculo que enche com a sessão + anel que fecha com a semana e fica vermelho quando ela acaba) e minimizar/maximizar/fechar. Arrastar move a janela, duplo clique maximiza, as bordas redimensionam |
| Barra esquerda | **PROJETOS** no topo (cada projeto com os seus agentes; o ativo lista as abas dele; "+" abre uma pasta como projeto; clique direito renomeia, abre a pasta ou remove), a seção **ORGANIZAÇÃO** (chave da tela dividida) e o explorador (atualizar, recolher tudo); recolhe com Ctrl+B |
| Centro | **tela dividida por tipo de agente**: cada tipo (Claude, Antigravity, Ollama, Codex, Terminal…) tem uma janela com as próprias abas; ver abaixo |
| Barra direita | **fixa, não recolhe**; a largura muda arrastando a borda. No topo, três abas com ícone: **Conversa** (balões; com o número de mensagens novas enquanto você está em outra aba), **Mapa** (diagrama) e **Git** (branch). Embaixo, o compositor ("Enviar para", anexos, mensagem, microfone/anexo/Enviar), que some na aba Git |

**Projetos.** O projeto é a unidade de trabalho: uma pasta com os seus agentes. Cada aba pertence ao projeto em
que foi aberta (o terminal roda na pasta dele) e só aparece com ele na tela; trocar de projeto esconde as abas do
anterior, que **continuam rodando**. A árvore, a aba Git, a Conversa e o Mapa são do projeto ativo; o uso na barra
de título é global (o limite é da conta). Ao abrir o app, voltam só as abas do projeto ativo; as dos outros abrem
na primeira vez que o projeto é selecionado (ou quando um agente chama alguém de lá). Não existe mais workspace
padrão: sem projeto, o centro pede uma pasta.

A **tela dividida** organiza as abas por tipo de agente:
- abas do mesmo tipo ficam na mesma janela (2 Claude = 1 janela com 2 abas; todos os modelos do Ollama juntos;
  `claude@dev` fica na janela do Claude); um tipo novo abre mais uma janela;
- as janelas ficam numa grade automática: 2 lado a lado; 3 = 2 em cima + 1 largo embaixo; 4 = 2×2; 5 = 3+2;
  6 = 3×2. Se não cabem no tamanho mínimo (~260 × 140 px), os tipos que sobram viram abas da última janela;
- cada janela tem o cabeçalho com as abas dela, a alça ⋮⋮ (arrastar sobre outra janela troca as duas de lugar) e
  um "+" que abre outra aba do mesmo tipo. A janela focada tem o contorno na cor do agente e recebe o teclado,
  o IME e o "Enviar para → Aba ativa";
- os divisores entre janelas se arrastam (duplo clique iguala); tamanhos, ordem e o liga/desliga ficam salvos.
  Desligada (botão na barra de título), volta a ser uma janela só com todas as abas;
- uma aba aberta por outro agente (`ask-agent codex …`) entra na janela do tipo dela (ou cria a janela) sem tirar
  o foco de onde você está digitando;
- atalhos: Ctrl+Tab / Ctrl+Shift+Tab trocam de aba dentro da janela focada; Ctrl+Alt+setas mudam o foco para a
  janela vizinha; Ctrl+T abre outra aba do tipo focado.

O **Mapa** mostra, em texto de terminal, quem está conversando com quem agora: a árvore de quem pergunta a quem
(pergunta na fila, digitada, com resposta atrasada ou em modo headless, com o tempo de espera), as respostas a
caminho, as conversas concluídas no último minuto (esmaecidas) e os agentes sem conversa.
O Mapa rola dentro da barra, que mantém a largura. Clicar num agente abre a aba dele.

A aba **Git** trabalha no repositório da pasta de trabalho, com o git instalado na máquina:
- topo: branch atual e menu de branches (trocar, criar, **mesclar** ou fazer **rebase** da branch escolhida); buscar, puxar
  (só avanço direto) e enviar ("Publicar" quando a branch ainda não existe no remoto);
- merge, rebase, cherry-pick ou revert em andamento aparecem numa faixa no topo com **Continuar** (só sem conflitos) e **Abortar**;
- quatro visões: **Alterações**, **Histórico**, **Stash** e, só quando há conflito, **Conflitos**;
- Alterações: seções (conflitos, preparadas, alterações, não rastreados) com preparar (+), tirar (−) e descartar (↶, com
  confirmação); clique abre o diff (rola dentro da barra), que tem **Preparar / Tirar / Descartar por trecho** (hunk);
  duplo clique abre o arquivo; caixa de commit (Ctrl+Enter) com a opção de corrigir o último commit;
- Histórico: commits da branch (hash, autor, data, assunto; "Carregar mais") e, ao clicar, o detalhe do commit (mensagem,
  arquivos e diff);
- Stash: guardar (com mensagem; opção de incluir não rastreados), ver o diff, aplicar, aplicar e apagar e apagar;
- Conflitos: cada arquivo mostra os blocos com "o nosso", "o deles" ou "ambos" por bloco ou no arquivo inteiro, e "Marcar como resolvido";
- commit com `commit.gpgsign` e chave travada falha na hora, explicando como destravar (o app não desliga a assinatura);
  push/pull por ssh com chave com senha explicam o ssh-agent;
- sem repositório: botão "Inicializar repositório"; sem git: o link para instalar.

Atualiza sozinha quando o `.git/` muda (commit ou checkout feitos pelo terminal ou por um agente), quando a
pasta muda e quando a janela volta a ter foco. Nada de agentes por enquanto: tudo é manual.

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
- Acompanhar o **uso** de cada agente aberto (sessão e semana) na barra de título.
- Mandar uma mensagem (com anexos ou áudio transcrito) para a aba ativa pelo compositor.
- Acompanhar e intervir nas conversas entre agentes pela Conversa integrada e pelas próprias abas.
- Ver no **Mapa** quem está conversando com quem neste momento.
- Usar o **git** do projeto pela aba Git: ver alterações e diffs, preparar, fazer commit, trocar/criar branch,
  buscar, puxar e enviar.
- Citar arquivos do workspace (`@arquivo`), anexar arquivos e imagens, ditar por voz (whisper.cpp local).

**Como agente, dentro de uma aba:**

| Comando | Faz |
| :-- | :-- |
| `ask-agent <agente> "mensagem"` | pergunta a outro agente (ou instância: `claude@dev`) |
| `ask-agent --new <agente> "mensagem"` | abre outra instância do mesmo cargo |
| `ask-agent --reply <n> "resposta"` | responde à pergunta número `n` |
| `cat arquivo \| ask-agent <agente> -` | manda a mensagem pela entrada padrão |
| `ask-agent --list` | lista os agentes disponíveis |
| `ask-agent --team` | mostra o seu chefe e a sua equipe (como estão agora) |
| `ask-agent --boss "mensagem"` | pergunta/reporta ao seu chefe |
| `ask-agent --signal busy\|idle` | uso interno dos hooks (sai sempre com 0) |

---

## 3. Documentos e as regras de cada um

Cada documento cuida de um assunto. Mudou o assunto? Atualize o documento **no mesmo commit**.

| Arquivo | Para que serve | Regras que define / como manter |
| :-- | :-- | :-- |
| `memory/GERAL.md` | visão geral e este mapa | atualizar quando surgir um documento, uma pasta, um comando ou um limite novo |
| `memory/ARCHITECTURE.md` | stack, crates, protocolo da ponte, instâncias, entrega nas abas | **contrato**: rotas, códigos HTTP, variáveis de ambiente e limites da ponte. Mudou o protocolo? Atualize aqui |
| `memory/MANUTENCAO.md` | guia prático: Rust, comandos, mapa do código, receitas | mapa do código e receitas (ex.: novo agente: prompt nunca cru na linha de comando, hooks se o CLI tiver) |
| `memory/pendencias.md` | trabalho de desenvolvimento por prioridade e o que já foi concluído | item concluído vai para "Concluído" com uma linha do que foi feito; limitação nova vira item; só desenvolvimento (testes manuais ficam em `testes.md`) |
| `memory/testes.md` | roteiro de testes manuais (Windows, agentes reais) | marcar `[x]` o que passou; o que falhar vira item em `pendencias.md`; funcionalidade nova que só dá para validar à mão ganha um item aqui |
| `README.md` (raiz) | apresentação, como compilar e rodar, exemplos de `ask-agent` | fica na raiz porque o GitHub mostra na página do repositório |
| `CONTRIBUTING.md` (raiz) | como contribuir e os **princípios do projeto** | rodar `cargo fmt`, `clippy -D warnings` e `cargo test` antes do PR; privacidade; UI nunca trava; textos em pt-BR. Fica na raiz porque o GitHub o mostra ao abrir PRs |
| `AGENTS.md` (raiz) e `workspace/AGENTS.md` | instruções lidas pelos CLIs de agentes | **gerados pelo app**: o bloco entre `<!-- commanddeck:start -->` e `<!-- commanddeck:end -->` é reescrito a cada início; edite só fora dele. Ficam na raiz da pasta de trabalho porque é lá que os CLIs os procuram |
| `workspace/README.md` | o que é a pasta de trabalho dos agentes | isolamento: os agentes trabalham ali, não nos arquivos do app |
| `dados/README.md` | o que vai em `dados/` | privacidade: áudio apagado após transcrever, retenção só das últimas sessões, nada versionado |
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
   correspondente em `memory/` no mesmo commit. (este documento)
