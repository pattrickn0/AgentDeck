---
titulo: O EguiGlow não preenche ViewportInfo (maximized sempre None)
resumo: Sem egui_winit::update_viewport_info a cada quadro, viewport().maximized é None; o app preenche em App::redraw.
touches: [crates/app/src/app.rs, crates/app/src/ui/titlebar.rs]
---
Usamos winit + `egui_glow::EguiGlow` sem eframe. Quem chama `egui_winit::update_viewport_info` é o eframe; o `EguiGlow::run`
só tem um `viewport_info` privado, que nunca chega ao `RawInput`. Resultado: `ctx.input(|i| i.viewport().maximized)` (e
`minimized`, `focused`, `inner_rect`...) é sempre `None`. Foi assim que a janela maximizada não voltava ao modo janela: o
botão mandava sempre `Maximized(true)`, o glifo não mudava e `resize_edges` ficava ligado.

**Por quê:** nada avisa; `unwrap_or(false)` esconde o `None`.
**Como aplicar:** `App::redraw` chama `update_viewport_info(&mut viewport_info, ..)` e grava o resultado em
`egui.egui_winit.egui_input_mut().viewports[ROOT]` antes de `egui.run`. Qualquer UI que dependa de `ViewportInfo` precisa
desse passo. Ao gravar o tamanho da janela, ignore o maximizado (`size_to_persist`): senão a janela "normal" reabre do
tamanho da tela.
