//! Segunda abertura do app: em vez de recusar ("outra janela já usa a pasta de dados"), a nova instância pede à viva que
//! mostre a janela (`POST /activate` na ponte) e sai em silêncio.
//!
//! A instância viva grava porta e token da ponte em `dados/.agentdeck.bridge` depois de subir a ponte e apaga o arquivo
//! ao encerrar. O arquivo sozinho não prova que há instância (um travamento deixa um velho): quem decide é a trava da
//! pasta ([`crate::lock_data_dir`]), e o arquivo velho é apagado por quem pega a trava.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Porta (linha 1) e token (linha 2) da ponte da instância viva.
const BRIDGE_FILE: &str = ".agentdeck.bridge";
/// Quanto esperar uma instância que ainda está subindo (trava pega, ponte ainda não publicada).
const STARTUP_WAIT: Duration = Duration::from_secs(6);
const POLL: Duration = Duration::from_millis(200);
const POST_TIMEOUT: Duration = Duration::from_secs(3);

fn bridge_file(data_dir: &Path) -> PathBuf {
    data_dir.join(BRIDGE_FILE)
}

/// Publica porta e token da ponte para a segunda abertura.
pub(crate) fn publish(data_dir: &Path, port: u16, token: &str) {
    if let Err(e) = std::fs::write(bridge_file(data_dir), format!("{port}\n{token}\n")) {
        log::warn!("não consegui gravar {}: {e}", bridge_file(data_dir).display());
    }
}

/// Apaga o arquivo da ponte (a instância encerra, ou pegou a trava e o que havia era de uma que travou).
pub(crate) fn forget(data_dir: &Path) {
    let _ = std::fs::remove_file(bridge_file(data_dir));
}

/// A ponte da instância viva (`None` enquanto ela ainda não a publicou).
fn read(data_dir: &Path) -> Option<(u16, String)> {
    let text = std::fs::read_to_string(bridge_file(data_dir)).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let token = lines.next()?.trim().to_owned();
    (!token.is_empty()).then_some((port, token))
}

/// Há uma instância viva usando `data_dir` (a trava está pega por outro handle).
fn running(data_dir: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(data_dir.join(crate::LOCK_FILE)) else { return false };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// `POST /activate` autenticado. `true` = a instância respondeu 200.
fn post_activate(port: u16, token: &str) -> bool {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(POST_TIMEOUT)).build().into();
    match agent.post(format!("http://127.0.0.1:{port}/activate")).header("x-agentdeck-token", token).send_empty() {
        Ok(resp) => resp.status().as_u16() == 200,
        Err(e) => {
            log::warn!("a instância em execução não respondeu ao /activate: {e}");
            false
        }
    }
}

/// O Windows só deixa trazer uma janela para a frente a quem recebeu o foco do usuário: este processo (que o usuário
/// acabou de abrir) passa o direito à instância viva, senão a janela só pisca na barra de tarefas.
#[cfg(windows)]
fn allow_foreground() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
    // SAFETY: chamada sem ponteiros.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

#[cfg(not(windows))]
fn allow_foreground() {}

/// Se outra instância do app usa `data_dir`, pede a ela que mostre a janela e devolve `true` (esta pode sair em silêncio).
/// `false`: não há instância viva, ou ela não respondeu (travada): siga o início normal, que mostra o erro da trava.
pub fn activate_running(data_dir: &Path) -> bool {
    if !running(data_dir) {
        return false;
    }
    allow_foreground();
    let give_up = Instant::now() + STARTUP_WAIT;
    loop {
        if let Some((port, token)) = read(data_dir) {
            return post_activate(port, &token);
        }
        // Sem arquivo: a instância ainda está subindo. Espera até ela publicar a ponte.
        if Instant::now() >= give_up || !running(data_dir) {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_file_round_trips_and_ignores_garbage() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert_eq!(read(tmp.path()), None);
        publish(tmp.path(), 4242, "abc123");
        assert_eq!(read(tmp.path()), Some((4242, "abc123".to_owned())));
        std::fs::write(bridge_file(tmp.path()), "não é porta\ntoken\n").expect("write");
        assert_eq!(read(tmp.path()), None);
        std::fs::write(bridge_file(tmp.path()), "80\n").expect("write");
        assert_eq!(read(tmp.path()), None, "sem token não vale");
        forget(tmp.path());
        assert!(!bridge_file(tmp.path()).exists());
    }

    #[test]
    fn without_a_live_instance_there_is_nothing_to_activate() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(!running(tmp.path()), "sem arquivo de trava");
        // Arquivo velho de uma instância que travou: a trava está livre, então não há a quem pedir.
        publish(tmp.path(), 1, "velho");
        let _free = crate::lock_data_dir(tmp.path()).expect("trava");
        drop(_free);
        assert!(!activate_running(tmp.path()));
    }

    #[test]
    fn a_held_lock_is_seen_as_a_running_instance() {
        let tmp = tempfile::tempdir().expect("tmp");
        let held = crate::lock_data_dir(tmp.path()).expect("trava");
        assert!(running(tmp.path()));
        drop(held);
        assert!(!running(tmp.path()));
    }
}
