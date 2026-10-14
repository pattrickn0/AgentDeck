//! Transcrição local com whisper.cpp (equivalente ao bloco "Transcrição local" do main.js).
//!
//! Motores em ordem de preferência: CUDA (GPU NVIDIA), Vulkan (qualquer GPU) e CPU. No
//! Windows os binários ficam em `whisper/bin-*/[Release/]whisper-cli.exe`; no Linux, em
//! `whisper/bin-*-linux/whisper-cli` (ou o `whisper-cli` do PATH). O modelo e o motor de CPU
//! (Windows e Linux) e o CUDA (Windows) são baixados sob demanda, com SHA-256 fixado neste
//! arquivo; CUDA/Vulkan no Linux são compilados com `scripts/build-whisper-linux.sh`.
//! O WAV temporário é SEMPRE apagado após a transcrição.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crate::download::{download_file, DownloadOpts};
use crate::events::{CoreEvent, DownloadProgress, DownloadState, FeedEntry};
use crate::process::{run_captured, Exit};
use crate::{Core, Transcription};

pub const MODEL_FILE: &str = "ggml-large-v3-turbo.bin";
const RELEASE: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/b5454";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin";
/// SHA-256 do modelo: bate com o ETag LFS do Hugging Face.
const MODEL_SHA256: &str = "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69";
/// Vocabulário que orienta o reconhecimento de nomes próprios.
pub const WHISPER_PROMPT: &str = "CommandDeck Terminal, Claude, Antigravity, agy, Claude Code, Ollama, Gemini, Aider, Goose, terminal, CLI, assistente Claude, Google Antigravity, Vulkan, NVIDIA, AMD.";
pub const UNAVAILABLE: &str = "indisponível";
const NOT_READY: &str = "a transcrição local ainda não está pronta (veja a Conversa integrada)";
const NO_SPEECH: &str = "nenhuma fala detectada";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Cuda,
    Vulkan,
    Cpu,
}

/// Pacote pré-compilado de um motor (release b5454 do whisper.cpp), com o SHA-256 fixado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Archive {
    file: &'static str,
    tar_gz: bool,
    /// Componentes iniciais removidos ao extrair (os .tar.gz de Linux trazem uma pasta raiz).
    strip: usize,
    sha256: &'static str,
}

/// Hashes do campo "digest" do release no GitHub; os de CPU (Windows e Linux) foram conferidos
/// baixando os arquivos, o do pacote CUDA (685 MB) vem só do digest.
fn engine_archive(engine: Engine, os: &str, arch: &str) -> Option<Archive> {
    let zip = |file, sha256| Some(Archive { file, tar_gz: false, strip: 0, sha256 });
    let tgz = |file, sha256| Some(Archive { file, tar_gz: true, strip: 1, sha256 });
    match (os, arch, engine) {
        ("windows", "x86_64", Engine::Cpu) => {
            zip("whisper-bin-x64.zip", "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb")
        }
        ("windows", "x86_64", Engine::Cuda) => {
            zip("whisper-bin-win-cuda-12.4.0-x64.zip", "afef0b881c500958921c3f5523b50e59ee2ec9b6f5cbd25b324c51ed308a957a")
        }
        ("linux", "x86_64", Engine::Cpu) => {
            tgz("whisper-bin-ubuntu-x64.tar.gz", "a72becf15d7917f990f6313867a52638b82b7f9ef237fb0c980dac56a135781c")
        }
        ("linux", "aarch64", Engine::Cpu) => {
            tgz("whisper-bin-ubuntu-arm64.tar.gz", "6b95ebfc60447df48e70ef00a73bdc3f41679ed2d01ef465827206a2ff325149")
        }
        _ => None,
    }
}

