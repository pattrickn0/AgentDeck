---
name: commanddeck-ui
description: Regras visuais e de código da interface do CommandDeck (egui, crates/app/src/ui e theme.rs). Use antes de criar ou mudar qualquer coisa na interface do app — barra lateral, barra de título, tela dividida, barra direita, menus, modais, ícones, cores, espaçamentos — e para conferir o resultado com captura de tela.
---

# Interface do CommandDeck

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
- O Mapa de agentes tende a sair (a árvore da barra lateral cobre o "quem é de quem").

## Tokens (`crates/app/src/theme.rs`)

Use sempre as constantes; não escreva cor solta no código.

| Uso | Token |
| :-- | :-- |
| Fundo da janela / terminal | `BG` (#0b0b0d) |
| Barras (título, cabeçalhos de janela) | `PANEL` (#131317) |
| Hover, item ativo, menus | `PANEL_2` (#1b1b21) |
| Bordas, separadores, linhas-guia, item selecionado | `BORDER` (#2a2a33) |
| Texto principal / secundário | `TEXT` / `MUTED`; árvore: `TREE_TEXT`, `TREE_ICON` |
| Ação primária (botão Salvar) | `ACCENT` |
| Perigo / erro | `RED`, `RED_TEXT` |
| Cor de cada agente | `CLAUDE`, `ANTIGRAVITY`, `OLLAMA`, `SHELL` (ou `deck.agents.tab_color(kind)`) |
| Cor do projeto/aba em foco | `deck.active_color()` |

Ajustes: `with_opacity(c, a)`, `brighten(c, f)`, `rgba(...)`. Fontes: `theme::ui(px)`, `ui_semibold`,
`ui_bold`; texto de interface 12–13 px, títulos de seção 11 px semibold em MAIÚSCULAS e `MUTED`.

## Medidas

- Linha de lista: `ROW_H = 22`; título de seção: `SECTION_H = 24`; margem lateral do texto: 12 px.
- Árvore de agentes: logo no x 28 da linha, recuo de 14 px por nível (até 5 níveis), logo de 12 px, texto 6 px
  depois do logo.
- Botões de ícone no cabeçalho: 20×20, cantos de 4 px, hover `PANEL_2`, ícone `MUTED` → `TEXT` no hover.
- Menus e modais: `frame(p, r, 6–10, PANEL_2, BORDER)` + `shadow(...)`; itens de 26 px; posição com
  `menus::keep_on_screen` (não use `Area::constrain`, ele fecha o menu no clique — ver o comentário em
  `menus.rs`).
- Barra de título: `TITLEBAR_H = 36`. Barra lateral: 150–480 px (padrão 220). Barra direita: 280–640 (padrão 330).

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
cargo build -p commanddeck
COMMANDDECK_HOME=<scratch>/home COMMANDDECK_SMOKE=<scratch>/shot COMMANDDECK_DEBUG_UI=projects-demo \
  ./target/debug/commanddeck.exe        # salva <scratch>/shot/screen.png e sai
```

Outros estados em `COMMANDDECK_DEBUG_UI` (lista em `apply_debug_ui`, `app.rs`): `settings`, `newtab`, `map-demo`,
`usage-demo`, `git-demo`, `split-drag`... Abra a imagem e confira alinhamento, truncamento e contraste antes de
dizer que terminou; interação (clique, arrasto) não aparece na captura — diga ao usuário o que falta testar à mão.
