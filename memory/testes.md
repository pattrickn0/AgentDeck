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

---

## 0. Preparação

- [ ] Compilar e abrir: `cargo build --workspace --release` e `.\target\release\commanddeck.exe` (os três `.exe` na mesma pasta).
- [ ] Ter instalados, se possível: Claude Code, Antigravity, Git for Windows. Opcionais: Codex (`npm i -g @openai/codex`), OpenCode, Ollama.
- [ ] Trocar a pasta de trabalho para um projeto seu que seja repositório git (clique no caminho na topbar), ou usar a pasta `workspace/` padrão.

## 1. Barra direita ★

- [ ] A Conversa integrada e o compositor aparecem à **direita**, ocupando a altura toda; o terminal ocupa a altura toda no centro.
- [ ] Arrastar a borda esquerda da barra muda a largura (entre ~300 px e ~metade da janela); **não existe** como recolher.
- [ ] Fechar e reabrir o app: a largura escolhida volta igual.
- [ ] Diminuir a janela até o mínimo (~820 px de largura): nada se sobrepõe; o terminal continua usável.
- [ ] Compositor: "Enviar para" + seletor na 1ª linha, anexos embaixo, caixa de texto na largura toda, microfone/anexo à esquerda e **Enviar** à direita.
- [ ] Abrir o seletor "Enviar para" com a barra estreita: a lista fica **inteira dentro da tela** e dá para escolher um item (o menu não fecha sozinho no clique).
- [ ] Menu "+" (nova aba) perto da borda direita da janela: abre dentro da tela e a escolha funciona.
- [ ] Download do Whisper (se aparecer): a faixa de progresso fica logo abaixo do topo da barra.

## 2. Ícones do topo da barra ★

- [ ] Três ícones: **balões** (Conversa), **diagrama** (Mapa), **branch** (Git). Passar o mouse mostra o nome de cada um.
- [ ] Os ícones ficam nítidos (sem serrilhado estranho) e o ícone de branch mostra os **círculos vazados**.
- [ ] Ficar no Mapa ou no Git enquanto chegam mensagens: aparece um **número** sobre o ícone da Conversa; voltar à Conversa zera.
- [ ] "Limpar" só aparece na Conversa.

## 3. Conversa entre agentes (ponte) ★

Abra uma aba do Claude e uma do Antigravity.

- [ ] Peça ao Claude: *"pergunte ao antigravity qual é a capital da França"*. A pergunta é **digitada na aba do Antigravity** e a resposta volta ao Claude.
- [ ] A mensagem digitada termina com `ask-agent --reply <número> "sua resposta"`, e o agente responde assim (a resposta chega).
- [ ] **Resposta lenta:** peça uma tarefa demorada (mais de 90 s) a outro agente. O `ask-agent` do Claude termina avisando que a resposta virá depois; quando ela chega, é **digitada na aba do Claude** como mensagem nova.
- [ ] Numa aba **Terminal** (shell), rode `ask-agent claude "oi"` e deixe o Claude demorar: o comando **espera até o fim** (nada é digitado no seu shell).
- [ ] Delegação por cargo: `ask-agent claude@dev "tarefa"` abre a aba **Claude · Dev** em segundo plano.
- [ ] Fechar a aba de quem perguntou no meio da espera: nada trava; a outra aba segue normal.
- [ ] Cadeia longa: A pergunta a B, B a C, e C tenta perguntar a um quarto agente → recusado com "profundidade máxima" (o limite é 2 saltos).

## 4. Quando a mensagem é digitada na aba ★

- [ ] Enquanto **você digita** algo (sem Enter) na aba de um agente, uma pergunta de outro agente **não é digitada no meio** (espera ~20 s sem teclas).
- [ ] Com o Claude **no meio de uma resposta** (ou numa pausa longa de raciocínio), a pergunta espera ele terminar (hooks do Claude).
- [ ] Com o Claude parado num **pedido de permissão**, a pergunta não é digitada por cima do pedido.
- [ ] Apertar Enter num **menu** do Claude (ex.: `/model`) não trava a fila: uma pergunta seguinte ainda é entregue.
- [ ] Digitar logo depois de uma mensagem ser enviada à aba: suas teclas aparecem **depois** da mensagem, nunca misturadas.

## 5. Mapa de agentes ★

