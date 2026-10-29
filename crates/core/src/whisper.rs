//! Local transcription with whisper.cpp (equivalent to "Local transcription" block in main.js).
//!
//! Engines in order of preference: CUDA (NVIDIA GPU), Vulkan (any GPU) and CPU. On
//! Windows binaries are in `whisper/bin-*/[Release/]whisper-cli.exe`; on Linux, in
//! `whisper/bin-*-linux/whisper-cli` (or `whisper-cli` on PATH). The model and CPU engine
//! (Windows and Linux) and CUDA (Windows) are downloaded on demand, with SHA-256 pinned in this
//! file; for CUDA/Vulkan on Linux, a `whisper-cli` compiled with them on PATH.
//! The temporary WAV is ALWAYS deleted after transcription.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crate::config::DictationChoice;
use crate::download::{download_file, DownloadOpts};
use crate::events::{CoreEvent, DownloadProgress, DownloadState, FeedEntry};
use crate::process::{run_captured, Exit};
use crate::t;
use crate::{Core, Transcription};

pub const MODEL_FILE: &str = "ggml-large-v3-turbo.bin";
const RELEASE: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/b5454";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin";
/// Model SHA-256: matches Hugging Face LFS ETag.
const MODEL_SHA256: &str = "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69";
/// Vocabulary that guides proper noun recognition (the agent names are the point).
const PROMPT_NAMES: &str = "Holt, Claude, Antigravity, agy, Claude Code, Ollama, Codex, OpenCode, Aider, Goose, terminal, CLI";

/// Vocabulary prompt for a spoken language code (`en`, `pt`, `auto`...): English and Portuguese get their own
/// wording, any other language gets only the language-neutral names.
pub fn whisper_prompt(lang: &str) -> String {
    match lang {
        "en" => format!("{PROMPT_NAMES}, Claude assistant, Google Antigravity, Vulkan, NVIDIA, AMD."),
        "pt" => format!("{PROMPT_NAMES}, assistente Claude, Google Antigravity, Vulkan, NVIDIA, AMD."),
        _ => format!("{PROMPT_NAMES}, Google Antigravity, Vulkan, NVIDIA, AMD."),
    }
}

/// `whisper-cli` arguments that depend on the dictation settings: spoken language and translation into English.
pub fn language_args(lang: &str, translate: bool) -> Vec<String> {
    let lang = if lang.is_empty() { "auto" } else { lang };
    let mut args = vec!["-l".to_owned(), lang.to_owned()];
    if translate {
        args.push("-tr".to_owned());
    }
    args
}
/// Name of the Whisper model in use, for display.
pub fn model_name() -> &'static str {
    MODEL_FILE.trim_start_matches("ggml-").trim_end_matches(".bin")
}
pub const UNAVAILABLE: &str = "unavailable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Cuda,
    Vulkan,
    Cpu,
}

/// Precompiled package of an engine (whisper.cpp release b5454), with pinned SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Archive {
    file: &'static str,
    tar_gz: bool,
    /// Leading path components removed during extraction (Linux .tar.gz packages include a root directory).
    strip: usize,
    sha256: &'static str,
}

/// Hashes from the GitHub release "digest" field; CPU ones (Windows and Linux) were verified
/// by downloading files, CUDA package (685 MB) comes from digest only.
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
    /// Engine directory in `whisper/`. On Linux uses `-linux` suffix to avoid mixing with
    /// Windows binaries versioned in `bin-cpu/` and `bin-vulkan/`.
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

    /// Precompiled package for this system and architecture, if available.
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

/// Subsystem state (detected GPUs and download in progress).
#[derive(Default)]
pub(crate) struct WhisperState {
    gpus: OnceLock<Vec<String>>,
    ensuring: AtomicBool,
}

