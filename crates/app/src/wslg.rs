//! Ajustes para rodar sob o WSLg (o WSL 2 com interface gráfica).
//!
//! O WSLg mostra as janelas Linux por um Weston (Wayland) com um XWayland ao lado. O cliente Wayland do
//! Weston 9 quebra com quem desenha a própria decoração e redimensiona: a moldura sai errada, o cursor some
//! e o próprio Weston já caiu (`rust-windowing/winit#4306`, `microsoft/wslg#1008` e `#633`), enquanto o
//! XWayland é o caminho que os relatos dão como estável. Por isso, no WSLg o app abre pelo X11.
//!
//! A detecção exige o WSL **e** a pasta `/mnt/wslg` (que só existe com a interface gráfica): num WSL sem
//! WSLg ou num `DISPLAY` apontado para fora, nada muda.

/// As pistas do WSLg, separadas do sistema para poderem ser testadas.
///
/// `distro` é o `WSL_DISTRO_NAME`; `osrelease` é o conteúdo de `/proc/sys/kernel/osrelease` (no WSL ele cita
/// "microsoft"); `has_wslg_dir` é a existência de `/mnt/wslg`.
#[cfg(any(target_os = "linux", test))]
fn looks_like_wslg(distro: Option<&str>, osrelease: &str, has_wslg_dir: bool) -> bool {
    let wsl = distro.is_some_and(|d| !d.trim().is_empty()) || osrelease.to_ascii_lowercase().contains("microsoft");
    wsl && has_wslg_dir
}

/// `true` quando o app está rodando dentro do WSLg.
#[cfg(target_os = "linux")]
pub fn on_wslg() -> bool {
    let distro = std::env::var("WSL_DISTRO_NAME").ok();
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    looks_like_wslg(distro.as_deref(), &osrelease, std::path::Path::new("/mnt/wslg").is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wslg_needs_the_wsl_marks_and_the_graphical_folder() {
        assert!(looks_like_wslg(Some("Ubuntu-24.04"), "5.15.90.1-microsoft-standard-WSL2", true));
        // Sem WSL_DISTRO_NAME a versão do kernel ainda diz que é WSL (WSL 1 e 2).
        assert!(looks_like_wslg(None, "4.4.0-19041-Microsoft", true));
        // WSL sem interface gráfica, ou com o X11 apontado para fora: o backend é escolhido como sempre.
        assert!(!looks_like_wslg(Some("Ubuntu-24.04"), "5.15.90.1-microsoft-standard-WSL2", false));
        // Linux de verdade não tem as pistas, mesmo que a pasta exista por algum motivo.
        assert!(!looks_like_wslg(None, "6.8.0-45-generic", true));
        assert!(!looks_like_wslg(Some(""), "6.8.0-45-generic", true));
    }
}
