# Roteiro de testes manuais

Testes para fazer **à mão, no Windows, com os agentes reais**. Os testes automáticos (`cargo test`) e as capturas de
tela no Linux já passaram; o que está aqui não dá para verificar sem a sua máquina: ConPTY, Git for Windows,
Credential Manager, fontes do Windows, os CLIs instalados e o comportamento real dos agentes.

O trabalho de desenvolvimento que falta fica em [pendencias.md](pendencias.md); visão geral em [GERAL.md](GERAL.md).

**Como usar**
- Marque `[x]` no que passou. No que falhar, anote embaixo do item: o que fez, o que esperava, o que aconteceu,
  um print e, se o app fechou, o fim de `dados/crash.log`.
- Os números entre parênteses (ex.: 202, 409) são só referência para quem for corrigir.
- Itens com ★ são os mais importantes (mudanças grandes que ainda não rodaram no Windows).

**Novidades desta rodada** (comece por elas): as novidades do **7. Git**, o **7b. GitHub** e a **16. Auditoria de
2026-10-15**. Depois, **15. Agentes em árvore e equipe** e o que falta em **13. Barra de título** e **14. Projetos**.

---

## 0. Preparação

- [x] Compilar e abrir: `cargo build --workspace --release` e `.\target\release\commanddeck.exe` (os três `.exe` na mesma pasta).
- [x] Ter instalados, se possível: Claude Code, Antigravity, Git for Windows. Opcionais: Codex (`npm i -g @openai/codex`), OpenCode, Ollama.
- [x] (Opcional) Compilar e abrir no **Linux** seguindo a seção "Compilar e rodar (Linux)" do README; anotar a distribuição e se é X11 ou Wayland.
- [x] Primeira abertura (sem `config.json`) e reabrir depois de fechar **todas** as abas: abre **sem abas**; nenhum agente abre sozinho.
- [x] Abrir um projeto seu que seja repositório git ("+" da seção PROJETOS na barra lateral).

## 1. Barra direita ★

- [x] A Conversa integrada e o compositor aparecem à **direita**, ocupando a altura toda; as janelas dos terminais ocupam a altura toda no centro.
- [x] Arrastar a borda esquerda da barra muda a largura (entre ~280 px e ~metade da janela; o padrão agora é 330 px, um pouco mais estreito); **não existe** como recolher.
- [x] Fechar e reabrir o app: a largura escolhida volta igual.
- [x] Diminuir a janela até o mínimo (~820 px de largura): nada se sobrepõe; o terminal continua usável.
- [x] Compositor: **sem** o seletor "Enviar para"; anexos (se houver) em cima, caixa de texto na largura toda, microfone/anexo à esquerda e **Enviar** à direita. A mensagem vai **só para a aba ativa** (sem aba ativa viva: aviso na Conversa).
- [x] Menu "+" (nova aba) perto da borda direita da janela: abre dentro da tela e a escolha funciona.
- [x] Download do Whisper (se aparecer): a faixa de progresso fica logo abaixo do topo da barra.

## 2. Ícones do topo da barra ★

- [x] Ícones: **balões** (Conversa) e **branch** (Git). Passar o mouse mostra o nome de cada um.
- [x] Os ícones ficam nítidos (sem serrilhado estranho) e o ícone de branch mostra os **círculos vazados**.
- [X] Ficar no Git enquanto chegam mensagens: aparece um **número** sobre o ícone da Conversa; voltar à Conversa zera.
- [X] "Limpar" só aparece na Conversa.

## 3. Tela dividida ★


- [x] "+" no cabeçalho do Claude: a 2ª aba do Claude entra **na mesma janela** (não divide de novo).
- [x] Abrir um Ollama (ou Codex, ou Terminal) pelo "+" da topbar: a tela vira **2 em cima + 1 largo embaixo**. Com o 4º tipo, 2×2.
- [x] Dois modelos do Ollama ficam na **mesma janela**, um em cada aba. `claude@dev` fica na janela do Claude.
- [x] Clicar numa janela: ela ganha o **contorno na cor do agente** e o que você digita vai para ela; o compositor manda para ela.
- [x ] A rolagem do mouse funciona na janela sob o ponteiro, mesmo sem foco.
- [x] Ctrl+Tab troca de aba **só dentro** da janela focada; Ctrl+Alt+← → ↑ ↓ passa o foco para a janela vizinha (anotar se o driver de vídeo girar a tela com esse atalho).
- [x] Arrastar o divisor entre janelas muda o tamanho; duplo clique iguala; fechar e reabrir o app mantém.
- [x] Arrastar a alça **⋮⋮** de uma janela sobre outra: aparece "Soltar para trocar de lugar" e, ao soltar, as duas trocam. A ordem volta igual ao reabrir.
- [x] Fechar a última aba de um tipo: a janela some e as outras ocupam o espaço. Fechar uma aba com outras do mesmo tipo: o foco fica na mesma janela.
- [x] Um agente chamar outro que não está aberto (`ask-agent codex "oi"`): a janela do Codex aparece **sem tirar o foco** de onde você digita. Se já houver Codex, vira aba na janela dele.
- [x] Botão de **tela dividida** na topbar: desligado, volta a uma janela só com todas as abas; ligado de novo, volta a dividir.
- [x] Janela mínima (~820 px) com 3 ou mais tipos: as janelas empilham; o que não cabe vira aba da última janela, sem sumir nada.
- [ ] Com um diff do Git aberto, a barra **mantém a largura** (o conteúdo rola dentro dela) e as janelas **não mudam de tamanho**.
- [x] Com 4–6 janelas e agentes respondendo ao mesmo tempo: o app continua fluido (anotar se travar).