impl WhisperState {
    /// GPU names (cached). Windows: `Get-CimInstance Win32_VideoController`;
    /// Linux: `lspci -mm` and, if no NVIDIA in it, `nvidia-smi -L`.
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

/// Executable inside `whisper/bin-<engine>/` (root, `Release/` or `bin/`).
fn local_cli(whisper_dir: &Path, engine: Engine) -> Option<PathBuf> {
    cli_in(&whisper_dir.join(engine.dir()))
}

/// Executable inside an engine directory (root, `Release/` or `bin/`).
fn cli_in(engine_dir: &Path) -> Option<PathBuf> {
    let subs: &[&str] = if cfg!(windows) { &["", "Release"] } else { &["", "Release", "bin"] };
    subs.iter().map(|s| engine_dir.join(s).join(exe_name())).find(|p| p.is_file())
}

/// Engine executable, if present. Outside Windows a `whisper-cli` on PATH counts as CPU.
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

/// Model and an engine are on disk: dictation works (`whisper-cli` on PATH counts as CPU outside Windows).
pub fn is_installed(whisper_dir: &Path) -> bool {
    model_path(whisper_dir).is_file() && [Engine::Cuda, Engine::Vulkan, Engine::Cpu].iter().any(|e| whisper_cli(whisper_dir, *e).is_some())
}

/// What the app does with dictation upon opening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    /// Ensures missing components (model and engines) in the background, as always.
    Ensure,
    /// Never asked and nothing downloaded: UI asks before downloading.
    Ask,
    /// User does not want dictation: downloads nothing.
    Skip,
}

/// Decides what to do on startup and which choice to save. Anyone who already downloaded Whisper (users
/// before the prompt) is never asked: counts as "yes".
pub fn startup(choice: DictationChoice, installed: bool) -> (Startup, DictationChoice) {
    match choice {
        DictationChoice::Yes => (Startup::Ensure, choice),
        DictationChoice::No => (Startup::Skip, choice),
        DictationChoice::Unasked if installed => (Startup::Ensure, DictationChoice::Yes),
        DictationChoice::Unasked => (Startup::Ask, choice),
    }
}

pub(crate) fn is_downloading(core: &Core) -> bool {
    core.inner.whisper.ensuring.load(Ordering::SeqCst)
}

/// What `remove_downloaded` deletes inside `whisper_dir`: the model, CUDA engine, packages and
/// partial downloads and, outside Windows, downloaded engines. On Windows `bin-cpu/` and `bin-vulkan/` remain:
/// they are versioned in the repository and are small compared to the model.
fn removable(whisper_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![model_path(whisper_dir), crate::download::part_path(&model_path(whisper_dir))];
    for engine in [Engine::Cuda, Engine::Vulkan, Engine::Cpu] {
        if engine == Engine::Cuda || !cfg!(windows) {
            let dir = whisper_dir.join(engine.dir());
            let mut staging = dir.as_os_str().to_os_string();
            staging.push(".staging");
            paths.push(dir);
            paths.push(PathBuf::from(staging));
        }
        if let Some(a) = engine.archive() {
            let file = whisper_dir.join(a.file);
            paths.push(crate::download::part_path(&file));
            paths.push(file);
        }
    }
    paths.into_iter().filter(|p| p.exists()).collect()
}

fn size_of(path: &Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => std::fs::read_dir(path).map(|d| d.flatten().map(|e| size_of(&e.path())).sum()).unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// Deletes what dictation downloaded and returns freed bytes. Refuses while a download is in progress.
pub(crate) fn remove_downloaded(core: &Core) -> Result<u64, String> {
    if is_downloading(core) {
        return Err(t!("wait for dictation download to finish").to_owned());
    }
    let mut freed = 0;
    for path in removable(&core.paths().whisper_dir) {
        let size = size_of(&path);
        let res = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        res.map_err(|e| t!("could not delete {path}: {error}", path = path.display(), error = e))?;
        freed += size;
    }
    Ok(freed)
}

/// Order of preference for available engines (empty without model).
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
    // Outside Windows we do not detect Vulkan GPUs: we try if binary exists.
    if !st.gpus().is_empty() || !cfg!(windows) {
        order.push(Engine::Vulkan);
    }
    order.push(Engine::Cpu);
    order.retain(|e| whisper_cli(dir, *e).is_some());
    order
}