impl Engine {
    /// Pasta do motor em `whisper/`. No Linux usa o sufixo `-linux`, para não se misturar aos
    /// binários de Windows versionados em `bin-cpu/` e `bin-vulkan/`.
    pub fn dir(self) -> String {
        if cfg!(windows) {
            format!("bin-{}", self.id())
        } else {
            format!("bin-{}-{}", self.id(), std::env::consts::OS)
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Engine::Cuda => "GPU NVIDIA (CUDA)",
            Engine::Vulkan => "GPU (Vulkan)",
            Engine::Cpu => "CPU",
        }
    }

    /// Pacote pré-compilado para este sistema e arquitetura, se existir.
    fn archive(self) -> Option<Archive> {
        engine_archive(self, std::env::consts::OS, std::env::consts::ARCH)
    }

    fn id(self) -> &'static str {
        match self {
            Engine::Cuda => "cuda",
            Engine::Vulkan => "vulkan",
            Engine::Cpu => "cpu",
        }
    }
}

/// Estado do subsistema (GPUs detectadas e download em andamento).
#[derive(Default)]
pub(crate) struct WhisperState {
    gpus: OnceLock<Vec<String>>,
    ensuring: AtomicBool,
}

impl WhisperState {
    /// Nomes das GPUs (cache). Windows: `Get-CimInstance Win32_VideoController`;
    /// Linux: `lspci -mm` e, sem NVIDIA nele, `nvidia-smi -L`.
    fn gpus(&self) -> &[String] {
        self.gpus.get_or_init(detect_gpus)
    }

    fn has_nvidia(&self) -> bool {
        self.gpus().iter().any(|n| n.to_lowercase().contains("nvidia"))
    }
}

fn detect_gpus() -> Vec<String> {
    if cfg!(windows) {
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-Command", "(Get-CimInstance Win32_VideoController).Name"]);
        let Ok(out) = run_captured(cmd, None, Duration::from_secs(20), None, None) else { return Vec::new() };
        out.stdout
            .lines()
            .map(str::trim)
            .filter(|s| {
                let l = s.to_lowercase();
                !s.is_empty() && !l.contains("basic display") && !l.contains("remote display")
            })
            .map(str::to_owned)
            .collect()
    } else if cfg!(target_os = "linux") {
        linux_gpus()
    } else {
        Vec::new()
    }
}

fn linux_gpus() -> Vec<String> {
    let run = |program: &str, args: &[&str]| {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        run_captured(cmd, None, Duration::from_secs(5), None, None).ok().map(|o| o.stdout)
    };
    let mut names = run("lspci", &["-mm"]).map(|o| parse_lspci(&o)).unwrap_or_default();
    if !names.iter().any(|n| n.to_lowercase().contains("nvidia")) {
        names.extend(run("nvidia-smi", &["-L"]).map(|o| parse_nvidia_smi(&o)).unwrap_or_default());
    }
    names
}

/// `lspci -mm`: `00:02.0 "VGA compatible controller" "Intel Corporation" "UHD Graphics 620" …`
fn parse_lspci(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('"').collect();
            let class = parts.get(1)?.to_lowercase();
            if !(class.contains("vga") || class.contains("3d controller") || class.contains("display controller")) {
                return None;
            }
            Some(format!("{} {}", parts.get(3)?, parts.get(5)?).trim().to_owned())
        })
        .collect()
}

/// `nvidia-smi -L`: `GPU 0: NVIDIA GeForce RTX 3060 (UUID: GPU-…)`
fn parse_nvidia_smi(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("GPU ")?;
            let (_, after) = rest.split_once(':')?;
            Some(after.split(" (UUID").next()?.trim().to_owned()).filter(|n| !n.is_empty())
        })
        .collect()
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    }
}

/// Executável dentro de `whisper/bin-<motor>/` (raiz, `Release/` ou `bin/`).
fn local_cli(whisper_dir: &Path, engine: Engine) -> Option<PathBuf> {
    let subs: &[&str] = if cfg!(windows) { &["", "Release"] } else { &["", "Release", "bin"] };
    subs.iter().map(|s| whisper_dir.join(engine.dir()).join(s).join(exe_name())).find(|p| p.is_file())
}

