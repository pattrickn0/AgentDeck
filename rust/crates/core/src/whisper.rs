//! Transcrição local com whisper.cpp (equivalente ao bloco "Transcrição local" do main.js).
//!
//! Motores em ordem de preferência: CUDA (GPU NVIDIA), Vulkan (qualquer GPU) e CPU. No
//! Windows os binários ficam em `whisper/bin-*/[Release/]whisper-cli.exe` e são baixados
//! sob demanda; nos demais sistemas usamos `whisper/bin-*/whisper-cli` ou o `whisper-cli`
//! do PATH. O WAV temporário é SEMPRE apagado após a transcrição.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crate::events::FeedEntry;
use crate::process::{run_captured, Exit};
use crate::{Core, Transcription};

pub const MODEL_FILE: &str = "ggml-large-v3-turbo.bin";
const RELEASE: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/b5454";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin";
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

impl Engine {
    pub fn dir(self) -> &'static str {
        match self {
            Engine::Cuda => "bin-cuda",
            Engine::Vulkan => "bin-vulkan",
            Engine::Cpu => "bin-cpu",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Engine::Cuda => "GPU NVIDIA (CUDA)",
            Engine::Vulkan => "GPU (Vulkan)",
            Engine::Cpu => "CPU",
        }
    }

    /// Pacote pré-compilado (somente Windows x64).
    fn zip_url(self) -> Option<String> {
        match self {
            Engine::Cuda => Some(format!("{RELEASE}/whisper-bin-win-cuda-12.4.0-x64.zip")),
            Engine::Vulkan => None,
            Engine::Cpu => Some(format!("{RELEASE}/whisper-bin-x64.zip")),
        }
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
    /// demais: "NVIDIA" se houver `nvidia-smi`.
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
    } else if which::which("nvidia-smi").is_ok() {
        vec!["NVIDIA".into()]
    } else {
        Vec::new()
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    }
}

/// Executável do motor, se presente.
pub fn whisper_cli(whisper_dir: &Path, engine: Engine) -> Option<PathBuf> {
    let subs: &[&str] = if cfg!(windows) { &["", "Release"] } else { &["", "Release", "bin"] };
    let found = subs.iter().map(|s| whisper_dir.join(engine.dir()).join(s).join(exe_name())).find(|p| p.is_file());
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
            (r"(?i)\b(?:Anti-?crafty|Anti-?craft|Anti-?graft(?:ing)?|Anti-?graph|Anti-gravity|Anti\s+gravity|Antigraviti)\b", "Antigravity"),
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
    cmd.arg("-m")
        .arg(model_path(dir))
        .arg("-f")
        .arg(wav)
        .args(["-l", &lang, "-nt", "-np", "-t", &threads.to_string(), "--prompt", WHISPER_PROMPT, "-mc", "0", "--carry-initial-prompt"]);
    if engine == Engine::Cpu {
        cmd.arg("-ng");
    }
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
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
                Err(e) => core.feed(FeedEntry::system(format!(
                    "Whisper em {} falhou ({e}); tentando a próxima opção.",
                    engine.label()
                ))),
            }
        }
        anyhow::bail!(NOT_READY)
    })();
    // Não há serventia em manter o áudio.
    let _ = std::fs::remove_file(&wav);
    result
}

// ---------- Download sob demanda ----------

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent("commanddeck")
        .timeout_connect(Some(Duration::from_secs(30)))
        .build()
        .into();
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(format!("HTTP {status} ao baixar {url}"));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let mut reader = resp.into_body().into_reader();
    let copy = std::fs::File::create(&part).and_then(|mut f| std::io::copy(&mut reader as &mut dyn Read, &mut f));
    if let Err(e) = copy {
        let _ = std::fs::remove_file(&part);
        return Err(e.to_string());
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

fn download_engine(whisper_dir: &Path, engine: Engine) -> Result<(), String> {
    let url = engine.zip_url().ok_or_else(|| "sem pacote pré-compilado".to_owned())?;
    let file = whisper_dir.join(format!("{}.zip", engine.id()));
    download(&url, &file)?;
    let target = whisper_dir.join(engine.dir());
    let res = (|| {
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let f = std::fs::File::open(&file).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        zip.extract(&target).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&file);
    res
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
        type Step = Box<dyn FnOnce() -> Result<(), String>>;
        let mut steps: Vec<(&str, Step)> = Vec::new();
        let any_cli = [Engine::Cuda, Engine::Vulkan, Engine::Cpu].iter().any(|e| whisper_cli(&dir, *e).is_some());
        // Fora do Windows não há binários pré-compilados: só vale baixar o modelo se já
        // houver um whisper-cli instalado.
        if !model_path(&dir).is_file() && (cfg!(windows) || any_cli) {
            let (d, url) = (model_path(&dir), MODEL_URL);
            steps.push(("modelo large-v3-turbo (1,6 GB)", Box::new(move || download(url, &d))));
        }
        if cfg!(windows) {
            if whisper_cli(&dir, Engine::Cpu).is_none() {
                let d = dir.clone();
                steps.push(("whisper.cpp para CPU", Box::new(move || download_engine(&d, Engine::Cpu))));
            }
            if core.inner.whisper.has_nvidia() && whisper_cli(&dir, Engine::Cuda).is_none() {
                let d = dir.clone();
                steps.push(("whisper.cpp com CUDA (~700 MB)", Box::new(move || download_engine(&d, Engine::Cuda))));
            }
        }
        for (what, run) in steps {
            core.feed(FeedEntry::system(format!("Transcrição local: baixando {what}…")));
            match run() {
                Ok(()) => core.feed(FeedEntry::system(format!("Transcrição local: {what} pronto."))),
                Err(e) => core.feed(FeedEntry::system(format!("Transcrição local: falha ao baixar {what} ({e})."))),
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
    fn engine_lookup() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(whisper_cli(tmp.path(), Engine::Vulkan).is_none());
        let d = tmp.path().join("bin-vulkan").join("Release");
        std::fs::create_dir_all(&d).expect("mkdir");
        std::fs::write(d.join(exe_name()), "x").expect("write");
        assert_eq!(whisper_cli(tmp.path(), Engine::Vulkan), Some(d.join(exe_name())));
    }
}
