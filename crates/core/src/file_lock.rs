//! Quem segura uma pasta que não sai (a worktree de uma tarefa ao validar ou descartar): os arquivos que ficaram e,
//! no Windows, os processos que os têm abertos (Restart Manager, o mesmo que o instalador do Windows usa para dizer
//! "feche estes programas"). Serve só para o log e a mensagem: nada aqui fecha processo.

use std::path::{Path, PathBuf};

/// Quantos arquivos que ficaram entram na mensagem (o log leva o mesmo).
const MAX_FILES: usize = 10;
/// Quantas entradas a busca percorre, no máximo (uma worktree com `target/` pode ter dezenas de milhares).
const MAX_WALK: usize = 20_000;

/// O que ficou de uma pasta que não saiu.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Leftovers {
    /// Até [`MAX_FILES`] arquivos que ainda existem.
    pub files: Vec<PathBuf>,
    /// Quantos arquivos ficaram ao todo (até onde a busca foi).
    pub total: usize,
    /// Os processos que têm algum desses arquivos aberto ("agentdeck.exe (pid 123)"); vazio fora do Windows.
    pub holders: Vec<String>,
}

impl Leftovers {
    /// Uma frase para o aviso da tarefa: quais arquivos ficaram e quem os segura, ou a pista de que só pastas
    /// vazias ficaram (um processo com o diretório atual dentro dela, como o terminal do agente).
    pub fn describe(&self, dir: &Path) -> String {
        if self.total == 0 {
            return "só pastas vazias ficaram: algum processo está com o diretório atual dentro dela (um terminal, o \
                    agente, o editor)"
                .to_owned();
        }
        let shown: Vec<String> = self.files.iter().map(|f| f.strip_prefix(dir).unwrap_or(f).to_string_lossy().replace('\\', "/")).collect();
        let more = if self.total > shown.len() { format!(" e mais {}", self.total - shown.len()) } else { String::new() };
        let holders = if self.holders.is_empty() { String::new() } else { format!("; aberto por: {}", self.holders.join(", ")) };
        format!("{} arquivo(s) ficaram: {}{more}{holders}", self.total, shown.join(", "))
    }
}

/// Os arquivos que ainda existem em `dir` e quem os tem aberto.
pub(crate) fn leftovers(dir: &Path) -> Leftovers {
    let mut files = Vec::new();
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    let mut walked = 0;
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            walked += 1;
            if walked > MAX_WALK {
                break;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(_) => {
                    total += 1;
                    if files.len() < MAX_FILES {
                        files.push(e.path());
                    }
                }
                Err(_) => {}
            }
        }
    }
    let holders = holders(&files);
    Leftovers { files, total, holders }
}

/// Os processos que têm algum de `files` aberto, pelo Restart Manager.
#[cfg(windows)]
fn holders(files: &[PathBuf]) -> Vec<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY, RM_PROCESS_INFO,
    };

    if files.is_empty() {
        return Vec::new();
    }
    let wide: Vec<Vec<u16>> = files.iter().map(|f| f.as_os_str().encode_wide().chain(std::iter::once(0)).collect()).collect();
    let names: Vec<*const u16> = wide.iter().map(|w| w.as_ptr()).collect();
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: `key` tem o tamanho que a API pede (CCH_RM_SESSION_KEY + 1); `session` só é usado se ela der certo.
    if unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: `names` aponta para textos terminados em NUL que vivem em `wide` até o fim da função.
    let registered = unsafe { RmRegisterResources(session, names.len() as u32, names.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) };
    if registered == ERROR_SUCCESS {
        let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
        let (mut needed, mut count, mut reasons) = (0u32, 0u32, 0u32);
        // Primeiro pergunta quantos são; depois lê (pode crescer no meio: tenta de novo algumas vezes).
        for _ in 0..3 {
            infos.resize_with(needed as usize, Default::default);
            count = infos.len() as u32;
            // SAFETY: `infos` tem `count` posições; a API escreve no máximo isso e diz em `needed` quantas precisa.
            let r = unsafe { RmGetList(session, &mut needed, &mut count, infos.as_mut_ptr(), &mut reasons) };
            if r == ERROR_SUCCESS {
                infos.truncate(count as usize);
                break;
            }
            if r != ERROR_MORE_DATA {
                infos.clear();
                break;
            }
        }
        for info in infos.iter().take(count as usize) {
            let len = info.strAppName.iter().position(|&c| c == 0).unwrap_or(info.strAppName.len());
            let name = String::from_utf16_lossy(&info.strAppName[..len]);
            out.push(format!("{name} (pid {})", info.Process.dwProcessId));
        }
    }
    // SAFETY: fecha a sessão aberta acima, uma vez.
    unsafe { RmEndSession(session) };
    out.sort();
    out.dedup();
    out
}

#[cfg(not(windows))]
fn holders(_files: &[PathBuf]) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_left_behind_are_listed_and_described() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("target/debug")).expect("mkdir");
        std::fs::create_dir_all(dir.join("vazia")).expect("mkdir");
        for i in 0..12 {
            std::fs::write(dir.join("target/debug").join(format!("f{i}.rlib")), b"x").expect("write");
        }
        let left = leftovers(dir);
        assert_eq!((left.total, left.files.len()), (12, MAX_FILES));
        let text = left.describe(dir);
        assert!(text.starts_with("12 arquivo(s) ficaram: target/debug/f"), "{text}");
        assert!(text.contains(" e mais 2"), "{text}");
        // Só pastas vazias: a pista do diretório atual.
        let empty = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(empty.path().join("a/b")).expect("mkdir");
        assert!(leftovers(empty.path()).describe(empty.path()).starts_with("só pastas vazias ficaram"));
    }

    #[cfg(windows)]
    #[test]
    fn a_file_held_open_names_this_process() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("preso.txt");
        std::fs::write(&file, b"x").expect("write");
        let _open = std::fs::File::open(&file).expect("open");
        let held = holders(&[file]);
        let me = format!("(pid {})", std::process::id());
        assert!(held.iter().any(|h| h.ends_with(&me)), "{held:?}");
    }
}