## 4. Conversa entre agentes (ponte) ★

- [x] Peça ao Claude: *"pergunte ao antigravity qual é a capital da França"*. A pergunta é **digitada na aba do Antigravity** e a resposta volta ao Claude.
- [x] A mensagem digitada termina com `ask-agent --reply <número> "sua resposta"`, e o agente responde assim (a resposta chega).
- [x] **Resposta lenta:** peça uma tarefa demorada (mais de 90 s) a outro agente. O `ask-agent` do Claude termina avisando que a resposta virá depois; quando ela chega, é **digitada na aba do Claude** como mensagem nova.
- [ ] Numa aba **Terminal** (shell), rode `ask-agent claude "oi"` e deixe o Claude demorar: o comando **espera até o fim** (nada é digitado no seu shell).
- [x] Delegação por cargo: `ask-agent claude@dev "tarefa"` abre a aba **Claude · Dev** em segundo plano.
- [ ] Fechar a aba de quem perguntou no meio da espera: nada trava; a outra aba segue normal.
- [ ] Cadeia longa: A pergunta a B, B a C, e C tenta perguntar a um quarto agente → recusado com "profundidade máxima" (o limite é 2 saltos).

**Novidade: modelo por instância ★**

- [ ] `ask-agent --model sonnet claude@dev "diga oi"` de dentro de uma aba: abre "Claude · Dev" com o sufixo `sonnet` no cabeçalho e na barra lateral; o Claude mostra Sonnet (`/status`).
- [ ] Repetir com `--model opus` e a aba aberta: 409 `claude@dev já está aberta com sonnet; use --new para abrir outra com opus`. Com o mesmo modelo, ou sem `--model`, a entrega é normal. Com `--new --model opus`, abre `claude@dev@ana` com opus.
- [ ] `ask-agent --model opencode/big-pickle opencode "oi"`: a aba do OpenCode abre com "Big Pickle" na linha de status. Sem `--model`, abre com o padrão de sempre.
- [ ] `ask-agent --model x aider "oi"`: 400 "o agente aider não aceita escolher modelo". `--model "a b"`: "modelo inválido".
- [ ] `ask-agent --list` e `--team` mostram `claude@dev (sonnet)`.
- [ ] `"roleModels": {"claude@dev": "sonnet"}` no config.json: abrir `claude@dev` já usa sonnet, `claude@qa` cai no padrão e um `--model` explícito vence.
- [ ] Reabrir o app: as abas voltam com o mesmo modelo. Renomear mantém o modelo, e fechar a aba apaga a entrada de `tabModels`.
- [ ] "+ Novo agente" › do Claude lista Padrão, opus, sonnet, haiku, fable. O › do OpenCode e do Antigravity mostra "Carregando modelos…" e depois a lista (sem CLI ou rede, só "Padrão"). Clicar no nome abre com o padrão.
- [ ] Clique direito numa aba com modelo mostra "Modelo: <m>" no alto do menu.
- [ ] Antigravity com `--model claude-sonnet-5-5-medium`: o rodapé do agy mostra "Claude Sonnet 5.5 · medium".
- [ ] Codex com `--model <m>` (se instalado): abre com o modelo e o aviso de fim de turno continua funcionando.

**Novidade: tarefas (`ask-agent --task`) ★**