/// Corrects common phonetic variations of agent names.
///
/// `lang` is the spoken language setting: the Portuguese rules apply for `pt` and for `auto` (which may hear
/// Portuguese); the agent-name rules always apply. With translation on, the output is English: no Portuguese rules.
pub fn normalize_transcription(text: &str, lang: &str, translate: bool) -> String {
    static RULES: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    static PT_RULES: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    let compile = |list: &[(&str, &'static str)]| -> Vec<(regex::Regex, &'static str)> {
        list.iter().filter_map(|(re, rep)| regex::Regex::new(re).ok().map(|r| (r, *rep))).collect()
    };
    let rules = RULES.get_or_init(|| {
        compile(&[
            // Antigravity.
            (
                r"(?i)\b(?:Anti-?crafty|Anti-?craft|Anti-?graft(?:ing)?|Anti-?graph|Anti-gravity|Anti\s+gravity|Antigraviti)\b",
                "Antigravity",
            ),
        ])
    });
    let pt_rules = PT_RULES.get_or_init(|| {
        compile(&[
            // Claude with command context ("Cláudio Code" → "Claude Code").
            (r"(?i)\b(?:Cl[áa]udio|Cl[áa]ud[eê]|Cl[áa]udi[ao])\b(\s+(?:Code|CLI|agente|assistente))", "Claude${1}"),
            (r"(?i)\bCloud\s+(Code|CLI|agente|assistente)\b", "Claude ${1}"),
            (r"(?i)\b(com o|para o|pro|ao|chama o|pergunta pro|pergunta ao)\s+Cloud\b", "${1} Claude"),
            (r"(?i)\b(com o|para o|pro|chama o|pergunta pro|pergunta ao)\s+Cl[áa]udio\b", "${1} Claude"),
        ])
    });
    let portuguese = !translate && matches!(lang, "pt" | "auto" | "");
    let mut out = text.to_owned();
    let all = rules.iter().chain(portuguese.then_some(pt_rules).into_iter().flatten());
    for (re, rep) in all {
        out = re.replace_all(&out, *rep).into_owned();
    }
    out
}

/// Loudest sample below which a recording is taken as silence (-40 dBFS). Speech peaks well above it even on a weak
/// microphone; a dead or wrong input stays far below (about -76 dBFS measured on Linux). Whisper fed silence does not
/// say so: it makes up a short phrase ("e aí", "Thank you."), which would be typed as if dictated.
const SILENCE_PEAK: f32 = 0.01;

fn is_silence(samples: &[f32]) -> bool {
    samples.iter().all(|s| s.abs() < SILENCE_PEAK)
}

/// 16-bit PCM WAV, mono, 16 kHz (f32 samples in -1..1).
pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    const RATE: u32 = 16_000;
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes()); // fmt block size
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    b.extend_from_slice(&2u16.to_le_bytes()); // block alignment
    b.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
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
    let exe = whisper_cli(dir, engine).ok_or_else(|| t!("whisper-cli ({engine}) not found", engine = engine.label()))?;
    let cfg = core.config();
    let (lang, translate) = (cfg.whisper_lang.clone(), cfg.whisper_translate);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).saturating_sub(2).max(1);
    // Process runs in executable's directory: relative paths would become invalid.
    let (exe, model, wav) = (crate::util::absolutize(&exe), crate::util::absolutize(&model_path(dir)), crate::util::absolutize(wav));
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("-m").arg(&model).arg("-f").arg(&wav).args(language_args(&lang, translate)).args([
        "-nt",
        "-np",
        "-t",
        &threads.to_string(),
        "--prompt",
        &whisper_prompt(&lang),
        "-mc",
        "0",
        "--carry-initial-prompt",
    ]);
    if engine == Engine::Cpu {
        cmd.arg("-ng");
    }
    if let Some(parent) = exe.parent() {
        cmd.current_dir(parent);
        // Linux binaries include .so libraries next to executable.
        if !cfg!(windows) && crate::util::path_within(&exe, &crate::util::absolutize(dir)) {
            let mut paths = vec![parent.to_path_buf()];
            paths.extend(std::env::var_os("LD_LIBRARY_PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("LD_LIBRARY_PATH", joined);
            }
        }
    }
    // Closing the app (`Core::shutdown`) interrupts transcription: process is in registry and
    // termination signal kills it, instead of running orphaned until timeout.
    let shutdown = &core.inner.state.shutdown;
    if shutdown.load(Ordering::SeqCst) {
        return Err(t!("transcription interrupted: the app is closing").into());
    }
    let out = run_captured(cmd, None, Duration::from_secs(600), Some(shutdown), Some(&core.inner.processes)).map_err(|e| e.to_string())?;
    let text = out.stdout.replace("[BLANK_AUDIO]", " ").split_whitespace().collect::<Vec<_>>().join(" ");
    match out.exit {
        Exit::Code(Some(0)) => Ok(normalize_transcription(&text, &lang, translate)),
        Exit::Cancelled => Err(t!("transcription interrupted: the app is closing").into()),
        exit => {
            let last = out.stderr.trim().lines().last().map(str::to_owned).filter(|l| !l.is_empty());
            Err(last.unwrap_or_else(|| match exit {
                Exit::Code(Some(c)) => t!("whisper exited with code {code}", code = c),
                _ => t!("whisper exited with code null").to_owned(),
            }))
        }
    }
}

