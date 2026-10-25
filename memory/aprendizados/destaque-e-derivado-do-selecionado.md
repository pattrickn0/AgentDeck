---
titulo: No Destaque, a janela grande é derivada da aba ativa (sem estado) e a organização tem dois campos na Config
resumo: Nada guarda "quem está em destaque"; trocar de aba troca as janelas, e o PTY só muda de tamanho com o tamanho parado. `organization` manda, `splitEnabled` só serve de herança.
touches: [crates/app/src/split.rs, crates/app/src/deck/**, crates/app/src/ui/panes.rs, crates/app/src/ui/sidebar.rs, crates/core/src/config.rs]
---
O modo **Destaque** (`Organization::Highlight`) não tem estado próprio além da proporção (`Config::highlight_ratio`, global,
não por projeto): `Deck::plan_panes` põe o grupo da **aba ativa** primeiro (`split::highlight_order`), e `assign` +
`highlight_layout` fazem o resto. Os outros grupos ficam na ordem de `group_order`, que não muda. Por isso clicar numa janela
pequena, ativar uma aba pela barra lateral ou por atalho e o botão "Trazer para o destaque" são todos o mesmo `Deck::activate`;
e a alça de trocar janelas de lugar não existe no Destaque (a ordem das pequenas é a de `group_order`, o destaque não é uma posição).

**Por quê:** guardar "o destaque" separado da aba ativa dá dois estados que se desencontram (ativar uma aba por atalho, por
tarefa ou por `ask-agent` precisaria atualizar os dois). Trocar o destaque muda o tamanho das janelas uma vez; o PTY segue a
regra de sempre (`fit_tab` + tamanho parado, ver `claude-code-sem-esc3j-no-windows.md`), com 600 ms numa troca de aba no
Destaque em vez dos 150 ms do arrasto, e o próximo redimensionamento da aba só sai depois que ela termina de reimprimir
(300 ms de saída parada, teto de 3 s). O teste `the_highlight_follows_the_active_tab_and_the_pty_resizes_once_per_switch`
(`tasks_flow_tests.rs`) confere que quadros parados não pedem redimensionamento. Não anime o tamanho das janelas: cada quadro
da animação seria um tamanho novo para o PTY.

**Organização na Config:** `organization` (`Option<Organization>`: `tabs`, `split`, `highlight`) é o campo que vale; o antigo
`splitEnabled` continua sendo lido (arquivo sem `organization` → `Config::organization()` deduz dele) e gravado junto
(`set_organization`), para uma versão anterior entender o arquivo. Nunca leia `split_enabled` direto: use `organization()`.

**Realce (Dividida e Destaque):** o "62%" das outras janelas é uma película da cor `bg` a 38% por cima da janela (não há
opacidade de grupo no egui), animada por `animate_bool_with_time`; o mouse em cima a tira. O halo de 3 px é um traço de fora
(`StrokeKind::Outside`) pintado com o `ui.painter()` inteiro, não com o `painter_at(rect)` da janela, senão o clip o corta.