- [ ] Num projeto git, de uma aba orquestradora: `ask-agent --task new "teste" --start -` com o pedido na entrada. A aba da tarefa abre em segundo plano, o terminal dela está em `<repo>\.agentdeck\worktrees\t1`, o pedido e as regras chegam digitados, e o `git status` do repositório principal continua limpo.
- [ ] Na aba da tarefa, fazer um commit e rodar `ask-agent --task done t1 -`. O `--task show t1` mostra o relatório e o diff, e o `--boss` da tarefa chega a quem a criou.
- [ ] Na aba da tarefa, escrever o relatório num arquivo com acentos (ex.: `C:\Temp\relatorio.md`) e rodar `ask-agent --task done t1 --file C:\Temp\relatorio.md`: o `--task show t1` mostra os acentos inteiros (sem `?`).
- [ ] `--task return t1 "ajuste"` digita o retorno na aba. Depois, `--task merge t1` cria o commit de merge, fecha a aba e apaga a pasta e a branch, mesmo com o processo do agente vivo.
- [ ] Com o repositório principal em outra branch, ou com um arquivo rastreado alterado, o merge recusa com explicação e a tarefa fica em revisão.
- [ ] Conflito (mesma linha alterada na base e na tarefa): o merge é desfeito, a tarefa fica em revisão e o repositório principal fica limpo.
- [ ] `--task discard` numa tarefa em andamento fecha a aba e apaga a worktree e a branch, também com a pasta já apagada à mão.
- [ ] Reabrir o app com uma tarefa em andamento: a aba volta na worktree, e o número da próxima tarefa não se repete.
- [ ] Com "Abrir abas dos agentes" desligado, `--task start` recusa com explicação.
- [ ] **Aba Tarefas:** abrir o app mostra Tarefas como a primeira aba da barra direita. Num projeto git sem tarefas, aparece a explicação e "+ Nova tarefa".
- [ ] "+ Nova": com o título vazio, o botão fica desligado; a base vem com a branch atual; base inexistente dá erro inline; Ctrl+Enter cria. Com "Iniciar agora", o detalhe abre em andamento e a aba do agente aparece na worktree com o pedido digitado. Sem ele, a tarefa fica em rascunho e "Iniciar" a começa.
- [ ] Fechar a aba de uma tarefa em andamento: o detalhe mostra "Reabrir aba", que a abre de novo na worktree com o modelo da tarefa.
- [ ] O agente roda `ask-agent --task done tN -`: o cartão vai sozinho para "Em revisão", o selo do ícone sobe e aparece "N arquivos +X −Y".
- [ ] No detalhe em revisão: relatório, alterações por arquivo, e "Ver diff completo" abre o diff (cores, rolagem horizontal); "←" volta.
- [ ] "Devolver" com Ctrl+Enter: a tarefa volta a andamento e o texto chega à aba do agente, reabrindo-a se estiver fechada.
- [ ] "Mesclar em <base>" pede confirmação. Com o repositório principal sujo, mostra a recusa e a tarefa fica em revisão. Limpo, mescla, fecha a aba, move para Concluídas e apaga a worktree.
- [ ] "Descartar" pede confirmação; depois a aba fecha e a worktree e a branch somem.
- [ ] Na largura mínima (280 px), nada vaza na lista, no detalhe nem no formulário; trocar de projeto mostra as tarefas do outro.
- [ ] Uma tarefa criada por outro agente (`ask-agent --task new --start`) aparece sem clicar em atualizar.

## 5. Quando a mensagem é digitada na aba ★

- [ ] Enquanto **você digita** algo (sem Enter) na aba de um agente, uma pergunta de outro agente **não é digitada no meio** (espera ~20 s sem teclas).
- [ ] Com o Claude **no meio de uma resposta** (ou numa pausa longa de raciocínio), a pergunta espera ele terminar (hooks do Claude).
- [ ] Com o Claude parado num **pedido de permissão**, a pergunta não é digitada por cima do pedido.
- [ ] Apertar Enter num **menu** do Claude (ex.: `/model`) não trava a fila: uma pergunta seguinte ainda é entregue.
- [ ] Digitar logo depois de uma mensagem ser enviada à aba: suas teclas aparecem **depois** da mensagem, nunca misturadas.

## 6. Git ★

Com a pasta de trabalho num repositório git:

