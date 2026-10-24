---
titulo: No WSLg a janela tem de abrir pelo X11, não pelo Wayland
resumo: O Weston 9 do WSLg quebra clientes Wayland (e chega a cair); o app força o X11 quando `/mnt/wslg` existe.
touches: [crates/app/src/main.rs, crates/app/src/wslg.rs]
---
Sob o WSLg o desktop é um **Weston 9** (Wayland) com um XWayland ao lado. O cliente Wayland do Weston 9 quebra
com janelas que desenham a própria decoração e redimensionam — moldura errada, cursor sumido, e o próprio
Weston já segfaultou (`rust-windowing/winit#4306`, `microsoft/wslg#1008` e `#633`). O XWayland é o caminho que
os relatos dão como estável; o app agora abre por ele quando detecta o WSLg.

**Por quê:** o `winit` escolhe Wayland sempre que `WAYLAND_DISPLAY` existe (é o caso no WSLg), e o backend
forçado por `EventLoopBuilderExtX11::with_x11()` tem precedência sobre essa escolha e sobre o
`WINIT_UNIX_BACKEND`.

**Como aplicar:** a detecção (`crates/app/src/wslg.rs`) exige o WSL (`WSL_DISTRO_NAME` ou "microsoft" no
`/proc/sys/kernel/osrelease`) **e** a pasta `/mnt/wslg` (que só existe com a interface gráfica); a função pura
tem teste. Um `WINIT_UNIX_BACKEND` explícito continua mandando, para quem quiser experimentar o Wayland de lá.
Duas limitações do WSLg que o app não resolve: no X11 a escala vem do XSettings (`Xft/DPI`, que o WSLg não
publica), então é sempre 1 e a interface fica pequena em tela de alta densidade — e o `winit` 0.30 **não tem
mais** o `WINIT_X11_SCALE_FACTOR` (foi removido; não documente esse contorno como se funcionasse). Janela sem
decoração maximizada também já apareceu deslocada em 32 px. Por isso, para o Windows, o caminho é o app nativo.
