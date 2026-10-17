//! Registro de falhas: o app de release não tem console, então panics e quedas fatais sumiam
//! sem deixar rastro. Aqui elas viram linhas em `<dados>/crash.log`.
//!
//! - Panics: mensagem + pilha de chamadas.
//! - Windows: só o que derruba o app grava o código, o endereço e a pilha: corrupção de heap
//!   (`0xC0000374`) e estouro de buffer (`0xC0000409`), que nunca são tratados, e qualquer exceção
//!   que ninguém tratou (filtro de última chance). Violações de acesso de primeira chance que o
//!   driver de vídeo trata sozinho não entram mais. A primeira camada não aloca memória (o heap
//!   pode estar corrompido); a segunda tenta resolver os símbolos.
//! - O arquivo passa para `crash.log.1` (substituindo o anterior) quando passa de `MAX_LOG`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Arquivo aberto na inicialização (não dá para abrir arquivos com o heap corrompido).
static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Tamanho a partir do qual o log é girado ao abrir o app.
const MAX_LOG: u64 = 256 * 1024;

/// Gira o log grande: `crash.log` vira `crash.log.1` (o anterior sai) e o próximo começa vazio.
fn rotate(path: &Path, max: u64) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > max) {
        let old = path.with_extension("log.1");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(path, &old);
    }
}

/// Instala os ganchos. `dir` é a pasta de dados do app.
pub fn install(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    rotate(&dir.join("crash.log"), MAX_LOG);
    let Ok(file) = OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) else { return };
    let _ = LOG.set(Mutex::new(file));

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        append(&format!("PANIC na thread {:?}: {info}\n{}", thread.name().unwrap_or("?"), std::backtrace::Backtrace::force_capture()));
        default_hook(info);
    }));

    #[cfg(windows)]
    windows::install();
    debug_trigger();
}

/// SOMENTE DEPURAÇÃO: `COMMANDDECK_DEBUG_CRASH=panic|heap` derruba o app 3 s depois de abrir, para
/// verificar que o registro de falhas funciona.
fn debug_trigger() {
    let Ok(kind) = std::env::var("COMMANDDECK_DEBUG_CRASH") else { return };
    let _ = std::thread::Builder::new().name("crash-test".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        match kind.as_str() {
            "panic" => panic!("falha de teste (COMMANDDECK_DEBUG_CRASH=panic)"),
            "heap" => {
                // SAFETY: liberação dupla proposital para corromper o heap; só em depuração.
                unsafe {
                    let block = Box::into_raw(Box::new([0u8; 64]));
                    drop(Box::from_raw(block));
                    drop(Box::from_raw(block));
                }
            }
            _ => {}
        }
    });
}