- [ ] O botão redundante da branch foi **removido da topbar**. O status da branch e contagem de alterações aparecem no topo da aba Git (`dev · N ▾`), no tooltip do ícone Git e no badge de alterações do ícone.
- [ ] A aba Git lista as alterações em seções; as letras têm cores (M âmbar, A verde, D vermelho…).
- [ ] Clicar num arquivo abre o **diff** (na largura da barra, com rolagem horizontal), com linhas numeradas e cores; **←** volta.
- [ ] Preparar (+) e tirar (−) um arquivo e "tudo" pelos botões das seções.
- [ ] Descartar (↶) pede **confirmação**; num arquivo não rastreado, avisa que vai **apagar**.
- [ ] Commit com mensagem (botão e **Ctrl+Enter**); "Corrigir último commit" traz a última mensagem.
- [ ] Commit sem `user.name`/`user.email` configurados: aparece a explicação de como configurar.
- [ ] Menu da branch: buscar, trocar de branch, criar branch nova (digitar o nome + Enter).
- [ ] HEAD destacado (detached HEAD): o botão da branch mostra `{hash} (destacado)` com a contagem de alterações.
- [ ] **Push** num repositório do GitHub por HTTPS: abre o login do **Git Credential Manager** e o envio conclui. Sem credencial: mensagem de erro, **sem travar**.
- [ ] Branch nova: o botão vira **Publicar** e cria o upstream.
- [ ] Buscar (⟳) e Puxar (↓n) depois de um commit feito em outro lugar.
- [ ] Fazer um commit **pelo terminal** (ou um agente fazer): a aba Git se atualiza sozinha.
- [ ] Caminhos com **acentos e espaços** aparecem certos e o diff abre.
- [ ] Pasta que não é repositório: aparece "Inicializar repositório" e funciona.
- [ ] (Se possível) sem git instalado / fora do PATH: aparece a mensagem com o link de instalação.
- [ ] Observar: o `AGENTS.md` da pasta aparece como alterado (já conhecido, está em pendências).

**Novidades: trechos, histórico, stash, merge/rebase e conflitos ★**

- [ ] Diff de um arquivo com duas regiões alteradas: "Preparar" num trecho deixa o arquivo nas duas seções (preparado e alterações); "Tirar" o devolve; "Descartar" no outro trecho pede confirmação e mantém o primeiro. Os botões não cobrem o texto do cabeçalho `@@`.
- [ ] O mesmo teste de trecho num arquivo com fim de linha CRLF (`autocrlf=false`).
- [ ] Menu da branch, "Mesclar" uma branch com conflito: aparecem a faixa "Mesclagem · N conflito(s)" e a aba "Conflitos". "Continuar" fica desativado até resolver tudo; "Abortar" pede confirmação e restaura o estado.
- [ ] Conflitos: "O nosso" num bloco e "O deles" noutro; "Marcar como resolvido" só fica ativo depois; "Continuar" conclui o merge; "Abrir" abre o arquivo no programa padrão.
- [ ] O mesmo fluxo com "Rebase" (Continuar e Abortar).
- [ ] Conflito "nós apagamos / eles alteraram": "Nosso" ou "Deles" no arquivo inteiro resolve; ↶ na seção Conflitos descarta com confirmação.
- [ ] Histórico: lista com autor e data; "Carregar mais" traz mais 50; clicar abre o detalhe (mensagem, arquivos, diff); depois de um commit novo a lista é relida.
- [ ] Stash: "Guardar" com mensagem e "Incluir não rastreados" tira os arquivos da pasta (o arquivo novo some e volta ao Aplicar). "Aplicar e apagar" remove da lista; "Apagar" pede confirmação.
- [ ] gpgsign: com `commit.gpgsign=true` e a chave travada, o commit falha em menos de ~5 s com a explicação, sem diálogo de senha. Depois de `echo teste | gpg --clearsign` numa aba, o commit passa.
- [ ] Push/pull por ssh com chave com senha fora do ssh-agent: "falha de autenticação" com o passo a passo do ssh-agent.
- [ ] Descartar (↶) uma pasta não rastreada com `.git` dentro: apaga a pasta.

## 6b. GitHub ★

- [ ] Aba GitHub (4º ícone da barra direita) num projeto com `origin` do GitHub: mostra `dono/repo`, a bolinha verde (login do GCM) e as listas PRs / Issues / Actions com contagens; o selo do ícone mostra o nº de PRs abertos; o compositor some na aba.
- [ ] Projeto sem `origin` ou com remoto fora do GitHub: a aba diz "Sem repositório no GitHub" e explica; "Tentar de novo" funciona depois de adicionar o remoto.
- [ ] Clicar num PR abre o detalhe embaixo (checks/jobs). Com job vermelho, o fim do log abre sozinho, rola nos dois sentidos sem alargar a barra e destaca as linhas de erro. "Log", "Corrigir ▾" e ↗ do job funcionam; o X volta à lista.
- [ ] "Atualizar" (ou abrir a aba depois de 60 s) relê os dados. Sem rede, a lista anterior continua e aparece a faixa de erro vermelha (fechável).
- [ ] Sem login: faixa "Sem login (60 por hora)". Num repositório privado, mensagem 404 explicando como entrar. O log de job pede login.
- [ ] "Abrir no navegador" (PR, issue, execução, job, repositório) abre a página certa.
- [ ] "Enviar para agente ▾" num PR, issue ou job vermelho: escolher uma aba aberta digita a mensagem pronta quando ela fica livre (não no meio da sua digitação); escolher modelo + cargo (ex.: claude@review) abre a aba se não existir e entrega; a mensagem aparece na Conversa integrada.
- [ ] Trocar de projeto com a aba aberta limpa a lista e lê o repositório do novo projeto.
- [ ] Barra direita no mínimo (280 px): nada corta além das reticências; janela baixa não derruba o app.