- [ ] Abrir o Mapa: a barra **se alarga sobre o terminal** e o terminal **não muda de tamanho** (o texto do agente não se reorganiza).
- [ ] Clicar no terminal visível ao lado do mapa funciona; clicar sobre o mapa não vaza clique para o terminal.
- [ ] Com dois agentes conversando: aparece `AO VIVO`, a árvore `claude ─► antigravity #n`, o estado (na fila / digitada / 202) e o tempo correndo.
- [ ] Ao terminar, a conversa aparece em `RECENTES` (esmaecida) e some depois de ~1 minuto.
- [ ] Agentes sem conversa aparecem em `SEM CONVERSA` com ● ocupado / ○ livre / × encerrado.
- [ ] Clicar num agente no mapa abre a aba dele.
- [ ] Os traços `─│├└►` e os símbolos `●○×` ficam **alinhados** (colunas certas) na fonte do Windows.
- [ ] Rodar uma **mesa** (Enviar para → mesa): aparece `MESA · rodada x/y` com quem está falando.

## 6. Git ★

Com a pasta de trabalho num repositório git:

- [ ] O **chip da branch** aparece na topbar (`main •3`, com ↑/↓ quando houver); clicar nele abre a aba Git.
- [ ] A aba Git lista as alterações em seções; as letras têm cores (M âmbar, A verde, D vermelho…).
- [ ] Clicar num arquivo abre o **diff** (a barra se alarga), com linhas numeradas e cores; **←** volta.
- [ ] Preparar (+) e tirar (−) um arquivo e "tudo" pelos botões das seções.
- [ ] Descartar (↶) pede **confirmação**; num arquivo não rastreado, avisa que vai **apagar**.
- [ ] Commit com mensagem (botão e **Ctrl+Enter**); "Corrigir último commit" traz a última mensagem.
- [ ] Commit sem `user.name`/`user.email` configurados: aparece a explicação de como configurar.
- [ ] Menu da branch: buscar, trocar de branch, criar branch nova (digitar o nome + Enter).
- [ ] **Push** num repositório do GitHub por HTTPS: abre o login do **Git Credential Manager** e o envio conclui. Sem credencial: mensagem de erro, **sem travar**.
- [ ] Branch nova: o botão vira **Publicar** e cria o upstream.
- [ ] Buscar (⟳) e Puxar (↓n) depois de um commit feito em outro lugar.
- [ ] Fazer um commit **pelo terminal** (ou um agente fazer): a aba Git e o chip se atualizam sozinhos.
- [ ] Caminhos com **acentos e espaços** aparecem certos e o diff abre.
- [ ] Pasta que não é repositório: aparece "Inicializar repositório" e funciona.
- [ ] (Se possível) sem git instalado / fora do PATH: aparece a mensagem com o link de instalação.
- [ ] Observar: o `AGENTS.md` da pasta aparece como alterado (já conhecido, está em pendências).

## 7. Segurança e ambiente

- [ ] Abrir **uma segunda janela** do CommandDeck: aparece a mensagem "outra janela já está usando a pasta de dados" e ela fecha.
- [ ] Com `COMMANDDECK_HOME` apontando para outra pasta, a segunda janela abre (instância separada).
- [ ] Abrir o CommandDeck **de dentro de uma aba** de outro CommandDeck: a aba de origem não cai.
- [ ] Colar (Ctrl+V) um texto grande com várias linhas no terminal de um agente: vira texto, não comandos soltos.

## 8. Agentes novos e shims do Windows

- [ ] **Codex**: abre pela aba "+"; ao terminar um turno, a fila de perguntas anda (aviso `notify`). Se ele pedir aprovação para rodar `ask-agent`, anotar.
- [ ] **OpenCode**: abre pela aba "+" e responde a um `ask-agent opencode "oi"`.
- [ ] Agente instalado **via npm sem `.exe`** (ex.: Codex ou OpenCode, `codex.cmd`/`opencode.cmd`): responder a uma pergunta numa **mesa** com uma mensagem com aspas, `&`, `|` e várias linhas → a resposta vem e **nada é executado** no cmd.
- [ ] Codex, OpenCode, Aider, Goose, Copilot, Amazon Q, Open Interpreter (os que tiver): ao receber uma pergunta numa aba, pedem aprovação para rodar `ask-agent`? Anotar quais pedem.

## 9. Teclado e entrada

- [ ] **IME real**: teclado chinês/japonês/coreano ou o seletor de emoji `Win+.` dentro do terminal — a pré-edição aparece sublinhada no cursor e o texto confirmado chega ao programa.
- [ ] Acentos do português no terminal e no compositor.

## 10. Para fechar

- [ ] Tirar um print da tela principal (com a barra direita) para substituir `docs/screenshots/main.png`.