pub(crate) fn transcribe_engine(core: &Core) -> String {
    engines(core).first().map(|e| e.label().to_owned()).unwrap_or_else(|| t!("unavailable").to_owned())
}

/// Unique temporary WAV filename: milliseconds + process + counter (two calls in the same
/// millisecond cannot overwrite or delete each other's file).
fn unique_wav_name() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("audio-{}-{}-{}.wav", crate::util::now_ms(), std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn transcribe_pcm16k(core: &Core, samples: &[f32]) -> anyhow::Result<Transcription> {
    let list = engines(core);
    if list.is_empty() {
        anyhow::bail!(t!("local transcription is not ready yet (see the integrated chat)"));
    }
    if is_silence(samples) {
        anyhow::bail!(t!("the microphone recorded only silence: check the input device in the system's sound settings"));
    }
    let dir = crate::attachments::attachments_dir(&core.paths().data_dir)?;
    let wav = dir.join(unique_wav_name());
    std::fs::write(&wav, wav_bytes(samples))?;
    let result = (|| {
        for (i, engine) in list.iter().enumerate() {
            let res =
                run_whisper(core, *engine, &wav).and_then(|t| if t.is_empty() { Err(t!("no speech detected").to_owned()) } else { Ok(t) });
            match res {
                Ok(text) => return Ok(Transcription { text, engine: engine.label().to_owned() }),
                Err(e)
                    if e == t!("no speech detected") || e == t!("transcription interrupted: the app is closing") || i + 1 == list.len() =>
                {
                    anyhow::bail!(e)
                }
                Err(e) => core.feed(FeedEntry::system(t!(
                    "Whisper on {engine} failed ({error}); trying the next option.",
                    engine = engine.label(),
                    error = e,
                ))),
            }
        }
        anyhow::bail!(t!("local transcription is not ready yet (see the integrated chat)"))
    })();
    // No use keeping the audio.
    let _ = std::fs::remove_file(&wav);
    result
}

// ---------- On-demand download ----------

/// Extracts a .zip into `target` (the `zip` library rejects paths that escape the folder).
fn extract_zip(file: &Path, target: &Path) -> Result<(), String> {
    let f = File::open(file).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    zip.extract(target).map_err(|e| e.to_string())
}

/// Removes `strip` leading path components from an entry. `Ok(None)` = entry
/// entirely discarded (the root directory itself); paths with `..` or absolute are rejected.
fn stripped(path: &Path, strip: usize) -> Result<Option<PathBuf>, String> {
    use std::path::Component;
    let mut rel = PathBuf::new();
    for (i, c) in path.components().enumerate() {
        match c {
            Component::Normal(p) if i >= strip => rel.push(p),
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(t!("unsafe path in package: {path}", path = path.display())),
        }
    }
    Ok(Some(rel).filter(|r| !r.as_os_str().is_empty()))
}

/// Extracts a .tar.gz into `target`, removing `strip` leading components.
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

/// Downloads (with resume and SHA-256), extracts and verifies whisper-cli is in place.
fn download_engine(core: &Core, engine: Engine, label: &str) -> Result<(), String> {
    let archive = engine.archive().ok_or_else(|| t!("no precompiled package for this system").to_owned())?;
    let dir = core.paths().whisper_dir.clone();
    let file = dir.join(archive.file);
    let url = format!("{RELEASE}/{}", archive.file);
    fetch(core, &url, &file, label, archive.sha256)?;
    let target = dir.join(engine.dir());
    core.emit(CoreEvent::Download(progress(label, DownloadState::Extracting)));
    // Package is only deleted after success: on failure, the next attempt reuses it.
    install_package(&file, &target, archive.tar_gz, archive.strip)?;
    let _ = std::fs::remove_file(&file);
    Ok(())
}

/// Extracts package into a staging directory next to `target`, verifies `whisper-cli` is
/// there and only then swaps it for `target`. A failure midway (app closed, disk full) never leaves
/// a half-installed engine in place of the final one.
fn install_package(file: &Path, target: &Path, tar_gz: bool, strip: usize) -> Result<(), String> {
    let mut name = target.as_os_str().to_os_string();
    name.push(".staging");
    let staging = PathBuf::from(name);
    let _ = std::fs::remove_dir_all(&staging); // leftover from interrupted run
    let res = (|| {
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        if tar_gz {
            extract_tar_gz(file, &staging, strip)?;
        } else {
            extract_zip(file, &staging)?;
        }
        let exe = cli_in(&staging).ok_or_else(|| t!("{exe} not found in extracted package", exe = exe_name()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        let _ = exe;
        if target.exists() {
            // Leftover from an old/incomplete installation (no executable): removed to make room for new one.
            std::fs::remove_dir_all(target).map_err(|e| t!("could not replace {path}: {error}", path = target.display(), error = e))?;
        }
        crate::download::rename_with_retry(&staging, target)
            .map_err(|e| t!("could not install in {path}: {error}", path = target.display(), error = e))
    })();
    if res.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
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

/// Download with UI progress. Downloader's `Done` is followed by
/// extraction, so it becomes `Finalizing`; only step completion (in `ensure_whisper_async`) emits `Done`.
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

/// Downloads what is missing (model and engines) on a thread, with progress in integrated chat.
pub(crate) fn ensure_whisper_async(core: &Core) {
    if core.inner.whisper.ensuring.swap(true, Ordering::SeqCst) {
        return;
    }
    let flag_owner = core.clone();
    let core = core.clone();
    let spawned = std::thread::Builder::new().name("cd-whisper-setup".into()).spawn(move || {
        let dir = core.paths().whisper_dir.clone();
        // (description, bar label, file whose .part survives a failure, action)
        type Step<'a> = (&'static str, &'static str, PathBuf, Box<dyn FnOnce() -> Result<(), String> + 'a>);
        let mut steps: Vec<Step> = Vec::new();
        if !model_path(&dir).is_file() {
            let (d, c) = (model_path(&dir), core.clone());
            steps.push((
                t!("large-v3-turbo model (1.6 GB)"),
                t!("large-v3-turbo model"),
                d.clone(),
                Box::new(move || fetch(&c, MODEL_URL, &d, t!("large-v3-turbo model"), MODEL_SHA256)),
            ));
        }
        if whisper_cli(&dir, Engine::Cpu).is_none() {
            match Engine::Cpu.archive() {
                Some(a) => {
                    let c = core.clone();
                    steps.push((
                        t!("whisper.cpp for CPU"),
                        t!("whisper.cpp (CPU)"),
                        dir.join(a.file),
                        Box::new(move || download_engine(&c, Engine::Cpu, t!("whisper.cpp (CPU)"))),
                    ));
                }
                None => core.feed(FeedEntry::system(t!(
                    "Local transcription: no precompiled whisper.cpp binary for {os}/{arch}. Compile whisper.cpp with Vulkan/CUDA and put whisper-cli on PATH, or use the CPU engine.",
                    os = std::env::consts::OS,
                    arch = std::env::consts::ARCH,
                ))),
            }
        }
        if core.inner.whisper.has_nvidia() && local_cli(&dir, Engine::Cuda).is_none() {
            if let Some(a) = Engine::Cuda.archive() {
                let c = core.clone();
                steps.push((
                    t!("whisper.cpp with CUDA (~700 MB)"),
                    t!("whisper.cpp (CUDA)"),
                    dir.join(a.file),
                    Box::new(move || download_engine(&c, Engine::Cuda, t!("whisper.cpp (CUDA)"))),
                ));
            }
        }
        for (what, label, keep, run) in steps {
            core.feed(FeedEntry::system(t!("Local transcription: downloading {what}…", what)));
            match run() {
                Ok(()) => {
                    core.emit(CoreEvent::Download(progress(label, DownloadState::Done)));
                    core.feed(FeedEntry::system(t!("Local transcription: {what} ready.", what)));
                }
                Err(e) => {
                    let mut p = progress(label, DownloadState::Error);
                    p.message = Some(e.clone());
                    core.emit(CoreEvent::Download(p));
                    let resumable = if crate::download::part_path(&keep).exists() {
                        t!(" What was already downloaded has been kept and will resume next time the app opens.")
                    } else {
                        ""
                    };
                    core.feed(FeedEntry::system(t!(
                        "Local transcription: failed to download {what} ({error}).{resumable}",
                        what,
                        error = e,
                        resumable,
                    )));
                }
            }
        }
        core.inner.whisper.ensuring.store(false, Ordering::SeqCst);
        core.emit(CoreEvent::DictationChanged);
    });
    if spawned.is_err() {
        // (Very rare) thread spawn failure: allows retrying later.
        flag_owner.inner.whisper.ensuring.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_recording_never_reaches_whisper() {
        // Dead input as measured on openSUSE/PipeWire: peak 33 of 32768.
        assert!(is_silence(&[0.0, 0.001, -0.00101, 0.0005]));
        assert!(is_silence(&[]));
        // Quiet speech on a weak microphone still peaks above -40 dBFS.
        assert!(!is_silence(&[0.0, 0.002, -0.03, 0.001]));
    }

    #[test]
    fn normalization() {
        let n = |t: &str| normalize_transcription(t, "pt", false);
        assert_eq!(n("abre o Cláudio Code agora"), "abre o Claude Code agora");
        assert_eq!(n("usa o Cloud CLI"), "usa o Claude CLI");
        assert_eq!(n("pergunta pro Cloud sobre isso"), "pergunta pro Claude sobre isso");
        assert_eq!(n("chama o Claudio"), "chama o Claude");
        assert_eq!(n("pede ao Anti-crafty e ao Anti gravity"), "pede ao Antigravity e ao Antigravity");
        // Without context, "Cláudio" (proper noun) and "cloud" stay as they are.
        assert_eq!(n("o Cláudio foi à cloud"), "o Cláudio foi à cloud");
    }

    #[test]
    fn portuguese_rules_depend_on_the_language() {
        let text = "pergunta pro Cloud sobre o Anti gravity";
        assert_eq!(normalize_transcription(text, "auto", false), "pergunta pro Claude sobre o Antigravity");
        // English or another language, or a translation into English: only the agent names are fixed.
        for (lang, tr) in [("en", false), ("es", false), ("pt", true)] {
            assert_eq!(normalize_transcription(text, lang, tr), "pergunta pro Cloud sobre o Antigravity", "{lang} {tr}");
        }
    }

    #[test]
    fn prompt_depends_on_the_language() {
        assert!(whisper_prompt("en").contains("Claude assistant") && !whisper_prompt("en").contains("assistente"));
        assert!(whisper_prompt("pt").contains("assistente Claude"));
        for lang in ["en", "pt", "auto", "ja"] {
            assert!(whisper_prompt(lang).contains("Antigravity") && whisper_prompt(lang).contains("Codex"), "{lang}");
        }
        let neutral = whisper_prompt("ja");
        assert!(!neutral.contains("assistente") && !neutral.contains("assistant"));
    }

    #[test]
    fn language_arguments() {
        assert_eq!(language_args("en", false), ["-l", "en"]);
        assert_eq!(language_args("pt", true), ["-l", "pt", "-tr"]);
        assert_eq!(language_args("auto", true), ["-l", "auto", "-tr"]);
        assert_eq!(language_args("", false), ["-l", "auto"]);
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
    fn platform_packages_and_valid_hashes() {
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
    fn engine_directory_by_system() {
        if cfg!(windows) {
            assert_eq!(Engine::Cpu.dir(), "bin-cpu");
            assert_eq!(Engine::Cuda.dir(), "bin-cuda");
        } else {
            assert_eq!(Engine::Cpu.dir(), format!("bin-cpu-{}", std::env::consts::OS));
        }
    }

    #[test]
    fn lspci_and_nvidia_smi() {
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
    fn component_strip_rejects_unsafe_paths() {
        assert_eq!(stripped(Path::new("folder/whisper-cli"), 1).expect("ok"), Some(PathBuf::from("whisper-cli")));
        assert_eq!(stripped(Path::new("folder/"), 1).expect("ok"), None);
        assert_eq!(stripped(Path::new("folder/sub/x.so"), 1).expect("ok"), Some(PathBuf::from("sub/x.so")));
        assert!(stripped(Path::new("folder/../outside"), 1).is_err());
        assert!(stripped(Path::new("/abs/x"), 1).is_err());
    }

    #[test]
    fn extracts_tar_gz_without_root_folder() {
        let tmp = tempfile::tempdir().expect("tmp");
        let archive = tmp.path().join("pacote.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(File::create(&archive).expect("create"), flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            for (name, data) in [("root/whisper-cli", &b"#!/bin/sh\n"[..]), ("root/lib/libwhisper.so", &b"so"[..])] {
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
        assert!(!out.join("root").exists());
    }

    fn tar_gz(path: &Path, entries: &[(&str, &[u8])]) {
        let gz = flate2::write::GzEncoder::new(File::create(path).expect("create"), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (name, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, name, *data).expect("append");
        }
        b.into_inner().expect("gz").finish().expect("finish");
    }

    #[test]
    fn installation_only_swaps_folder_after_extracting_everything() {
        let tmp = tempfile::tempdir().expect("tmp");
        let pkg = tmp.path().join("pacote.tar.gz");
        let target = tmp.path().join("bin-x");
        // Package without executable: final folder does not appear and staging is removed.
        tar_gz(&pkg, &[("root/other.dll", b"dll")]);
        assert!(install_package(&pkg, &target, true, 1).is_err());
        assert!(!target.exists());
        assert!(!tmp.path().join("bin-x.staging").exists());
        // Truncated package in the middle: same, and previous installation is untouched.
        std::fs::create_dir_all(&target).expect("mkdir");
        std::fs::write(target.join("old.txt"), "v").expect("write");
        let bytes = std::fs::read(&pkg).expect("read");
        std::fs::write(&pkg, &bytes[..bytes.len() / 2]).expect("trunc");
        assert!(install_package(&pkg, &target, true, 1).is_err());
        assert!(target.join("old.txt").is_file());
        assert!(!tmp.path().join("bin-x.staging").exists());
        // Good package: swaps folder (removing old leftovers) and cleans staging.
        tar_gz(&pkg, &[("root/whisper-cli", b"cli"), ("root/whisper-cli.exe", b"cli"), ("root/ggml.dll", b"dll")]);
        install_package(&pkg, &target, true, 1).expect("install");
        assert!(cli_in(&target).is_some());
        assert!(target.join("ggml.dll").is_file());
        assert!(!target.join("old.txt").exists());
        assert!(!tmp.path().join("bin-x.staging").exists());
        assert!(pkg.is_file(), "package is not deleted by install_package");
    }

    #[test]
    fn temp_wav_names_are_unique() {
        let names: std::collections::HashSet<String> = (0..1000).map(|_| unique_wav_name()).collect();
        assert_eq!(names.len(), 1000);
    }

    #[test]
    fn startup_asks_only_who_was_never_asked_and_has_nothing_downloaded() {
        use DictationChoice::{No, Unasked, Yes};
        // Clean first run: asks, without downloading or saving anything.
        assert_eq!(startup(Unasked, false), (Startup::Ask, Unasked));
        // User from before the prompt (already has Whisper): nothing changes, counts as "yes".
        assert_eq!(startup(Unasked, true), (Startup::Ensure, Yes));
        // Answered "yes": proceeds to download what is missing (an interrupted download continues).
        assert_eq!(startup(Yes, false), (Startup::Ensure, Yes));
        assert_eq!(startup(Yes, true), (Startup::Ensure, Yes));
        // Answered "not now": never downloads or asks again, even if model appeared.
        assert_eq!(startup(No, false), (Startup::Skip, No));
        assert_eq!(startup(No, true), (Startup::Skip, No));
    }

    #[test]
    fn installed_needs_the_model_and_an_engine() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        assert!(!is_installed(dir));
        let engine = dir.join(Engine::Vulkan.dir());
        std::fs::create_dir_all(&engine).expect("mkdir");
        std::fs::write(engine.join(exe_name()), "x").expect("write");
        assert!(!is_installed(dir), "engine without model");
        std::fs::create_dir_all(dir.join("models")).expect("mkdir");
        std::fs::write(model_path(dir), "m").expect("write");
        assert!(is_installed(dir));
        std::fs::remove_dir_all(&engine).expect("rm");
        // Outside Windows a whisper-cli on PATH also counts as CPU engine.
        assert_eq!(is_installed(dir), !cfg!(windows) && which::which("whisper-cli").is_ok());
    }

    #[test]
    fn removal_lists_the_download_and_leaves_the_rest() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        assert!(removable(dir).is_empty());
        std::fs::create_dir_all(dir.join("models")).expect("mkdir");
        std::fs::write(model_path(dir), "modelo").expect("write");
        std::fs::write(dir.join("models").join("README.md"), "read").expect("write");
        let cuda = dir.join(Engine::Cuda.dir());
        std::fs::create_dir_all(&cuda).expect("mkdir");
        std::fs::write(cuda.join("a.dll"), "dll").expect("write");
        let list = removable(dir);
        assert!(list.contains(&model_path(dir)) && list.contains(&cuda), "{list:?}");
        assert!(!list.iter().any(|p| p.ends_with("README.md")));
        assert_eq!(list.iter().map(|p| size_of(p)).sum::<u64>(), 6 + 3);
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