## 7. Segurança e ambiente

- [ ] Abrir **uma segunda janela** do CommandDeck: aparece a mensagem "outra janela já está usando a pasta de dados" e ela fecha.
- [ ] Com `COMMANDDECK_HOME` apontando para outra pasta, a segunda janela abre (instância separada).
- [ ] Abrir o CommandDeck **de dentro de uma aba** de outro CommandDeck: a aba de origem não cai.
- [ ] Colar (Ctrl+V) um texto grande com várias linhas no terminal de um agente: vira texto, não comandos soltos.

## 8. Agentes novos e shims do Windows

- [ ] O **Gemini não aparece** mais no "+" nem no `ask-agent --list` (o Antigravity é o agente do Google).
- [ ] **Codex**: abre pela aba "+"; ao terminar um turno, a fila de perguntas anda (aviso `notify`). Se ele pedir aprovação para rodar `ask-agent`, anotar.
- [ ] **OpenCode**: abre pela aba "+" e responde a um `ask-agent opencode "oi"`.
- [ ] Agente instalado **via npm sem `.exe`** (ex.: Codex ou OpenCode, `codex.cmd`/`opencode.cmd`): responder a um `ask-agent` sem aba aberta (headless) com uma mensagem com aspas, `&`, `|` e várias linhas → a resposta vem e **nada é executado** no cmd.
- [ ] Codex, OpenCode, Aider, Goose, Copilot, Amazon Q, Open Interpreter (os que tiver): ao receber uma pergunta numa aba, pedem aprovação para rodar `ask-agent`? Anotar quais pedem.

## 9. Teclado e entrada

- [ ] **IME real**: teclado chinês/japonês/coreano ou o seletor de emoji `Win+.` dentro do terminal — a pré-edição aparece sublinhada no cursor e o texto confirmado chega ao programa.
- [ ] Acentos do português no terminal e no compositor.
- [ ] **Ditado preso à aba (`Ctrl+Shift+M` ou ícone de microfone na aba)**: clicar no microfone da aba grava (pisca em vermelho), clicar de novo transcreve ("Transcrevendo…" na aba) e digita o texto no PTY daquela aba **sem Enter**; trocar de aba durante a gravação/transcrição mantém o destino na aba onde começou; fechar a aba antes do fim descarta o texto com aviso na Conversa.

## 10. Para fechar

- [ ] Tirar um print da tela principal (com a barra direita) para substituir `docs/screenshots/main.png`.

## 11. UI/UX: Novidades da Rodada (5 Tarefas) ★

- [ ] **Conversa integrada (mensagens do usuário)**:
  - [ ] Mensagens enviadas pelo usuário na aba ativa entram no feed com indicador vertical verde (`theme::USER = #10b981`), distinguindo de pronto das mensagens de IA.
  - [ ] Mensagens longas e multilinhas: quebra automática no card sem vazar a largura do painel e com altura dinâmica do indicador colorido.
- [ ] **Renomeação de agentes / abas**:
  - [ ] Duplo clique no título da aba ou clique direito → "Renomear".
  - [ ] Digitar novo cargo/nome (ex.: `dev`, `dev@ana` ou `claude@dev@ana`).
  - [ ] Nome vazio: restaura para o modelo original sem quebrar.
  - [ ] Nome duplicado: modal recusa com aviso em vermelho.
  - [ ] Nome com acento (`josé`) ou caracteres especiais (`!`, espaços): recusado amigavelmente com mensagem de formato esperado (`modelo@cargo@nome`).
  - [ ] Persistência: novo nome persiste na reinicialização (`config.custom_names`) e propaga no feed e no `ask-agent`.
- [ ] **Gestão de Projetos no explorador lateral**:
  - [ ] Seção `PROJETOS` colapsável no topo da árvore de arquivos, com ponto indicador no projeto ativo.
  - [ ] Botão `+`: seletor de pasta adiciona novo workspace à lista e ativa-o.
  - [ ] Botão `×` (aparece ao passar o mouse, com 2+ projetos): fica visível sob o ponteiro e o clique remove o projeto da lista; se for o ativo, migra automaticamente para o primeiro da lista.
  - [ ] Pasta inexistente ou inacessível: não quebra a aplicação; exibe erro amigável na conversa.
  - [ ] Lista de projetos persiste no arquivo de configuração (`config.projectList`).
