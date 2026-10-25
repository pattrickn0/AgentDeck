<!-- agentdeck:start -->
Você está rodando dentro do AgentDeck, um ambiente que conecta vários agentes de IA. Seu nome e cargo estão nas variáveis AGENTDECK_AGENT e AGENTDECK_ROLE; nunca pergunte a si mesmo.
Para falar com outro agente, rode no shell: ask-agent <agente> "mensagem". Precisando de outro agente, abra-o no AgentDeck (ask-agent <agente>, ask-agent <agente>@<cargo> ou ask-agent --task new): NÃO use subagentes internos do seu CLI (Task/Agent tool, sub-agents), a menos que o usuário peça.
A resposta sai no stdout e pode levar minutos (timeout de até 600000 ms). Se o ask-agent avisar que a resposta será digitada depois neste terminal, não repita a pergunta; se ela não aparecer, rode ask-agent --inbox (--all: as 20 últimas).
ask-agent --models [<agente>] lista os agentes e modelos desta máquina. O histórico de cada par é mantido e o usuário vê tudo na "Conversa integrada": não relate nem resuma a conversa; dialogue com o outro agente até concluir, sem parar para perguntar ao usuário.
Cada aba é uma instância <modelo>[@<cargo>[@<nome>]] (claude, claude@dev, claude@dev@ana): ask-agent claude@dev "tarefa" abre a aba se não existir; --new abre outra do mesmo cargo; --model <modelo> escolhe o LLM ao abrir. Trabalho grande vira tarefa, com worktree, branch e agente próprios: ask-agent --task new "título" --start --file <pedido.md>.
Texto com acento vai num arquivo (--file <arquivo>): pela entrada padrão o shell do Windows troca os acentos por "?".
Quando outro agente lhe mandar uma mensagem, responda com o comando indicado nela (ask-agent --reply <número> "resposta" ou --file <arquivo>): só assim a resposta chega a ele.
Os agentes formam uma equipe (chefe e subordinados; o usuário pode reorganizá-la). Antes de delegar ou reportar, veja a sua posição com ask-agent --team; delegue de preferência à equipe e, ao concluir algo que o chefe pediu, reporte com ask-agent --boss "mensagem".
Memória do projeto: memory/aprendizados/, um .md por armadilha, decisão ou convenção que custou tempo e o código não mostra. Formato: cabeçalho --- titulo: … / resumo: uma linha / touches: [globs dos arquivos; opcional] --- e o corpo curto, com Por quê e Como aplicar. Antes de mexer em arquivos, rode ask-agent --learn <arquivo>... (sem argumentos: o índice e a situação das tarefas; com arquivos, avisa se outra tarefa mexe neles); ao descobrir algo assim, crie o arquivo. ask-agent --outline <arquivo>... mostra as assinaturas com a linha, para ler só o trecho necessário de um arquivo grande (offset).
<!-- agentdeck:end -->

## Memória do projeto

A documentação e a memória do projeto ficam em `memory/`, versionadas no git. Leia antes de mexer no que não conhece:

- `memory/GERAL.md`: ponto de partida, com a visão geral e o mapa dos outros documentos e das regras de cada um.
- `memory/AGENTDECK.md`: visão e rumo do produto (o AgentDeck) e quem decide.
- `memory/ARCHITECTURE.md`: como o código está organizado (crates, módulos, fluxos).
- `memory/MANUTENCAO.md`: como compilar, testar e manter.
- `memory/pendencias.md`: o trabalho que falta, em ordem de prioridade.
- `memory/testes.md`: roteiro de testes manuais no Windows.

A memória funciona em duas camadas:

- **Aprendizados** (longo prazo): `memory/aprendizados/`, um arquivo por armadilha, decisão ou convenção que não é óbvia no código. O formato e quando criar estão em `memory/aprendizados/README.md`. Para ver os que valem para os arquivos em que você vai mexer, rode `ask-agent --learn <arquivo> [<arquivo>...]`.
- **Situação atual** (curto prazo): no início de cada tarefa, o app entrega as outras tarefas em andamento, os arquivos que elas mexem e os últimos commits da base. Fora de tarefa, consulte `ask-agent --task list` e `git log`.
