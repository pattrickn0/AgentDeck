---
name: agentdeck-ui
description: Regras visuais e de código da interface do AgentDeck (egui, crates/app/src/ui e theme.rs). Use antes de criar ou mudar qualquer coisa na interface do app — barra lateral, barra de título, tela dividida, barra direita, menus, modais, ícones, cores, espaçamentos — e para conferir o resultado com captura de tela.
---

# Interface do AgentDeck

O app é egui puro, sem widgets prontos na maior parte: cada componente é **pintado à mão** com o `Painter` e
recebe interação por `ui.interact`/`clickable`. Siga o que já existe antes de inventar.

Referências de apoio (instaladas no projeto): `game-ui-design` (HUD lido de relance, estados visíveis,
densidade) e `rust-desktop-applications` (`references/native-gui-frameworks.md` para egui). Quando divergirem
destas regras, valem estas.

## Princípios do produto

1. **Os agentes são o centro.** O que importa de relance: quais agentes existem, qual está trabalhando, qual
   terminou sem ser visto, quem responde a quem. Arquivos, git e configurações são secundários.
2. **Estado visível sem clicar.** Trabalhando = halo pulsando na cor do agente; concluído e não visto = anel;
   encerrado = apagado (40 %) e riscado. Reaproveite `paint_agent_mark` (`ui/projects.rs`) e o mesmo pulso de
   `ui/tabs.rs` — não crie outro código visual para o mesmo estado.
3. **Agente se identifica pelo logo do modelo** (`Icon::ClaudeLogo`, `AntigravityLogo`, `OllamaLogo`,
   `CodexLogo`), na cor do agente; o ponto (`dot`) só para quem não tem logo (terminal).
4. **Hierarquia por recuo e linhas-guia**, como no explorador de arquivos (`agent_tree`/`paint_guides`).
5. **Sem decoração gratuita.** Cor de destaque só para estado ou seleção; nada de pintar um cabeçalho inteiro
   com a cor de um agente (vários agentes dividem o mesmo projeto).

Decisões já tomadas pelo usuário (não reabrir sem ele pedir):
- "+ Novo agente" fica **dentro** do projeto ativo, no fim da lista de agentes; o "+" do cabeçalho PROJETOS
  cria projeto. Nada de "+" solto na barra de título.
- A seção ORGANIZAÇÃO continua sendo **seção** (vai ganhar outros modos, como a tela dividida com destaque),
  não um ícone no rodapé. Rodapé com atalhos: por enquanto não.

## Tokens (`crates/app/src/theme.rs`)

Cores vêm da paleta semântica: `theme::pal().<token>` (`Palette`, `DARK`). Nunca escreva cor solta: o teste
`colors_come_from_the_palette` falha com `Color32::from_rgb`/`from_rgba_*`/`from_gray`/`theme::rgba(` fora de
`theme.rs`, `terminal/colors.rs` e `icons.rs` (lista `STILL_LOOSE`, hoje vazia: só encolhe).

| Token | Valor | Uso |
| :-- | :-- | :-- |
| `bg` | #16181c | fundo da janela, por trás das ilhas |
| `glass` / `glass_border` / `glass_highlight` | branco-cinza 50% / branco 13% / branco 8% | fundo, borda de 1 px e filete do topo das ilhas |
| `well` | rgba(22,24,28,.92) | ilha central (terminal) |
| `strip` | rgba(44,47,54,.55) | faixa de abas |
| `separator`, `guide` | branco 10% / 8% | linhas dentro das ilhas, guias da árvore |
| `hover`, `selected`, `selected_soft` | branco 7% / 12% / 8% | hover, seleção neutra, arquivo selecionado |
| `recess`, `chip` | preto 25% / branco 7% | trilho de controles e campos; etiquetas |
| `popup` | #23262c | menus, modais, dicas (opaco) |
| `text`, `text_strong`, `text_2`, `text_3`, `text_soft`, `text_faint` | #e6e8eb, #fff, #aab1bb, #939aa4, #c9cdd3, #6b717c | texto: principal, selecionado, secundário, rótulos/metadados, valores, separadores |
| `on_accent` | #16181c | texto sobre a cor de destaque |
| `danger`, `danger_text`, `success`, `warning`, `info` | | estados |
| `scrim`, `shadow` | preto 50% / 28% | película do modal, sombra das ilhas |