- [ ] **Aba Git desacoplada da topbar**:
  - [ ] Topbar limpa (sem chip redundante da branch).
  - [ ] Status e contagem de alterações visíveis no botão da branch no topo da aba Git (`dev · N ▾`), com badge de alterações no ícone Git da lateral.
  - [ ] Repositório sem git: exibe aviso explicativo e botão para inicializar.
  - [ ] HEAD destacado: exibe `{hash} (destacado) · N ▾`.

## 12. Barra de título, uso dos agentes e barra lateral ★

- [x] A janela não tem mais a barra do Windows: no topo, ☰, o botão de painel (recolhe/expande a barra lateral), "CommandDeck" (sem logo) e +; à direita, os botões menores de minimizar, maximizar/restaurar e fechar (o fechar fica vermelho no hover).
- [ ] Arrastar pela barra move a janela; duplo clique maximiza/restaura; arrastar até a borda da tela encaixa (snap); Win+setas funcionam.
- [ ] Redimensionar pelas 4 bordas e pelos 4 cantos (o cursor muda); maximizada, as bordas não reagem e a janela não cobre a barra de tarefas.
- [ ] Fechar pelo × salva o tamanho da janela, como antes.
- [ ] ☰ abre as configurações alinhadas à esquerda.
- [x] Uso: com uma aba do Claude, depois da primeira resposta aparece o logo + círculo com o % da sessão; o tooltip mostra sessão, semana e quando renovam. A statusline do Claude mostra "sessão N% · semana M%".
- [ ] Antigravity aberto: em até ~3 min aparece o uso dele; sem login ou sem rede, anel tracejado e "uso indisponível".
  - Conferido: com login, o uso do Antigravity aparece. Falta o caso sem login/sem rede.
- [ ] Ollama: anel cinza, "local, sem limite". Fechar a última aba de um modelo tira o item da barra.
- [ ] Demonstração: `COMMANDDECK_DEBUG_UI=usage-demo` (Claude 34 %, Antigravity com a semana esgotada = anel vermelho fechado, Ollama local, Codex sem dados).
- [ ] Explorador sem a linha raiz repetida; "Atualizar" relê a árvore e "Recolher tudo" fecha as pastas.
- [x] Seção **ORGANIZAÇÃO** na barra lateral: a chave "Tela dividida" liga/desliga a divisão (a chave anima e fica na cor do agente ativo).

## 13. Projetos ★

- [x] **Migração:** a primeira abertura com a config antiga transforma cada pasta da lista em projeto; as abas que estavam abertas voltam no projeto da pasta ativa.
- [ ] "+" da seção PROJETOS: escolher uma pasta cria o projeto (nome = pasta) e o ativa; o centro mostra "Nenhum agente aberto".
- [ ] Abrir o Claude no projeto novo e clicar no projeto anterior: os terminais dele voltam como estavam; o Claude do novo **continua rodando** (o projeto mostra o logo e a contagem).
- [x] O projeto ativo lista as abas dele; clicar numa ativa a aba. Os outros mostram os logos dos agentes e a contagem.
- [ ] Cada terminal roda na pasta do seu projeto (`pwd`/`Get-Location`), e a árvore e a aba Git mudam com o projeto.
- [ ] A Conversa mostra só o projeto ativo; uma conversa entre agentes de outro projeto aparece quando ele volta à tela.
- [ ] `ask-agent claude@dev "oi"` num projeto abre `claude@dev` **nesse** projeto (escondido, se não for o da tela); o outro projeto não vê essa instância.
- [ ] Dois projetos com um `claude` cada: cada um responde só ao seu projeto.
- [ ] Fechar e reabrir o app: só as abas do projeto ativo abrem; as dos outros abrem ao clicar no projeto (e nada se perde).
- [ ] Clique direito → Renomear (o nome muda, a pasta não), Abrir pasta, Remover (confirma, fecha as abas do projeto; a pasta fica).
- [ ] Remover o último projeto: o centro pede "Abrir pasta…".
- [ ] Captura: `COMMANDDECK_DEBUG_UI=project:<id>` troca para o projeto `<id>` logo ao abrir.

## 14. Barra lateral: agentes em árvore e equipe ★

- [ ] "+ Novo agente" no fim da lista do projeto ativo abre o menu de agentes; Ctrl+T com a barra recolhida abre o menu sob a barra de título.
  - Conferido: o "+ Novo agente". Falta o Ctrl+T com a barra recolhida.