/// Executável do motor, se presente. Fora do Windows um `whisper-cli` do PATH vale como CPU.
pub fn whisper_cli(whisper_dir: &Path, engine: Engine) -> Option<PathBuf> {
    let found = local_cli(whisper_dir, engine);
    if found.is_none() && !cfg!(windows) && engine == Engine::Cpu {
        return which::which("whisper-cli").ok();
    }
    found
}

fn model_path(whisper_dir: &Path) -> PathBuf {
    whisper_dir.join("models").join(MODEL_FILE)
}

/// Ordem de preferência dos motores disponíveis (vazio sem o modelo).
fn engines(core: &Core) -> Vec<Engine> {
    let dir = &core.paths().whisper_dir;
    if !model_path(dir).is_file() {
        return Vec::new();
    }
    let st = &core.inner.whisper;
    let mut order = Vec::new();
    if st.has_nvidia() {
        order.push(Engine::Cuda);
    }
    // Fora do Windows não detectamos GPUs Vulkan: tentamos se o binário existir.
    if !st.gpus().is_empty() || !cfg!(windows) {
        order.push(Engine::Vulkan);
    }
    order.push(Engine::Cpu);
    order.retain(|e| whisper_cli(dir, *e).is_some());
    order
}

/// Corrige variações fonéticas comuns de nomes de agentes.
pub fn normalize_transcription(text: &str) -> String {
    static RULES: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            // Claude com contexto de comando ("Cláudio Code" → "Claude Code").
            (r"(?i)\b(?:Cl[áa]udio|Cl[áa]ud[eê]|Cl[áa]udi[ao])\b(\s+(?:Code|CLI|agente|assistente))", "Claude${1}"),
            (r"(?i)\bCloud\s+(Code|CLI|agente|assistente)\b", "Claude ${1}"),
            (r"(?i)\b(com o|para o|pro|ao|chama o|pergunta pro|pergunta ao)\s+Cloud\b", "${1} Claude"),
            (r"(?i)\b(com o|para o|pro|chama o|pergunta pro|pergunta ao)\s+Cl[áa]udio\b", "${1} Claude"),
            // Antigravity.
            (
                r"(?i)\b(?:Anti-?crafty|Anti-?craft|Anti-?graft(?:ing)?|Anti-?graph|Anti-gravity|Anti\s+gravity|Antigraviti)\b",
                "Antigravity",
            ),
        ]
        .into_iter()
        .filter_map(|(re, rep)| regex::Regex::new(re).ok().map(|r| (r, rep)))
        .collect()
    });
    let mut out = text.to_owned();
    for (re, rep) in rules {
        out = re.replace_all(&out, *rep).into_owned();
    }
    out
}

/// WAV PCM 16 bits, mono, 16 kHz (amostras f32 em -1..1).
pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    const RATE: u32 = 16_000;
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes()); // tamanho do bloco fmt
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes por segundo
    b.extend_from_slice(&2u16.to_le_bytes()); // alinhamento do bloco
    b.extend_from_slice(&16u16.to_le_bytes()); // bits por amostra
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