Cores dos agentes (não são da paleta): `CLAUDE`, `ANTIGRAVITY`, `OLLAMA`, `SHELL` ou `deck.agents.tab_color(kind)`.
Terminal: `TERM_BG` #16181c, `TERM_FG`, `TERM_SELECTION` #2f3542.

**A cor de destaque é a do agente ativo**, nunca uma cor fixa de marca: `deck.accent()` (ou `theme::accent()`, que
`ui::draw` atualiza a cada quadro; sem aba, cinza neutro `#c9cdd3`). Ela marca seleção, ação primária, divisor ativo e o
brilho do canto do fundo. Texto sobre ela: `on_accent`. Ajudantes: `accent_soft` (18%, linha selecionada),
`accent_wash` (10%, cartão em foco), `accent_edge` (50%, borda do cartão), `with_alpha`, `with_opacity`, `brighten`, `over`.

Fontes (embutidas, OFL, `assets/fonts/`): `theme::ui(px)` Inter, `ui_medium`, `ui_semibold`, `ui_bold`; `theme::label(px)`
IBM Plex Mono Medium para rótulos de seção em MAIÚSCULAS (10 px, espaçamento 0,1 em, `ui::section_label`);
`theme::meta(px)` Plex Mono Regular para metadados (10–10,5 px). Texto base 12 px. O terminal mantém a fonte do sistema.
Símbolos (▾ ▸ ✕) caem no fallback do sistema.

## Ilhas e fundo

- Barras e centro são **ilhas** de cantos 10 px (`ISLAND_RADIUS`), com margem e fresta de 6 px (`ISLAND_GAP`), sobre o
  fundo `bg` com dois brilhos radiais (`ui/background.rs`, `Mesh` em leque). Sem desfoque (o egui não tem).
- Ordem: `island_shadows` (sombra só nas frestas, o vidro é translúcido) → `island(ui, rect, kind)` (`Glass`/`Well`) →
  conteúdo → `island_edge(ui, rect)` (repinta os cantos com a cor do fundo, borda e filete). Cada janela da tela dividida é
  uma ilha `Well`. Divisores ficam na fresta (`paint_splitter` só aparece com o mouse).
- Componentes comuns em `ui/mod.rs`: `segmented`, `button` (`ButtonKind::{Primary, Secondary, Ghost}`), `section_label`,
  `chip`, `separator`, `galley_spaced`.
- Logo: `Icon::AgentDeck` (`icons.rs`, `LOGO_CARDS`); o `.ico` sai de `scripts/gerar-icone.ps1`.

## Medidas

- Barra lateral, de cima para baixo: chave Agentes | Documentos (`docs::switch`, trilho de 28 px com segmentos de 24 px,
  margem de 6 px, segmento ativo na cor de destaque; "Docs" se o segmento não comporta "Documentos n"; com a barra
  recolhida vai para a barra de título, compacta, 24 px), `separator`, PROJETOS, ORGANIZAÇÃO (`segmented` sem
  destaque, trilho de 24 px, margem de 10 px), ARQUIVOS. Separadores com a largura da ilha.
- Linhas da barra lateral têm margem de 4 px (a seleção não encosta na borda): projeto 22 px (▾/▸, nome em 600,
  contagem em `text_3`, anel 6 px do agente que terminou sem ser visto), **agente 24 px** (`AGENT_ROW_H`), "+ Novo agente"
  22 px em `text_3`, arquivo 20 px (11,5 px; `FILE_ROW_H`).
- Árvore de agentes: ícone do modelo (12 px numa caixa de 16) com o centro no x 30 da linha, recuo de 12 px por nível
  (até 5), nome em Inter 12 px/500 (600 se selecionado) 7 px depois da caixa, **modelo à direita** em `meta(10)`,
  `text_3`, encurtado por `projects::model_tag` (`sonnet`, `argon`, `flash`): o nome é quem corta (reticências), nada
  quebra linha. Selecionado: `accent_soft` + filete de 2 px na cor de destaque, nome `text_strong`, modelo `text`.