- [ ] Seta ▸/▾ de cada projeto e o botão de setas duplas do cabeçalho mostram/escondem os agentes; o estado volta igual ao reabrir o app.
- [ ] Clicar num agente de outro projeto troca de projeto e ativa a aba (inclusive de projeto ainda não aberto nesta sessão).
- [ ] Logo do modelo no lugar do ponto: halo pulsando trabalhando, anel ao concluir sem ser visto, apagado quando encerrado.
- [ ] Um agente que chama `ask-agent claude@xyz` (cargo novo) faz a aba nova aparecer sob ele; a primeira mensagem dela diz "você responde a …".
- [ ] Arrastar um agente sobre outro: contorno na cor do projeto e fantasma "→ equipe de …"; ao soltar, a árvore muda e continua assim depois de reabrir o app.
- [ ] Arrastar o chefe sobre alguém da própria equipe: contorno vermelho, cursor de proibido, nada muda. Esc cancela o arrasto.
- [ ] Soltar sobre a linha do projeto devolve o agente à raiz.
- [ ] Na aba de um agente reorganizado, `ask-agent --team` mostra o chefe e a equipe certos; `ask-agent --boss "oi"` chega ao chefe; sem chefe, o `--boss` explica que ele responde ao usuário.
  - Conferido: no chefe, `--team` mostra a equipe e `--boss` explica que ele responde ao usuário. Falta conferir na aba do subordinado.
- [ ] Depois de reorganizar, a próxima mensagem entregue ao agente termina com "[CommandDeck] Sua posição na equipe mudou…" (uma vez só; se ele rodou `--team` antes, não vem).
- [ ] Fechar a aba de um chefe: a equipe dele sobe para o chefe dele (ou para a raiz). Renomear uma aba com equipe mantém a árvore.

## 15. Auditoria de 2026-10-15: conferir no Windows ★