fn run_whisper(core: &Core, engine: Engine, wav: &Path) -> Result<String, String> {
    let dir = &core.paths().whisper_dir;
    let exe = whisper_cli(dir, engine).ok_or_else(|| format!("whisper-cli ({}) não encontrado", engine.label()))?;
    let lang = Some(core.config().whisper_lang).filter(|l| !l.is_empty()).unwrap_or_else(|| "auto".into());
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).saturating_sub(2).max(1);
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("-m").arg(model_path(dir)).arg("-f").arg(wav).args([
        "-l",
        &lang,
        "-nt",
        "-np",
        "-t",
        &threads.to_string(),
        "--prompt",
        WHISPER_PROMPT,
        "-mc",
        "0",
        "--carry-initial-prompt",
    ]);
    if engine == Engine::Cpu {
        cmd.arg("-ng");
    }
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
        // Os binários de Linux trazem as .so ao lado do executável.
        if !cfg!(windows) && exe.starts_with(dir) {
            let mut paths = vec![parent.to_path_buf()];
            paths.extend(std::env::var_os("LD_LIBRARY_PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("LD_LIBRARY_PATH", joined);
            }
        }
    }
    let out = run_captured(cmd, None, Duration::from_secs(600), None, None).map_err(|e| e.to_string())?;
    let text = out.stdout.replace("[BLANK_AUDIO]", " ").split_whitespace().collect::<Vec<_>>().join(" ");
    match out.exit {
        Exit::Code(Some(0)) => Ok(normalize_transcription(&text)),
        exit => {
            let last = out.stderr.trim().lines().last().map(str::to_owned).filter(|l| !l.is_empty());
            Err(last.unwrap_or_else(|| match exit {
                Exit::Code(Some(c)) => format!("whisper saiu com código {c}"),
                _ => "whisper saiu com código null".into(),
            }))
        }
    }
}

pub(crate) fn transcribe_engine(core: &Core) -> String {
    engines(core).first().map(|e| e.label().to_owned()).unwrap_or_else(|| UNAVAILABLE.into())
}

pub(crate) fn transcribe_pcm16k(core: &Core, samples: &[f32]) -> anyhow::Result<Transcription> {
    let list = engines(core);
    if list.is_empty() {
        anyhow::bail!(NOT_READY);
    }
    let dir = crate::attachments::attachments_dir(&core.paths().data_dir)?;
    let wav = dir.join(format!("audio-{}.wav", crate::util::now_ms()));
    std::fs::write(&wav, wav_bytes(samples))?;
    let result = (|| {
        for (i, engine) in list.iter().enumerate() {
            let res = run_whisper(core, *engine, &wav).and_then(|t| if t.is_empty() { Err(NO_SPEECH.to_owned()) } else { Ok(t) });
            match res {
                Ok(text) => return Ok(Transcription { text, engine: engine.label().to_owned() }),
                Err(e) if e == NO_SPEECH || i + 1 == list.len() => anyhow::bail!(e),
                Err(e) => core.feed(FeedEntry::system(format!("Whisper em {} falhou ({e}); tentando a próxima opção.", engine.label()))),
            }
        }
        anyhow::bail!(NOT_READY)
    })();
    // Não há serventia em manter o áudio.
    let _ = std::fs::remove_file(&wav);
    result
}

// ---------- Download sob demanda ----------

/// Extrai um .zip em `target` (a biblioteca `zip` recusa caminhos que escapam da pasta).
fn extract_zip(file: &Path, target: &Path) -> Result<(), String> {
    let f = File::open(file).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    zip.extract(target).map_err(|e| e.to_string())
}

/// Remove `strip` componentes iniciais do caminho de uma entrada. `Ok(None)` = entrada
/// inteira descartada (a própria pasta raiz); caminhos com `..` ou absolutos são recusados.
fn stripped(path: &Path, strip: usize) -> Result<Option<PathBuf>, String> {
    use std::path::Component;
    let mut rel = PathBuf::new();
    for (i, c) in path.components().enumerate() {
        match c {
            Component::Normal(p) if i >= strip => rel.push(p),
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(format!("caminho inseguro no pacote: {}", path.display())),
        }
    }
    Ok(Some(rel).filter(|r| !r.as_os_str().is_empty()))
}