/// Acrescenta uma entrada com a hora local.
fn append(text: &str) {
    let Some(log) = LOG.get() else { return };
    let Ok(mut file) = log.lock() else { return };
    let _ = writeln!(file, "[{}] v{} {text}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), env!("CARGO_PKG_VERSION"));
    let _ = file.flush();
}

#[cfg(windows)]
mod windows {
    use std::fmt::Write as _;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, RtlCaptureStackBackTrace, SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
        LPTOP_LEVEL_EXCEPTION_FILTER,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    const HEAP_CORRUPTION: u32 = 0xC000_0374;
    const FAST_FAIL: u32 = 0xC000_0409;
    const CONTINUE_SEARCH: i32 = 0;

    static LOGGED: AtomicBool = AtomicBool::new(false);
    /// Filtro de última chance que havia antes do nosso (chamado depois de registrar).
    static PREVIOUS: OnceLock<LPTOP_LEVEL_EXCEPTION_FILTER> = OnceLock::new();

    pub fn install() {
        // SAFETY: os ganchos só leem o registro de exceção que o sistema passa e devolvem
        // CONTINUE_SEARCH (ou o que o filtro anterior decidir).
        unsafe {
            AddVectoredExceptionHandler(1, Some(handler));
            let _ = PREVIOUS.set(SetUnhandledExceptionFilter(Some(unhandled)));
        }
    }

    /// Primeira chance: só os códigos que nunca são tratados (o processo vai cair). Uma violação
    /// de acesso aqui pode ser tratada por quem a causou (o driver de OpenGL faz isso), então ela
    /// só é registrada se chegar sem tratamento ao filtro de última chance.
    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: o sistema garante ponteiros válidos durante a chamada.
        let code = unsafe { (*(*info).ExceptionRecord).ExceptionCode as u32 };
        if matches!(code, HEAP_CORRUPTION | FAST_FAIL) {
            // SAFETY: repassa o mesmo ponteiro, válido durante esta chamada.
            unsafe { log_fatal(info) };
        }
        CONTINUE_SEARCH
    }

    /// Última chance: ninguém tratou a exceção e o processo vai cair.
    unsafe extern "system" fn unhandled(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: o sistema garante ponteiros válidos durante a chamada.
        unsafe { log_fatal(info) };
        match PREVIOUS.get().copied().flatten() {
            // SAFETY: filtro instalado antes do nosso, chamado com o mesmo argumento.
            Some(prev) => unsafe { prev(info) },
            None => CONTINUE_SEARCH,
        }
    }

    /// Buffer fixo em pilha: formata sem alocar.
    struct Stack {
        data: [u8; 2048],
        len: usize,
    }

    impl std::fmt::Write for Stack {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let n = s.len().min(self.data.len() - self.len);
            self.data[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
            self.len += n;
            Ok(())
        }
    }

    /// Grava a exceção que vai derrubar o app (uma vez só).
    ///
    /// # Safety
    /// `info` precisa ser o ponteiro válido que o sistema passou ao gancho.
    unsafe fn log_fatal(info: *const EXCEPTION_POINTERS) {
        if LOGGED.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: o sistema garante ponteiros válidos durante a chamada.
        let (code, address) = unsafe {
            let record = (*info).ExceptionRecord;
            ((*record).ExceptionCode as u32, (*record).ExceptionAddress as usize)
        };

        // Camada 1: sem alocação. Endereços relativos ao módulo, resolvíveis com o .pdb do mesmo build.
        let mut frames = [std::ptr::null_mut::<std::ffi::c_void>(); 48];
        // SAFETY: `frames` comporta 48 entradas; o hash é opcional.
        let count = unsafe { RtlCaptureStackBackTrace(0, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) } as usize;
        // SAFETY: nulo = módulo do executável.
        let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        let mut out = Stack { data: [0; 2048], len: 0 };
        let _ = write!(out, "EXCECAO FATAL 0x{code:08X} em 0x{address:X} (base do exe 0x{base:X}); pilha (offset no exe):");
        for frame in &frames[..count.min(frames.len())] {
            let ip = *frame as usize;
            // Só o que cai dentro da imagem do exe vira offset; o resto (ntdll, drivers) fica absoluto.
            if ip >= base && ip - base < 0x2000_0000 {
                let _ = write!(out, " +0x{:X}", ip - base);
            } else {
                let _ = write!(out, " 0x{ip:X}");
            }
        }
        let _ = out.write_char('\n');
        if let Some(log) = super::LOG.get() {
            if let Ok(mut file) = log.try_lock() {
                let _ = file.write_all(&out.data[..out.len]);
                let _ = file.flush();
            }
        }

        // Camada 2: pilha resolvida (aloca; pode falhar se o heap estiver muito danificado).
        let thread = std::thread::current();
        super::append(&format!("thread {:?}\n{}", thread.name().unwrap_or("?"), std::backtrace::Backtrace::force_capture()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_log_is_rotated_and_a_small_one_is_kept() {
        let dir = std::env::temp_dir().join(format!("cd-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("crash.log");
        std::fs::write(&log, "pequeno").unwrap();
        rotate(&log, 100);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "pequeno");
        std::fs::write(&log, "x".repeat(101)).unwrap();
        std::fs::write(dir.join("crash.log.1"), "antigo").unwrap();
        rotate(&log, 100);
        assert!(!log.exists());
        assert_eq!(std::fs::read_to_string(dir.join("crash.log.1")).unwrap().len(), 101);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
