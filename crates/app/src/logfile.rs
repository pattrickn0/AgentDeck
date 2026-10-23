//! Log do app em disco: um app de janela no Windows não tem stderr, então o que o `log` escreve
//! (nível info por padrão) também vai para `<dados>/agentdeck.log`.
//!
//! O arquivo passa para `agentdeck.log.1` (substituindo o anterior) quando passa de `MAX_LOG`;
//! o teste é feito a cada escrita, não só ao abrir o app.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Tamanho a partir do qual o log é girado.
const MAX_LOG: u64 = 2 * 1024 * 1024;

/// Destino do `env_logger`: o stderr (quando houver) e o arquivo.
pub struct LogFile {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max: u64,
}

impl LogFile {
    /// Abre (ou cria) `<dir>/agentdeck.log`. Sem conseguir abrir, só o stderr recebe o log.
    pub fn open(dir: &Path) -> Self {
        Self::with_max(dir.join("agentdeck.log"), MAX_LOG)
    }

    fn with_max(path: PathBuf, max: u64) -> Self {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut log = Self { path, file: None, size: 0, max };
        log.reopen();
        log
    }

    /// Abre o arquivo para acrescentar, girando antes se já passou do tamanho.
    fn reopen(&mut self) {
        self.file = None;
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > self.max) {
            let old = self.path.with_extension("log.1");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&self.path, &old);
        }
        self.file = OpenOptions::new().create(true).append(true).open(&self.path).ok();
        self.size = self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
    }
}

impl Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Sem console (app de janela) o stderr falha ou some: o arquivo é o que importa.
        let _ = std::io::stderr().write_all(buf);
        if self.size > self.max {
            self.reopen();
        }
        if let Some(file) = &mut self.file {
            if file.write_all(buf).is_ok() {
                self.size += buf.len() as u64;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &mut self.file {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_appends_to_the_file_and_rotates_when_it_grows_past_the_limit() {
        let dir = std::env::temp_dir().join(format!("cd-logfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = LogFile::with_max(dir.join("agentdeck.log"), 100);
        log.write_all(b"primeira\n").unwrap();
        log.write_all(&[b'x'; 120]).unwrap();
        // Passou do limite: a próxima escrita começa um arquivo novo e o anterior vira .log.1.
        log.write_all(b"depois\n").unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("agentdeck.log")).unwrap(), "depois\n");
        let old = std::fs::read_to_string(dir.join("agentdeck.log.1")).unwrap();
        assert!(old.starts_with("primeira\n") && old.len() == 9 + 120);
        // Reabrir o app continua o arquivo que já existe.
        drop(log);
        let mut again = LogFile::with_max(dir.join("agentdeck.log"), 100);
        again.write_all(b"mais\n").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("agentdeck.log")).unwrap(), "depois\nmais\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
