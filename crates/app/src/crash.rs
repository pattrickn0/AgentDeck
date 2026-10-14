//! Registro de falhas: o app de release não tem console, então panics e quedas fatais sumiam
//! sem deixar rastro. Aqui elas viram linhas em `<dados>/crash.log`.
//!
//! - Panics: mensagem + pilha de chamadas.
//! - Windows: exceções fatais (corrupção de heap `0xC0000374`, estouro de buffer `0xC0000409`,
//!   violação de acesso `0xC0000005`) gravam o código, o endereço e a pilha. A primeira camada
//!   não aloca memória (o heap pode estar corrompido); a segunda tenta resolver os símbolos.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Arquivo aberto na inicialização (não dá para abrir arquivos com o heap corrompido).
static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Instala os ganchos. `dir` é a pasta de dados do app.
pub fn install(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, RtlCaptureStackBackTrace, EXCEPTION_POINTERS};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    const HEAP_CORRUPTION: u32 = 0xC000_0374;
    const FAST_FAIL: u32 = 0xC000_0409;
    const ACCESS_VIOLATION: u32 = 0xC000_0005;
    const CONTINUE_SEARCH: i32 = 0;

    static LOGGED: AtomicBool = AtomicBool::new(false);
    /// Violações de acesso tratadas por terceiros são raras, mas não devem lotar o log.
    static ACCESS_VIOLATIONS: AtomicUsize = AtomicUsize::new(0);

    pub fn install() {
        // SAFETY: o gancho só lê o registro de exceção que o sistema passa e devolve CONTINUE_SEARCH.
        unsafe {
            AddVectoredExceptionHandler(1, Some(handler));
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

    unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: o sistema garante ponteiros válidos durante a chamada.
        let (code, address) = unsafe {
            let record = (*info).ExceptionRecord;
            ((*record).ExceptionCode as u32, (*record).ExceptionAddress as usize)
        };
        let fatal = matches!(code, HEAP_CORRUPTION | FAST_FAIL);
        // Violações de acesso podem ser tratadas por quem as causou: registra, mas sem travar o gancho.
        if !fatal && code != ACCESS_VIOLATION {
            return CONTINUE_SEARCH;
        }
        if fatal && LOGGED.swap(true, Ordering::SeqCst) {
            return CONTINUE_SEARCH;
        }
        if !fatal && ACCESS_VIOLATIONS.fetch_add(1, Ordering::Relaxed) >= 3 {
            return CONTINUE_SEARCH;
        }

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
        if fatal {
            let thread = std::thread::current();
            super::append(&format!("thread {:?}\n{}", thread.name().unwrap_or("?"), std::backtrace::Backtrace::force_capture()));
        }
        CONTINUE_SEARCH
    }
}