- Explorador: recuo de 12 px, ícones de extensão em `text_3` (cinza, menos cor), letra do Git (M, A, ?, D, R, U) em
  `meta(10)` `text_2`; selecionado `selected_soft`, hover `hover`.
- Abas (`ui/tabs.rs`): faixa de 30 px (`HEAD_H`), pílulas de 22 px, cantos 5, 3 px entre elas, padding 8; ativa com a
  cor do agente, texto `on_accent` 600 e sombra leve; sem o foco da janela ou de documento, a ativa é `selected` +
  `text_strong`; ponto de 7 px (halo trabalhando, anel de 1,5 px concluído sem ser visto, 40% sem processo); o ✕ é
  desenhado (a Inter não o tem). `StripKind::colored` escolhe entre agente e documento.
- Botões de ícone: na barra de título 26×24, cantos 5; nos cabeçalhos 20×20, cantos 4; hover `hover`.
- Linha da lista de arquivos de um diff: 20 px, selecionada `selected_soft`; letras e `+`/`−` em `meta(10)`.
- Menus e modais: `frame(p, r, 6–10, popup, glass_border)` + `shadow(...)`; itens de 26 px; posição com
  `menus::keep_on_screen` (não use `Area::constrain`, ele fecha o menu no clique — ver o comentário em `menus.rs`).
- Barra de título: `TITLEBAR_H = 32`, sem fundo; botões da janela 40×32. Barra lateral: 150–480 px (padrão 220).
  Barra direita: 280–640 (padrão 330).

## Padrões de código

- **Ajudantes de `ui/mod.rs`:** `galley`, `galley_trunc` (reticências), `place`, `text_in`, `text_w`, `dot`,
  `frame`, `shadow`, `clickable` (cursor de mão), `tip` (dica ao passar o mouse; toda ação sem texto precisa
  de uma), `paint_splitter`.
- **Interação:** pinte primeiro e decida depois. Guarde a ação numa variável dentro da closure
  (`switch_to_id`, `agent_click`...) e aplique em `deck` **fora** dela (o borrow do `Deck` não deixa antes).
- **Ids estáveis** para o que é arrastável ou tem estado entre quadros: `Id::new(("agent-row", &proj.id, &ag.id))`,
  não o id automático de `allocate_exact_size`.
- **Estado efêmero** (menu aberto, arrasto, âncoras) em `ctx.data_mut(|d| d.insert_temp(...))`; limpe quando
  a seção some (ex.: barra recolhida). **Estado que o usuário espera de volta** vai para a config
  (`Config`/`ProjectCfg` no core), não para `ctx.data`.
- **Overlay acima de tudo** (fantasma de arrasto, dicas): `ctx.layer_painter(LayerId::new(Order::Tooltip, ...))`.
- **Animação:** `ctx.animate_bool_with_time(id, on, 0.12–0.15)`; pulso com `ctx.input(|i| i.time)` e
  `request_repaint_after(33 ms)` só enquanto o estado dura.
- **`dragging`** (parâmetro das seções da barra lateral) desliga o hover enquanto algo é arrastado.
- **Lógica fora da pintura:** cálculo de layout/árvore/ordem vai para função pura com testes no mesmo arquivo
  (ex.: `agent_tree`, `in_subtree`, `split.rs`). O estado de verdade (instâncias, equipe) mora no core.
- **Textos** em português do Brasil, frases curtas; dicas dizem o que o clique faz.

## Conferir o resultado (captura de tela)

Compile e rode numa pasta de dados separada (a do usuário está em uso), com os projetos fictícios:

```sh
cargo build -p agentdeck
AGENTDECK_HOME=<scratch>/home AGENTDECK_SMOKE=<scratch>/shot AGENTDECK_DEBUG_UI=projects-demo \
  ./target/debug/agentdeck.exe        # salva <scratch>/shot/screen.png e sai
```

Outros estados em `AGENTDECK_DEBUG_UI` (lista em `apply_debug_ui`, `app.rs`): `settings`, `newtab`,
`usage-demo`, `git-demo`, `split-drag`... Abra a imagem e confira alinhamento, truncamento e contraste antes de
dizer que terminou; interação (clique, arrasto) não aparece na captura — diga ao usuário o que falta testar à mão.
