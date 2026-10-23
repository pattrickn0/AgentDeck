---
titulo: No Linux o AgentDeck não usa GTK3, e os diálogos dependem do portal
resumo: O rfd 0.17 usa xdg-portal (sem gtk-sys); não peça gtk3-devel — mas sem xdg-desktop-portal o diálogo não abre.
touches: [README.md, CONTRIBUTING.md, .github/workflows/ci.yml, crates/app/Cargo.toml]
---
A lista de dependências do Linux citava `libgtk-3-dev` (Ubuntu) e `gtk3-devel` (Fedora), que **não são usados**: o
`Cargo.lock` não tem `gtk-sys`/`glib-sys`, e o `rfd 0.17` tem `default = ["xdg-portal", "wayland"]` (a feature `gtk3`
existe, mas não está ligada). O `ureq` usa rustls, então também não há OpenSSL no build.

O que o Linux realmente pede além do compilador e da ALSA: `xdg-utils` (`xdg-open`, para abrir pastas/links) e
`xdg-desktop-portal` com um backend (`xdg-desktop-portal-gtk` ou `-kde`) para os diálogos de arquivo. Sem o portal o
diálogo não abre e o app não avisa.

**Por quê:** a documentação do rfd fala de GTK3 e é fácil copiar isso para o README; o resultado é uma dependência de
build grande que não muda nada no binário. Do outro lado, o portal é invisível no `Cargo.toml` (é serviço D-Bus) e
esqueceram dele.

**Como aplicar:** antes de acrescentar biblioteca de sistema, confira `Cargo.lock` (o crate está lá?) e as features
ligadas. Ao testar diálogo de arquivo num ambiente mínimo, confira `systemctl --user status xdg-desktop-portal`. Se um
dia o portal virar problema em ambiente sem D-Bus, a alternativa é ligar a feature `gtk3` do rfd e voltar a pedir o
GTK — decisão de produto, não “conserto” silencioso.