/// Extrai um .tar.gz em `target`, removendo `strip` componentes iniciais.
fn extract_tar_gz(file: &Path, target: &Path, strip: usize) -> Result<(), String> {
    let gz = flate2::read::GzDecoder::new(File::open(file).map_err(|e| e.to_string())?);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let Some(rel) = stripped(&path, strip)? else { continue };
        let dest = target.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        entry.unpack(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    Ok(())
}

/// Baixa (com retomada e SHA-256), extrai e confere que o whisper-cli ficou no lugar.
fn download_engine(core: &Core, engine: Engine, label: &str) -> Result<(), String> {
    let archive = engine.archive().ok_or_else(|| "sem pacote pré-compilado para este sistema".to_owned())?;
    let dir = core.paths().whisper_dir.clone();
    let file = dir.join(archive.file);
    let url = format!("{RELEASE}/{}", archive.file);
    fetch(core, &url, &file, label, archive.sha256)?;
    let target = dir.join(engine.dir());
    let existed = target.exists();
    core.emit(CoreEvent::Download(progress(label, DownloadState::Extracting)));
    let res = (|| {
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        if archive.tar_gz {
            extract_tar_gz(&file, &target, archive.strip)?;
        } else {
            extract_zip(&file, &target)?;
        }
        let exe = local_cli(&dir, engine).ok_or_else(|| format!("{} não encontrado no pacote extraído", exe_name()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        let _ = exe;
        Ok(())
    })();
    if res.is_err() && !existed {
        let _ = std::fs::remove_dir_all(&target);
    }
    let _ = std::fs::remove_file(&file);
    res
}

fn progress(label: &str, state: DownloadState) -> DownloadProgress {
    DownloadProgress {
        label: label.to_owned(),
        state,
        received: 0,
        total: None,
        speed: 0.0,
        attempt: 0,
        max_attempts: 0,
        retry_in_ms: 0,
        message: None,
    }
}

/// Download com progresso para a interface. O `Done` do downloader ainda é seguido por
/// extração, então vira `Finalizing`; só o fim do passo (em `ensure_whisper_async`) emite `Done`.
fn fetch(core: &Core, url: &str, dest: &Path, label: &str, sha256: &str) -> Result<(), String> {
    let opts = DownloadOpts::new(label, Some(sha256));
    download_file(url, dest, &opts, &mut |mut p| {
        if p.state == DownloadState::Done {
            p.state = DownloadState::Finalizing;
        }
        core.emit(CoreEvent::Download(p));
    })
    .map_err(|e| e.message)
}

/// Baixa o que faltar (modelo e motores) numa thread, com progresso na Conversa integrada.
pub(crate) fn ensure_whisper_async(core: &Core) {
    if core.inner.whisper.ensuring.swap(true, Ordering::SeqCst) {
        return;
    }
    let flag_owner = core.clone();
    let core = core.clone();
    let spawned = std::thread::Builder::new().name("cd-whisper-setup".into()).spawn(move || {
        let dir = core.paths().whisper_dir.clone();
        // (descrição, rótulo da barra, arquivo cujo .part sobrevive a uma falha, ação)
        type Step<'a> = (&'static str, &'static str, PathBuf, Box<dyn FnOnce() -> Result<(), String> + 'a>);
        let mut steps: Vec<Step> = Vec::new();
        if !model_path(&dir).is_file() {
            let (d, c) = (model_path(&dir), core.clone());
            steps.push((
                "modelo large-v3-turbo (1,6 GB)",
                "Modelo large-v3-turbo",
                d.clone(),
                Box::new(move || fetch(&c, MODEL_URL, &d, "Modelo large-v3-turbo", MODEL_SHA256)),
            ));
        }
        if whisper_cli(&dir, Engine::Cpu).is_none() {
            match Engine::Cpu.archive() {
                Some(a) => {
                    let c = core.clone();
                    steps.push((
                        "whisper.cpp para CPU",
                        "whisper.cpp (CPU)",
                        dir.join(a.file),
                        Box::new(move || download_engine(&c, Engine::Cpu, "whisper.cpp (CPU)")),
                    ));
                }
                None => core.feed(FeedEntry::system(format!(
                    "Transcrição local: não há binário pré-compilado do whisper.cpp para {}/{}. Compile-o (scripts/build-whisper-linux.sh) ou instale o whisper-cli no PATH.",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ))),
            }
        }
        if core.inner.whisper.has_nvidia() && local_cli(&dir, Engine::Cuda).is_none() {
            if let Some(a) = Engine::Cuda.archive() {
                let c = core.clone();
                steps.push((
                    "whisper.cpp com CUDA (~700 MB)",
                    "whisper.cpp (CUDA)",
                    dir.join(a.file),
                    Box::new(move || download_engine(&c, Engine::Cuda, "whisper.cpp (CUDA)")),
                ));
            }
        }
        for (what, label, keep, run) in steps {
            core.feed(FeedEntry::system(format!("Transcrição local: baixando {what}…")));
            match run() {
                Ok(()) => {
                    core.emit(CoreEvent::Download(progress(label, DownloadState::Done)));
                    core.feed(FeedEntry::system(format!("Transcrição local: {what} pronto.")));
                }
                Err(e) => {
                    let mut p = progress(label, DownloadState::Error);
                    p.message = Some(e.clone());
                    core.emit(CoreEvent::Download(p));
                    let resumable = if PathBuf::from(format!("{}.part", keep.display())).exists() {
                        " O que já foi baixado foi mantido e continua na próxima abertura do app."
                    } else {
                        ""
                    };
                    core.feed(FeedEntry::system(format!("Transcrição local: falha ao baixar {what} ({e}).{resumable}")));
                }
            }
        }
        core.inner.whisper.ensuring.store(false, Ordering::SeqCst);
    });
    if spawned.is_err() {
        // Falha (raríssima) ao criar a thread: permite tentar de novo depois.
        flag_owner.inner.whisper.ensuring.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(normalize_transcription("abre o Cláudio Code agora"), "abre o Claude Code agora");
        assert_eq!(normalize_transcription("usa o Cloud CLI"), "usa o Claude CLI");
        assert_eq!(normalize_transcription("pergunta pro Cloud sobre isso"), "pergunta pro Claude sobre isso");
        assert_eq!(normalize_transcription("chama o Claudio"), "chama o Claude");
        assert_eq!(normalize_transcription("pede ao Anti-crafty e ao Anti gravity"), "pede ao Antigravity e ao Antigravity");
        // Sem contexto, "Cláudio" (nome próprio) e "cloud" ficam como estão.
        assert_eq!(normalize_transcription("o Cláudio foi à cloud"), "o Cláudio foi à cloud");
    }

    #[test]
    fn wav_header() {
        let w = wav_bytes(&[0.0, 1.0, -1.0, 2.0]);
        assert_eq!(w.len(), 44 + 8);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([w[4], w[5], w[6], w[7]]), 36 + 8);
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([w[20], w[21]]), 1); // PCM
        assert_eq!(u16::from_le_bytes([w[22], w[23]]), 1); // mono
        assert_eq!(u32::from_le_bytes([w[24], w[25], w[26], w[27]]), 16_000);
        assert_eq!(u32::from_le_bytes([w[28], w[29], w[30], w[31]]), 32_000);
        assert_eq!(u16::from_le_bytes([w[34], w[35]]), 16);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 8);
        let s: Vec<i16> = w[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(s, [0, 32767, -32767, 32767]);
    }

    #[test]
    fn pacotes_por_plataforma_e_hashes_validos() {
        let a = engine_archive(Engine::Cpu, "windows", "x86_64").expect("win cpu");
        assert!(!a.tar_gz && a.strip == 0 && a.file == "whisper-bin-x64.zip");
        assert!(engine_archive(Engine::Cuda, "windows", "x86_64").is_some());
        let l = engine_archive(Engine::Cpu, "linux", "x86_64").expect("linux cpu");
        assert!(l.tar_gz && l.strip == 1);
        assert!(engine_archive(Engine::Cpu, "linux", "aarch64").is_some());
        assert!(engine_archive(Engine::Cuda, "linux", "x86_64").is_none());
        assert!(engine_archive(Engine::Vulkan, "windows", "x86_64").is_none());
        assert!(engine_archive(Engine::Cpu, "macos", "aarch64").is_none());
        let all = [
            engine_archive(Engine::Cpu, "windows", "x86_64"),
            engine_archive(Engine::Cuda, "windows", "x86_64"),
            engine_archive(Engine::Cpu, "linux", "x86_64"),
            engine_archive(Engine::Cpu, "linux", "aarch64"),
        ];
        for h in all.iter().flatten().map(|a| a.sha256).chain([MODEL_SHA256]) {
            assert!(h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{h}");
        }
    }

    #[test]
    fn pasta_do_motor_por_sistema() {
        if cfg!(windows) {
            assert_eq!(Engine::Cpu.dir(), "bin-cpu");
            assert_eq!(Engine::Cuda.dir(), "bin-cuda");
        } else {
            assert_eq!(Engine::Cpu.dir(), format!("bin-cpu-{}", std::env::consts::OS));
        }
    }

    #[test]
    fn lspci_e_nvidia_smi() {
        let out = [
            "00:02.0 \"VGA compatible controller\" \"Intel Corporation\" \"UHD Graphics 620\" -r07 \"Dell\" \"Device 08af\"",
            "01:00.0 \"3D controller\" \"NVIDIA Corporation\" \"GP108M [GeForce MX150]\" -ra1 \"Dell\" \"Device 08af\"",
            "00:1f.3 \"Audio device\" \"Intel Corporation\" \"Sunrise Point-LP HD Audio\" -r21 \"Dell\" \"Device 08af\"",
        ]
        .join("\n");
        assert_eq!(parse_lspci(&out), ["Intel Corporation UHD Graphics 620", "NVIDIA Corporation GP108M [GeForce MX150]"]);
        assert!(parse_lspci("").is_empty());
        assert_eq!(
            parse_nvidia_smi("GPU 0: NVIDIA GeForce RTX 3060 (UUID: GPU-abc)\nGPU 1: NVIDIA A100 (UUID: GPU-def)"),
            ["NVIDIA GeForce RTX 3060", "NVIDIA A100"]
        );
        assert!(parse_nvidia_smi("No devices were found").is_empty());
    }

    #[test]
    fn strip_de_componentes_recusa_caminhos_inseguros() {
        assert_eq!(stripped(Path::new("pasta/whisper-cli"), 1).expect("ok"), Some(PathBuf::from("whisper-cli")));
        assert_eq!(stripped(Path::new("pasta/"), 1).expect("ok"), None);
        assert_eq!(stripped(Path::new("pasta/sub/x.so"), 1).expect("ok"), Some(PathBuf::from("sub/x.so")));
        assert!(stripped(Path::new("pasta/../fora"), 1).is_err());
        assert!(stripped(Path::new("/abs/x"), 1).is_err());
    }

    #[test]
    fn extrai_tar_gz_sem_a_pasta_raiz() {
        let tmp = tempfile::tempdir().expect("tmp");
        let archive = tmp.path().join("pacote.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(File::create(&archive).expect("create"), flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            for (name, data) in [("raiz/whisper-cli", &b"#!/bin/sh\n"[..]), ("raiz/lib/libwhisper.so", &b"so"[..])] {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o755);
                h.set_cksum();
                b.append_data(&mut h, name, data).expect("append");
            }
            b.into_inner().expect("gz").finish().expect("finish");
        }
        let out = tmp.path().join("bin");
        extract_tar_gz(&archive, &out, 1).expect("extract");
        assert_eq!(std::fs::read(out.join("whisper-cli")).expect("cli"), b"#!/bin/sh\n");
        assert_eq!(std::fs::read(out.join("lib").join("libwhisper.so")).expect("so"), b"so");
        assert!(!out.join("raiz").exists());
    }

    #[test]
    fn engine_lookup() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(whisper_cli(tmp.path(), Engine::Vulkan).is_none());
        let d = tmp.path().join(Engine::Vulkan.dir()).join("Release");
        std::fs::create_dir_all(&d).expect("mkdir");
        std::fs::write(d.join(exe_name()), "x").expect("write");
        assert_eq!(whisper_cli(tmp.path(), Engine::Vulkan), Some(d.join(exe_name())));
    }
}
