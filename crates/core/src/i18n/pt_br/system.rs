//! pt-BR translations for the area: Whisper, downloads, processes, trust, scope, attachments and utilities in the core.
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See the `i18n` module docs for
//! the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    // Downloads
    ("invalid partial file; restarting from scratch", "arquivo parcial inválido; recomeçando do zero"),
    ("HTTP {status} downloading {url}", "HTTP {status} ao baixar {url}"),
    ("inconsistent partial response; restarting from scratch", "resposta parcial inconsistente; recomeçando do zero"),
    ("could not write {path}: {error}", "não foi possível gravar {path}: {error}"),
    ("could not start download: {error}", "não foi possível iniciar o download: {error}"),
    ("failed to write {path}: {error}", "falha ao gravar {path}: {error}"),
    ("connection interrupted: {error}", "conexão interrompida: {error}"),
    ("connection stalled (no data for {seconds}s)", "conexão parada (sem dados por {seconds}s)"),
    ("connection interrupted", "conexão interrompida"),
    ("incomplete download", "download incompleto"),
    ("failed to read {path} to verify hash: {error}", "falha ao ler {path} para verificar o hash: {error}"),
    (
        "SHA-256 does not match (expected {expected}, got {actual}); file discarded",
        "SHA-256 não confere (esperado {expected}, obtido {actual}); arquivo descartado",
    ),
    (
        "SHA-256 does not match (expected {expected}, got {actual}); retrying from scratch",
        "SHA-256 não confere (esperado {expected}, obtido {actual}); baixando de novo do zero",
    ),
    // Whisper
    ("unavailable", "indisponível"),
    (
        "local transcription is not ready yet (see the notifications)",
        "a transcrição local ainda não está pronta (veja as notificações)",
    ),
    ("no speech detected", "nenhuma fala detectada"),
    (
        "the microphone recorded only silence: check the input device in the system's sound settings",
        "o microfone gravou só silêncio: confira o dispositivo de entrada nas configurações de som do sistema",
    ),
    ("transcription interrupted: the app is closing", "transcrição interrompida: o app está fechando"),
    ("whisper-cli ({engine}) not found", "whisper-cli ({engine}) não encontrado"),
    ("whisper exited with code {code}", "whisper saiu com código {code}"),
    ("whisper exited with code null", "whisper saiu com código null"),
    ("Whisper on {engine} failed ({error}); trying the next option.", "Whisper em {engine} falhou ({error}); tentando a próxima opção."),
    ("wait for dictation download to finish", "espere o download do ditado terminar"),
    ("could not delete {path}: {error}", "não foi possível apagar {path}: {error}"),
    ("large-v3-turbo model (1.6 GB)", "modelo large-v3-turbo (1,6 GB)"),
    ("large-v3-turbo model", "Modelo large-v3-turbo"),
    ("whisper.cpp for CPU", "whisper.cpp para CPU"),
    ("whisper.cpp (CPU)", "whisper.cpp (CPU)"),
    (
        "Local transcription: no precompiled whisper.cpp binary for {os}/{arch}. Compile whisper.cpp with Vulkan/CUDA and put whisper-cli on PATH, or use the CPU engine.",
        "Transcrição local: não há binário pré-compilado do whisper.cpp para {os}/{arch}. Compile o whisper.cpp com Vulkan/CUDA e ponha o whisper-cli no PATH, ou use o motor de CPU.",
    ),
    ("whisper.cpp with CUDA (~700 MB)", "whisper.cpp com CUDA (~700 MB)"),
    ("whisper.cpp (CUDA)", "whisper.cpp (CUDA)"),
    ("Local transcription: downloading {what}…", "Transcrição local: baixando {what}…"),
    ("Local transcription: {what} ready.", "Transcrição local: {what} pronto."),
    (
        " What was already downloaded has been kept and will resume next time the app opens.",
        " O que já foi baixado foi mantido e continua na próxima abertura do app.",
    ),
    (
        "Local transcription: failed to download {what} ({error}).{resumable}",
        "Transcrição local: falha ao baixar {what} ({error}).{resumable}",
    ),
    ("no precompiled package for this system", "sem pacote pré-compilado para este sistema"),
    ("unsafe path in package: {path}", "caminho inseguro no pacote: {path}"),
    ("{exe} not found in extracted package", "{exe} não encontrado no pacote extraído"),
    ("could not replace {path}: {error}", "não foi possível substituir {path}: {error}"),
    ("could not install in {path}: {error}", "não foi possível instalar em {path}: {error}"),
];