- [ ] Git: abrir "Descartar" num arquivo do projeto A e trocar para o projeto B pela barra lateral: a confirmação some e nada é descartado em B.
- [ ] Git: na lista de branches, clicar numa remota (`origin/x`) cuja local já existe: troca para a local, sem erro.
- [ ] Enter num prompt vazio do Claude/terminal: a aba não fica "trabalhando" (o halo para em ~8 s, sem campainha).
- [ ] Ctrl+C com o teclado em layout russo/grego interrompe o programa do terminal.
- [ ] Colar (Ctrl+V) num shell com *bracketed paste* um texto com `ESC[201~` no meio: tudo chega como colagem, nada é executado.
- [ ] Aba do Ollama: "quem é você?" responde com o nome da instância (`ollama:<modelo>`); com o Ollama parado, quem perguntou recebe "não consegui responder" em vez de esperar 10 min.
- [ ] `ask-agent claude@dev "/review src"` pelo Bash do Claude chega com `/review` intacto.
- [ ] Ditado: gravar, clicar Enviar e trocar de aba durante a transcrição: o texto fica na caixa com o aviso, não vai para a outra aba.
- [ ] Tela dividida: arrastar um divisor, estreitar a janela até a grade mudar de formato e alargar de novo. As proporções arrastadas voltam, também depois de reabrir o app.
- [ ] Renomear uma aba (claude → claude@dev) enquanto ela está "pensando" na Conversa: as mensagens antigas mostram o nome novo, e o "pensando" some com a próxima mensagem dela.
- [ ] Com um modal aberto (renomear aba ou projeto, Ctrl+/), clicar nas abas, na barra lateral ou no terminal não faz nada, e o que se digita não vai ao terminal. Esc fecha e devolve o foco.
- [ ] Ctrl+/ com o terminal focado abre e fecha a tela de atalhos.
- [ ] Clique direito num projeto → Remover: a janela continua desenhando com a pergunta aberta. "Sim" remove, "Não" mantém.
- [ ] Shell focado: Ctrl+L limpa a tela, Ctrl+B chega ao programa (tmux) e Ctrl+T abre aba. Com "Atalhos do app acima do terminal" ligado, Ctrl+L foca a mensagem e Ctrl+B recolhe a barra. A opção continua depois de reabrir.
- [ ] Rascunho e anexo no projeto A, trocar para o B (compositor vazio) e voltar: rascunho e anexo estão lá, e o `@` enviado aponta para o arquivo certo. O "×" apaga a cópia em `dados/anexos`.
- [ ] Microfone: a janela não congela enquanto ele abre, e um segundo clique desiste. Gravar mais de 5 min para sozinho com aviso e transcreve.
- [ ] Uso: passado o horário de renovação da sessão sem o agente rodar, o círculo mostra 0% esmaecido e "sessão renovada (sem dados novos)".
- [ ] crash.log: usar o app normalmente não acrescenta nada; `COMMANDDECK_DEBUG_CRASH=heap` ainda registra a queda; um crash.log acima de 256 KB vira crash.log.1 ao abrir.
- [ ] Ponte: pergunta com resposta atrasada (202); renomear a aba de quem perguntou e a do destino antes da resposta. A resposta é digitada na aba renomeada de quem perguntou.
- [ ] Ponte: depois de um 202, o destino roda `ask-agent <quem perguntou> "pergunta"`. Chega como pergunta nova, e a original continua esperando o `--reply N`.
- [ ] Ponte: responder pela forma antiga com apelido (`ask-agent agy "..."` para uma pergunta do Antigravity) entrega a resposta, sem 409.
- [ ] Projetos: remover um projeto com pergunta em andamento entre abas dele. Quem perguntou recebe erro, e nada reaparece no projeto removido.
- [ ] Equipe: mudar o chefe de um agente e fechar a aba dele antes da próxima mensagem não gasta o aviso. Fechar um subordinado sem equipe avisa o chefe na próxima mensagem.
- [ ] Ollama: duas perguntas para a mesma aba do Ollama (de agentes diferentes, ou com renomeação no meio) voltam cada uma a quem perguntou.
- [ ] Colagem grande (~5 MB) numa aba de shell rodando `timeout /t 30`, que não lê a entrada: a janela continua respondendo e as outras abas funcionam. Quando o comando termina, o texto chega inteiro.
- [ ] Enviar pelo compositor para o Claude e digitar logo em seguida: a ordem é colagem → Enter → teclas.
- [ ] Pasta de dados com `&` no caminho (`COMMANDDECK_HOME=C:\tmp\Ana&Bia`): a aba do Claude (shim npm) abre e os hooks de turno funcionam.
- [ ] Numa aba em segundo plano ou de outro projeto, `printf '\e]52;c;%s\a' $(printf teste|base64)` não muda a área de transferência; na aba ativa, muda.
- [ ] Headless ("Abrir abas dos agentes" desligado): fazer uma pergunta e matar o commanddeck.exe pelo Gerenciador de Tarefas durante a resposta. O claude/node do headless some junto.
- [ ] No vim/htop com mouse ligado, Ctrl+roda e Alt+roda chegam ao programa com o modificador. Shift+roda rola o histórico do terminal, com ou sem programa usando o mouse.
- [ ] Projeto que é uma worktree git (`git worktree add`) ou subpasta de repositório: commit, `git cherry-pick`/`git revert` com conflito e `git rebase -i` pelo terminal atualizam a aba Git sem reabrir o projeto.
- [ ] Abrir o projeto por um caminho com outra caixa na unidade (`c:\...`): a árvore e o Git atualizam.
- [ ] Download do Whisper: desconectar o Wi-Fi no meio. A nova tentativa aparece em ~30 s, sem travar por horas. Com o antivírus ativo, o download de 1,6 GB termina sem erro de rename.
- [ ] Whisper CUDA: matar o app durante a extração. Na próxima abertura não existe `bin-cuda` pela metade, o `bin-cuda.staging` some e o `.zip` em `whisper\` é reaproveitado.
- [ ] Ditado duas vezes seguidas, rápido: nenhum `audio-*.wav` sobra em `dados\anexos`. Fechar o app durante uma transcrição não deixa `whisper-cli.exe` rodando.
- [ ] Antigravity com token vencido no Credential Manager e válido em `~\.gemini\oauth_creds.json`: a cota aparece. Sem login, o círculo reaparece em até 20 s depois do login.
- [ ] Codex: abrir uma aba nova (rollout sem `rate_limits`): o uso da sessão anterior continua visível.
- [ ] Arrastar devagar a borda da barra direita ou o divisor com o Claude aberto: durante o arrasto a aba não se redesenha; ao soltar, o Claude se redesenha uma vez na largura nova (no histórico fica no máximo uma cópia antiga por arrasto).
- [ ] Com o Claude trabalhando, abrir um terceiro tipo de agente (a tela vira 2+1) e arrastar os divisores: a aba do Claude se redesenha em seguida, sem ficar preta nem parada.
- [x] Repositório com `core.autocrlf=true` e `AGENTS.md` em CRLF: abrir o app não deixa o arquivo alterado no `git status`.
  - Conferido no `commanddeck-dev`: o conteúdo tem o mesmo hash do HEAD. O `M` que ainda aparecia era o índice desatualizado; um `git add AGENTS.md`, sem mudar nada, limpou.
