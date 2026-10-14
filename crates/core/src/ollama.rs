//! Motor de agente autônomo para Ollama com ferramentas de workspace (porta de `ollamaAgent.js`).
//!
//! Permite que modelos locais (qwen3.5:9b, qwen2.5-coder, ...) leiam, editem e executem
//! qualquer coisa dentro do workspace configurado. Usado pela ponte/mesas (modo headless,
//! sem streaming) e pelo CLI interativo `ollama-agent` (com streaming de tokens).

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};

use crate::context::{self, ContextChunk, ContextRequest, Providers};

/// Máximo de passos (chamadas ao modelo) por mensagem.
pub const MAX_STEPS: usize = 25;
/// Mensagens mantidas por sessão (sistema + 29 últimas).
pub const MAX_SESSION_MESSAGES: usize = 30;
/// Arquivos maiores que isso são truncados nas primeiras 1000 linhas.
pub const READ_LIMIT_BYTES: u64 = 250 * 1024;
/// Limite de caracteres da saída de `run_command`.
pub const OUTPUT_LIMIT_CHARS: usize = 20_000;
/// Timeout de `run_command` (comandos comuns / `ask-agent`).
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(180);
pub const ASK_AGENT_TIMEOUT: Duration = Duration::from_secs(600);
/// Host padrão da API.
pub const DEFAULT_HOST: &str = "127.0.0.1:11434";

const NOT_RUNNING: &str = "Ollama não está rodando. Inicie o Ollama antes de chamar este agente.";

const ASK_AGENT_NOTE: &str = "\n\n[INSTRUÇÃO DO SISTEMA: O usuário já acompanha toda esta conversa em tempo real no painel \"Conversa integrada\" do CommandDeck. NÃO resuma nem relate esta resposta para o usuário e NÃO transfira dúvidas intermediárias a ele. Se a tarefa necessitar de mais discussão, esclarecimentos ou refinamento, CONTINUE conversando diretamente com o agente parceiro executando ask-agent novamente no run_command. Se já estiver alinhado, aplique as ferramentas necessárias no workspace (edit_file, write_file, run_command) até concluir a tarefa.]";

// ---------- Plataforma ----------

/// Nome do sistema operacional, para o prompt.
pub fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

/// Shell usado por `run_command`.
pub fn shell_name() -> &'static str {
    if cfg!(windows) {
        "PowerShell"
    } else {
        "sh"
    }
}

// ---------- HTTP ----------

/// URL base a partir de `host` (`127.0.0.1:11434`, `http://x:1234`, `0.0.0.0`, ...).
pub fn base_url(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    let (scheme, rest) = match h.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("http".to_owned(), h),
    };
    let (name, port) = match rest.rsplit_once(':') {
        Some((n, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (n, p),
        _ => (rest, "11434"),
    };
    // `OLLAMA_HOST=0.0.0.0` é endereço de escuta do servidor; o cliente usa o loopback.
    let name = match name {
        "" | "0.0.0.0" => "127.0.0.1",
        n => n,
    };
    format!("{scheme}://{name}:{port}")
}

/// Cliente HTTP para a API local (sem proxy do ambiente; erros HTTP não viram `Err`).
fn http_agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .proxy(None)
        .http_status_as_error(false)
        .timeout_global(timeout)
        .timeout_connect(Some(Duration::from_secs(5)))
        .build()
        .into()
}

fn map_http_error(e: ureq::Error) -> String {
    match &e {
        ureq::Error::ConnectionFailed => NOT_RUNNING.to_owned(),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => NOT_RUNNING.to_owned(),
        _ => e.to_string(),
    }
}

/// Modelos instalados via `GET /api/tags`. Erro = mensagem (ex.: Ollama não está rodando).
pub fn list_models(host: &str, timeout: Duration) -> Result<Vec<String>, String> {
    let url = format!("{}/api/tags", base_url(host));
    let mut resp = http_agent(Some(timeout)).get(&url).call().map_err(map_http_error)?;
    if resp.status().as_u16() != 200 {
        return Err(format!("Ollama retornou HTTP {}", resp.status().as_u16()));
    }
    let v: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let models = v
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "resposta inválida de /api/tags".to_owned())?
        .iter()
        .filter_map(|m| m.get("name").or_else(|| m.get("model")).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(models)
}

// ---------- Ferramentas ----------

/// Nome e descrição de cada ferramenta exposta ao modelo.
pub fn tool_list() -> Vec<(&'static str, String)> {
    vec![
        ("read_file", "Lê o conteúdo textual de um arquivo dentro do workspace.".into()),
        ("write_file", "Cria ou substitui completamente um arquivo no workspace com o novo conteúdo.".into()),
        ("edit_file", "Substitui cirurgicamente um trecho específico de texto em um arquivo existente no workspace.".into()),
        ("list_directory", "Lista arquivos e diretórios dentro do workspace.".into()),
        ("run_command", format!("Executa um comando no terminal ({}) dentro do diretório do workspace.", shell_name())),
        ("delete_file", "Remove um arquivo ou diretório do workspace.".into()),
    ]
}

/// Esquemas JSON das ferramentas (`tools` da API `/api/chat`).
pub fn tool_schemas() -> Value {
    let desc: HashMap<&str, String> = tool_list().into_iter().collect();
    let d = |n: &str| desc.get(n).cloned().unwrap_or_default();
    let f = |name: &str, props: Value, required: &[&str]| {
        let mut params = json!({ "type": "object", "properties": props });
        if !required.is_empty() {
            params["required"] = json!(required);
        }
        json!({ "type": "function", "function": { "name": name, "description": d(name), "parameters": params } })
    };
    json!([
        f("read_file", json!({ "path": { "type": "string", "description": "Caminho do arquivo (relativo ao workspace)" } }), &["path"]),
        f(
            "write_file",
            json!({
                "path": { "type": "string", "description": "Caminho do arquivo a ser gravado (relativo ao workspace)" },
                "content": { "type": "string", "description": "Conteúdo completo do arquivo" }
            }),
            &["path", "content"]
        ),
        f(
            "edit_file",
            json!({
                "path": { "type": "string", "description": "Caminho do arquivo a ser editado" },
                "target": { "type": "string", "description": "Trecho exato de código/texto a ser substituído" },
                "replacement": { "type": "string", "description": "Novo trecho que substituirá o alvo" }
            }),
            &["path", "target", "replacement"]
        ),
        f(
            "list_directory",
            json!({ "path": { "type": "string", "description": "Subdiretório relativo (ou \".\" para a raiz do workspace)" } }),
            &[]
        ),
        f(
            "run_command",
            json!({ "command": { "type": "string", "description": "Comando de terminal a ser executado (ex: git status, npm test, ask-agent <agente> \"mensagem\", python script.py)" } }),
            &["command"]
        ),
        f("delete_file", json!({ "path": { "type": "string", "description": "Caminho do arquivo ou pasta a ser excluído" } }), &["path"]),
    ])
}

/// Garante que o caminho acessado esteja estritamente dentro do workspace (normalização
/// lexical: funciona com caminhos inexistentes; case-insensitive no Windows).
pub fn resolve_workspace_path(rel_or_abs: &str, workspace: &Path) -> Result<PathBuf, String> {
    if workspace.as_os_str().is_empty() {
        return Err("Workspace não configurado.".into());
    }
    let ws = crate::util::absolutize(workspace);
    let target = crate::util::normalize_lexical(&ws.join(rel_or_abs));
    if !crate::util::path_within(&target, &ws) {
        return Err(format!("Acesso negado: o caminho \"{rel_or_abs}\" está fora do workspace configurado (\"{}\").", workspace.display()));
    }
    Ok(target)
}

const IMAGE_EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".ico", ".svgz"];
const BINARY_EXTS: &[&str] = &[
    ".pdf", ".zip", ".tar", ".gz", ".7z", ".rar", ".exe", ".dll", ".bin", ".so", ".dylib", ".iso", ".mp3", ".mp4", ".wav", ".avi", ".mov",
    ".webm", ".pyc", ".pyd", ".node",
];

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("parâmetro \"{key}\" ausente"))
}

/// Valor textual de um argumento opcional (`null`/ausente = vazio; outros tipos em JSON).
fn arg_text(args: &Value, key: &str) -> String {
    match args.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Contexto de execução das ferramentas.
pub struct ToolContext<'a> {
    pub workspace: &'a Path,
    pub cancel: &'a AtomicBool,
    /// Ambiente completo para `run_command` (`None` = herda o do processo).
    pub command_env: Option<&'a BTreeMap<String, String>>,
}

/// Executa uma ferramenta e devolve o texto para o modelo (erros viram texto, como no JS).
pub fn execute_tool(name: &str, args: &Value, ctx: &ToolContext<'_>) -> String {
    let res = match name {
        "read_file" => tool_read_file(args, ctx.workspace),
        "write_file" => tool_write_file(args, ctx.workspace),
        "edit_file" => tool_edit_file(args, ctx.workspace),
        "list_directory" => tool_list_directory(args, ctx.workspace),
        "run_command" => arg_str(args, "command").map(|c| run_command(c, ctx)),
        "delete_file" => tool_delete_file(args, ctx.workspace),
        _ => return format!("Ferramenta desconhecida: \"{name}\""),
    };
    res.unwrap_or_else(|e| format!("Falha ao executar {name}: {e}"))
}

fn tool_read_file(args: &Value, ws: &Path) -> Result<String, String> {
    let p = arg_str(args, "path")?;
    let full = resolve_workspace_path(p, ws)?;
    if !full.exists() {
        return Ok(format!("Erro: Arquivo \"{p}\" não encontrado no workspace."));
    }
    let meta = std::fs::metadata(&full).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Ok(format!("Erro: \"{p}\" é um diretório, use list_directory."));
    }
    let ext = ext_of(&full);
    let size = meta.len();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return Ok(format!(
            "[Arquivo de imagem detectado: \"{p}\" ({size} bytes)]. A ferramenta read_file opera apenas com texto UTF-8 e não decodifica imagens. Para analisar ou inspecionar esta imagem visualmente, DELEGUE a análise para o Antigravity executando: ask-agent antigravity \"Analise o arquivo de imagem {p} e descreva seu conteúdo/texto\" via run_command. Caso deseje inspecionar dimensões ou metadados localmente, execute um script em Python (Pillow) via run_command."
        ));
    }
    if BINARY_EXTS.contains(&ext.as_str()) {
        return Ok(format!(
            "[Arquivo binário detectado: \"{p}\" ({size} bytes)]. A ferramenta read_file opera apenas com texto UTF-8. Para inspecionar ou processar este arquivo, execute comandos ou scripts no terminal com run_command."
        ));
    }
    let bytes = std::fs::read(&full).map_err(|e| e.to_string())?;
    let content = String::from_utf8_lossy(&bytes);
    if size > READ_LIMIT_BYTES {
        // Arquivo muito grande: primeiras 1000 linhas.
        let lines: Vec<&str> = content.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).take(1000).collect();
        return Ok(format!("[Arquivo grande - exibindo primeiras 1000 linhas]\n{}\n... [conteúdo restante truncado]", lines.join("\n")));
    }
    Ok(content.into_owned())
}

fn tool_write_file(args: &Value, ws: &Path) -> Result<String, String> {
    let p = arg_str(args, "path")?;
    let full = resolve_workspace_path(p, ws)?;
    let content = arg_text(args, "content");
    if let Some(dir) = full.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&full, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(format!("Arquivo \"{p}\" gravado com sucesso ({} bytes).", content.len()))
}

fn tool_edit_file(args: &Value, ws: &Path) -> Result<String, String> {
    let p = arg_str(args, "path")?;
    let full = resolve_workspace_path(p, ws)?;
    if !full.exists() {
        return Ok(format!("Erro: Arquivo \"{p}\" não encontrado para edição."));
    }
    let target = arg_str(args, "target")?;
    let replacement = arg_text(args, "replacement");
    let bytes = std::fs::read(&full).map_err(|e| e.to_string())?;
    let content = String::from_utf8_lossy(&bytes);
    if !content.contains(target) {
        return Ok(format!(
            "Erro: Trecho alvo não encontrado no arquivo \"{p}\". Use read_file para verificar o conteúdo exato antes de editar."
        ));
    }
    let updated = content.replacen(target, &replacement, 1);
    std::fs::write(&full, updated.as_bytes()).map_err(|e| e.to_string())?;
    Ok(format!("Arquivo \"{p}\" editado com sucesso."))
}

fn tool_list_directory(args: &Value, ws: &Path) -> Result<String, String> {
    let rel = args.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(".");
    let full = resolve_workspace_path(rel, ws)?;
    if !full.exists() {
        return Ok(format!("Erro: Diretório \"{rel}\" não encontrado."));
    }
    let mut entries: Vec<_> = std::fs::read_dir(&full).map_err(|e| e.to_string())?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let list: Vec<String> = entries
        .iter()
        .filter(|e| ![".git", "node_modules", ".gemini"].contains(&e.file_name().to_string_lossy().as_ref()))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                format!("[DIR]  {name}/")
            } else {
                match std::fs::metadata(e.path()) {
                    Ok(m) => format!("[FILE] {name} ({} bytes)", m.len()),
                    Err(_) => format!("[FILE] {name}"),
                }
            }
        })
        .collect();
    Ok(if list.is_empty() { "(diretório vazio)".into() } else { list.join("\n") })
}

fn tool_delete_file(args: &Value, ws: &Path) -> Result<String, String> {
    let p = arg_str(args, "path")?;
    let full = resolve_workspace_path(p, ws)?;
    if crate::util::same_path(&full, &crate::util::absolutize(ws)) {
        return Err("Acesso negado: não é permitido excluir a raiz do workspace.".into());
    }
    if !full.exists() {
        return Ok(format!("Erro: \"{p}\" não existe."));
    }
    let res = if full.is_dir() { std::fs::remove_dir_all(&full) } else { std::fs::remove_file(&full) };
    res.map_err(|e| e.to_string())?;
    Ok(format!("\"{p}\" excluído com sucesso."))
}

/// Monta o processo do shell para um comando (`powershell -EncodedCommand` no Windows,
/// evitando problemas de aspas; `sh -c` nos demais).
fn shell_command(command: &str) -> std::process::Command {
    if cfg!(windows) {
        // Força UTF-8 no console e na saída do PowerShell para evitar "?" no lugar de acentos.
        let script = format!("$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(); {command}");
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoLogo", "-NonInteractive", "-EncodedCommand", &encoded]);
        cmd
    } else {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

fn run_command(command: &str, ctx: &ToolContext<'_>) -> String {
    let is_ask_agent = command.to_lowercase().contains("ask-agent");
    let timeout = if is_ask_agent { ASK_AGENT_TIMEOUT } else { COMMAND_TIMEOUT };
    let mut cmd = shell_command(command);
    cmd.current_dir(ctx.workspace);
    if let Some(env) = ctx.command_env {
        cmd.env_clear().envs(env);
    }
    let mut result = match crate::process::run_captured(cmd, None, timeout, Some(ctx.cancel), None) {
        Err(e) => format!("[Erro de execução: {e}]"),
        Ok(c) => {
            let out = c.stdout.trim();
            let err = c.stderr.trim();
            let mut r = String::from(out);
            if !err.is_empty() {
                if !r.is_empty() {
                    r.push_str("\n[stderr]\n");
                }
                r.push_str(err);
            }
            let tail = match c.exit {
                crate::process::Exit::Code(Some(0)) => None,
                crate::process::Exit::Code(code) => Some(format!("[Erro de execução: código {}]", code.unwrap_or(1))),
                crate::process::Exit::TimedOut => {
                    Some(format!("[Erro de execução: comando excedeu o tempo limite ({}s)]", timeout.as_secs()))
                }
                crate::process::Exit::Cancelled => Some("[Erro de execução: comando cancelado pelo usuário]".to_owned()),
            };
            if let Some(t) = tail {
                if !r.is_empty() {
                    r.push('\n');
                }
                r.push_str(&t);
            }
            r
        }
    };
    if result.is_empty() {
        result = "(comando executado sem saída)".into();
    }
    let (head, truncated) = crate::util::truncate_chars(&result, OUTPUT_LIMIT_CHARS);
    if truncated {
        result = format!("{head}\n... [saída truncada]");
    }
    if is_ask_agent {
        result.push_str(ASK_AGENT_NOTE);
    }
    result
}

/// Imagens dos anexos em base64 (para modelos com visão).
pub fn extract_base64_images(attachments: &[String], workspace: &Path) -> Vec<String> {
    const EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"];
    attachments
        .iter()
        .filter_map(|att| {
            let p = workspace.join(att);
            if !EXTS.contains(&ext_of(&p).as_str()) {
                return None;
            }
            std::fs::read(&p).ok().map(|b| base64::engine::general_purpose::STANDARD.encode(b))
        })
        .collect()
}

// ---------- Prompt de sistema ----------

/// Prompt de sistema (porta do `baseSystem` do ollamaAgent.js, com os fatos de app/SO/shell
/// corrigidos para a versão nativa) + AGENTS.md do workspace + instrução extra + contexto.
pub fn system_prompt(workspace: &Path, host: &str, extra_system: Option<&str>, chunks: &[ContextChunk]) -> String {
    let shell = shell_name();
    let base_system = [
        "Você é o Ollama Agent, um agente autônomo de inteligência artificial operando localmente dentro do CommandDeck Terminal.".to_owned(),
        "Você NÃO é um chatbot isolado e NÃO está em um CLI genérico ou sem contexto. Você tem ferramentas ativas e integração profunda com o sistema.".to_owned(),
        String::new(),
        "=== AMBIENTE E CONTEXTO OPERACIONAL ===".to_owned(),
        "* Aplicação: CommandDeck Terminal (projeto \"commanddeck\") — terminal multi-agente desktop nativo, escrito em Rust.".to_owned(),
        format!("* Execução: Modelo local rodando no servidor Ollama ({}). Você opera com total privacidade, segurança e sem custo de nuvem.", base_url(host).trim_start_matches("http://")),
        format!("* Workspace Atual: \"{}\". Você tem autorização completa do usuário para ler, criar, editar e testar qualquer arquivo nesta pasta.", workspace.display()),
        format!("* Sistema Operacional: {} (shell {shell}).", os_name()),
        String::new(),
        "=== SUAS FERRAMENTAS INTEGRADAS (FUNCTION CALLING) ===".to_owned(),
        "Você possui ferramentas reais ativas no workspace. Sempre que precisar realizar uma ação ou verificar o ambiente, EXECUTE a ferramenta:".to_owned(),
        "- list_directory: Inspeciona diretórios e lista arquivos. Se você não sabe o que há no projeto, execute list_directory(\".\") para mapear a estrutura.".to_owned(),
        "- read_file: Lê arquivos de texto e código (código-fonte, markdown, configs JSON/YAML, etc.).".to_owned(),
        "- write_file: Cria novos arquivos ou sobrescreve arquivos inteiros.".to_owned(),
        "- edit_file: Faz alterações pontuais e cirúrgicas em arquivos existentes (substituindo trechos específicos).".to_owned(),
        format!("- run_command: Executa qualquer comando no {shell} no diretório do workspace (npm, node, git, python, scripts, testes, etc.)."),
        "- delete_file: Exclui arquivos ou diretórios quando explicitamente instruído.".to_owned(),
        String::new(),
        "=== ARQUIVOS BINÁRIOS E IMAGENS (PNG, JPG, PDF, ETC.) ===".to_owned(),
        "* A ferramenta read_file é estritamente textual (UTF-8). Tentar ler imagens (.png, .jpg) ou binários com read_file causará erro ou conteúdo corrompido.".to_owned(),
        "* Para analisar imagens, diagramas, prints de tela ou PDFs visualmente, você deve DELEGAR a um agente multimodal com visão ativa:".to_owned(),
        "    Execute via run_command:".to_owned(),
        "    ask-agent antigravity \"Analise a imagem <caminho.png> e descreva o conteúdo/texto contido nela.\"".to_owned(),
        format!("* Para extrair metadados ou fazer OCR localmente, você pode executar scripts Python (Pillow/tesseract) ou comandos {shell} via run_command."),
        String::new(),
        "=== REDE DE AGENTES E COLABORAÇÃO (ask-agent) ===".to_owned(),
        "Você faz parte de uma equipe de agentes de elite no CommandDeck. Você pode consultar e colaborar com outros agentes a qualquer momento:".to_owned(),
        "- antigravity: Google Antigravity CLI — parceiro de ponta para visão multimodal (imagens), planejamento arquitetural, auditoria e tarefas complexas.".to_owned(),
        "- claude: Claude Code CLI — parceiro de ponta para refatoração e implementação profunda.".to_owned(),
        "Como chamar outro agente:".to_owned(),
        "  run_command: ask-agent <agente> \"sua pergunta ou instrução detalhada\"".to_owned(),
        "Exemplo: ask-agent antigravity \"Preciso de uma revisão de segurança neste script.\"".to_owned(),
        String::new(),
        "=== REGRAS OBRIGATÓRIAS DE COLABORAÇÃO COM OUTROS AGENTES (CRÍTICO) ===".to_owned(),
        "1. O USUÁRIO JÁ TEM ACESSO À CONVERSA INTEGRADA (NÃO PRECISA ME FALAR):".to_owned(),
        "   O usuário acompanha em tempo real todas as mensagens trocadas entre os agentes na janela/painel \"Conversa integrada\" do CommandDeck.".to_owned(),
        "   Por essa razão, você NUNCA deve relatar, retransmitir, resumir ou papagaiar para o usuário o que o outro agente (Antigravity, Claude, etc.) respondeu. O usuário já leu diretamente na interface!".to_owned(),
        "2. NÃO PARE PARA PERGUNTAR AO USUÁRIO — CONTINUE CONVERSANDO COM O AGENTE (ANTIGRAVITY):".to_owned(),
        "   Quando você estiver colaborando com o Antigravity (ou outro agente), NÃO pare a execução para ficar perguntando ao usuário coisas como: \"o que você acha?\", \"como você prefere fazer?\", \"devo aplicar o que ele falou?\", \"qual opção você escolhe?\".".to_owned(),
        "   Em vez de fazer perguntas ao usuário, CONTINUE A CONVERSA DIRETAMENTE COM O ANTIGRAVITY! Use ask-agent antigravity novamente no run_command para tirar dúvidas, debater a solução, pedir complementos ou alinhar a implementação. Resolvam entre vocês!".to_owned(),
        "3. EXECUÇÃO AUTÔNOMA NO WORKSPACE:".to_owned(),
        "   Assim que você e o agente parceiro chegarem ao alinhamento técnico, aplique o código ou alterações diretamente com write_file, edit_file ou run_command. Não transfira para o usuário dúvidas técnicas que os agentes podem resolver juntos.".to_owned(),
        "4. QUANDO FALAR COM O USUÁRIO:".to_owned(),
        "   Fale com o usuário APENAS no final de tudo, de forma concisa e focada, avisando que a tarefa foi concluída com sucesso. Não reconte a conversa que o usuário já acompanhou na Conversa integrada.".to_owned(),
        String::new(),
        "=== DIRETRIZES DE POSTURA E AUTONOMIA ===".to_owned(),
        "1. NUNCA diga \"sou apenas um modelo de linguagem\", \"não tenho acesso ao sistema de arquivos\" ou \"não posso executar comandos\". Você é um agente operacional com ferramentas ativas. Use-as!".to_owned(),
        "2. SEJA PROATIVO: Se o usuário pedir algo sobre o projeto e você não souber onde está, use list_directory ou leia arquivos como README.md, package.json, etc. Nunca fique perdido ou passivo.".to_owned(),
        "3. AÇÃO SOBRE CONVERSAÇÃO: Quando o usuário pedir para criar, modificar código ou verificar o ambiente, execute a ferramenta imediatamente em vez de apenas explicar o que faria.".to_owned(),
        "4. ANTES DE EDITAR: Leia o arquivo primeiro com read_file para garantir que o trecho a ser alterado esteja correto.".to_owned(),
        "5. CONCISÃO E FOCO NO RESULTADO: Seja direto, técnico e objetivo ao responder ao usuário. Jamais resuma a conversa inter-agente já exibida na Conversa integrada.".to_owned(),
        "6. PERSISTÊNCIA EM EQUIPE: Ao interagir com o Antigravity ou outro agente, continue o loop de colaboração com ele até solucionar o problema por completo. Jamais transfira o diálogo de volta para o usuário com perguntas intermediárias.".to_owned(),
    ]
    .join("\n");

    let mut system = base_system;
    let agents_md = std::fs::read_to_string(workspace.join("AGENTS.md")).map(|t| t.trim().to_owned()).unwrap_or_default();
    if !agents_md.is_empty() {
        system.push_str(&format!("\n\n=== REGRAS DO WORKSPACE (AGENTS.md) ===\n{agents_md}"));
    }
    if let Some(extra) = extra_system.filter(|s| !s.is_empty()) {
        system.push_str(&format!("\n\n=== INSTRUÇÃO ADICIONAL ===\n{extra}"));
    }
    let ctx = context::render(chunks);
    if !ctx.is_empty() {
        system.push_str("\n\n");
        system.push_str(&ctx);
    }
    system
}

// ---------- Motor ----------

/// Parâmetros de uma execução do agente.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub model: String,
    pub prompt: String,
    pub workspace: PathBuf,
    /// Chave da conversa em memória (`None` = sem histórico).
    pub session_key: Option<String>,
    pub extra_system: Option<String>,
    /// Referências de anexos (imagens viram `images` em base64).
    pub attachments: Vec<String>,
    pub max_steps: usize,
    /// `true` = NDJSON com [`AgentEvent::ContentDelta`] token a token.
    pub stream: bool,
    /// Ambiente completo para `run_command` (`None` = herda o do processo).
    pub command_env: Option<BTreeMap<String, String>>,
}

impl RunOptions {
    pub fn new(model: impl Into<String>, prompt: impl Into<String>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            workspace: workspace.into(),
            session_key: None,
            extra_system: None,
            attachments: Vec::new(),
            max_steps: MAX_STEPS,
            stream: false,
            command_env: None,
        }
    }
}

/// Progresso de uma execução.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// O modelo pediu uma ferramenta (`summary` = caminho, comando ou argumentos).
    ToolCall { name: String, summary: String },
    /// Resultado da ferramenta (início do texto, para exibição).
    ToolResult { name: String, preview: String },
    /// Trecho da resposta (apenas com `stream: true`).
    ContentDelta(String),
    /// Trecho de raciocínio (`thinking`) do modelo (apenas com `stream: true`).
    ThinkingDelta(String),
    /// Fim da execução.
    Done { steps: usize, tool_calls: usize, cancelled: bool },
}

/// Resultado de uma execução.
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// Resposta final (ou `(tarefa concluída)` quando o modelo não deu texto).
    pub text: String,
    pub steps: usize,
    pub tool_calls: usize,
    /// Interrompida pelo `cancel` (o texto é parcial).
    pub cancelled: bool,
}

/// Tamanho de uma conversa em memória.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub messages: usize,
    /// Estimativa grosseira (caracteres / 4).
    pub approx_tokens: usize,
}

/// Motor do agente Ollama, com as conversas em memória por `session_key`.
pub struct OllamaAgent {
    host: RwLock<String>,
    sessions: Mutex<HashMap<String, Vec<Value>>>,
    providers: Providers,
}

impl OllamaAgent {
    pub fn new(host: impl Into<String>) -> Self {
        Self::with_providers(host, Providers::default())
    }

    pub fn with_providers(host: impl Into<String>, providers: Providers) -> Self {
        Self { host: RwLock::new(host.into()), sessions: Mutex::new(HashMap::new()), providers }
    }

    pub fn host(&self) -> String {
        self.host.read().clone()
    }

    pub fn set_host(&self, host: impl Into<String>) {
        *self.host.write() = host.into();
    }

    /// Esquece a conversa `key`.
    pub fn clear_session(&self, key: &str) {
        self.sessions.lock().remove(key);
    }

    pub fn session_stats(&self, key: &str) -> SessionStats {
        let sessions = self.sessions.lock();
        let Some(msgs) = sessions.get(key) else { return SessionStats::default() };
        let chars: usize = msgs.iter().map(|m| m.get("content").and_then(Value::as_str).map_or(0, |s| s.chars().count())).sum();
        SessionStats { messages: msgs.len(), approx_tokens: chars / 4 }
    }

    /// Loop completo do agente: chama o modelo e executa ferramentas até a resposta final
    /// (ou `max_steps`). `cancel` é verificado entre passos, entre ferramentas e entre
    /// trechos do streaming.
    pub fn run(&self, opts: RunOptions, cancel: &AtomicBool, on_event: &mut dyn FnMut(AgentEvent)) -> anyhow::Result<RunOutput> {
        let host = self.host();
        let chunks =
            context::gather(&self.providers, &ContextRequest { agent: "ollama", prompt: &opts.prompt, workspace: &opts.workspace });
        let system = system_prompt(&opts.workspace, &host, opts.extra_system.as_deref(), &chunks);

        let mut messages = opts.session_key.as_ref().and_then(|k| self.sessions.lock().get(k).cloned()).unwrap_or_default();
        // Atualiza o prompt de sistema da sessão com as diretrizes mais recentes.
        match messages.first_mut() {
            Some(first) if first.get("role").and_then(Value::as_str) == Some("system") => first["content"] = json!(system),
            _ => messages.insert(0, json!({ "role": "system", "content": system })),
        }
        let mut user = json!({ "role": "user", "content": opts.prompt });
        let images = extract_base64_images(&opts.attachments, &opts.workspace);
        if !images.is_empty() {
            user["images"] = json!(images);
        }
        messages.push(user);

        let result = self.run_loop(&host, &opts, &mut messages, cancel, on_event);

        // Mantém no máximo as últimas 30 mensagens na sessão para não estourar o contexto.
        if let Some(key) = &opts.session_key {
            if messages.len() > MAX_SESSION_MESSAGES {
                let tail = messages.split_off(messages.len() - (MAX_SESSION_MESSAGES - 1));
                messages.truncate(1);
                messages.extend(tail);
            }
            self.sessions.lock().insert(key.clone(), messages);
        }
        if let Ok(out) = &result {
            on_event(AgentEvent::Done { steps: out.steps, tool_calls: out.tool_calls, cancelled: out.cancelled });
        }
        result
    }

    fn run_loop(
        &self,
        host: &str,
        opts: &RunOptions,
        messages: &mut Vec<Value>,
        cancel: &AtomicBool,
        on_event: &mut dyn FnMut(AgentEvent),
    ) -> anyhow::Result<RunOutput> {
        let tools = tool_schemas();
        let agent = http_agent(None);
        let url = format!("{}/api/chat", base_url(host));
        let tool_ctx = ToolContext { workspace: &opts.workspace, cancel, command_env: opts.command_env.as_ref() };
        let mut out = RunOutput { text: String::new(), steps: 0, tool_calls: 0, cancelled: false };

        while out.steps < opts.max_steps {
            if cancel.load(Ordering::SeqCst) {
                out.cancelled = true;
                return Ok(out);
            }
            out.steps += 1;
            let payload = json!({ "model": opts.model, "stream": opts.stream, "messages": messages, "tools": tools });
            let (assistant, cancelled) = if opts.stream {
                chat_stream(&agent, &url, &payload, cancel, on_event)?
            } else {
                (chat_once(&agent, &url, &payload)?, false)
            };
            let Some(assistant) = assistant else { break };
            messages.push(assistant.clone());
            if cancelled {
                out.cancelled = true;
                out.text = assistant.get("content").and_then(Value::as_str).unwrap_or("").trim().to_owned();
                return Ok(out);
            }

            let calls = assistant.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
            if calls.is_empty() {
                // O modelo concluiu a resposta final.
                out.text = assistant.get("content").and_then(Value::as_str).unwrap_or("").trim().to_owned();
                break;
            }
            for call in calls {
                let func = call.get("function").cloned().unwrap_or(Value::Null);
                let name = func.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
                let args = match func.get("arguments") {
                    Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                    Some(v) => v.clone(),
                    None => json!({}),
                };
                if cancel.load(Ordering::SeqCst) {
                    // Mantém o histórico consistente: toda chamada recebe uma resposta.
                    messages.push(json!({ "role": "tool", "name": name, "content": "[cancelado pelo usuário]" }));
                    out.cancelled = true;
                    continue;
                }
                let summary = summarize_args(&args);
                on_event(AgentEvent::ToolCall { name: name.clone(), summary });
                out.tool_calls += 1;
                let result = execute_tool(&name, &args, &tool_ctx);
                let (preview, _) = crate::util::truncate_chars(result.trim(), 300);
                on_event(AgentEvent::ToolResult { name: name.clone(), preview: preview.to_owned() });
                messages.push(json!({ "role": "tool", "name": name, "content": result }));
            }
            if out.cancelled {
                return Ok(out);
            }
        }
        if out.text.is_empty() {
            out.text = "(tarefa concluída)".into();
        }
        Ok(out)
    }
}

/// `toolArgs.path || toolArgs.command || JSON.stringify(toolArgs)`.
fn summarize_args(args: &Value) -> String {
    for key in ["path", "command"] {
        if let Some(s) = args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            return s.to_owned();
        }
    }
    args.to_string()
}

fn chat_once(agent: &ureq::Agent, url: &str, payload: &Value) -> anyhow::Result<Option<Value>> {
    let mut resp = agent.post(url).send_json(payload).map_err(|e| anyhow::anyhow!(map_http_error(e)))?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().with_config().limit(256 * 1024 * 1024).read_to_string().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if status != 200 {
        anyhow::bail!("Ollama retornou HTTP {status}: {body}");
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| anyhow::anyhow!("Resposta inválida do Ollama: {body}"))?;
    Ok(v.get("message").filter(|m| m.is_object()).cloned())
}

/// Lê a resposta NDJSON, emitindo deltas. Retorna a mensagem montada e se foi cancelada.
fn chat_stream(
    agent: &ureq::Agent,
    url: &str,
    payload: &Value,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(AgentEvent),
) -> anyhow::Result<(Option<Value>, bool)> {
    let mut resp = agent.post(url).send_json(payload).map_err(|e| anyhow::anyhow!(map_http_error(e)))?;
    let status = resp.status().as_u16();
    if status != 200 {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("Ollama retornou HTTP {status}: {body}");
    }
    let reader = std::io::BufReader::new(resp.into_body().into_reader());
    let mut content = String::new();
    let mut thinking = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut got_any = false;
    let mut cancelled = false;
    for line in reader.lines() {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let line = line.map_err(|e| anyhow::anyhow!("falha ao ler a resposta do Ollama: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|_| anyhow::anyhow!("Resposta inválida do Ollama: {line}"))?;
        if let Some(err) = v.get("error") {
            let msg = err.as_str().map(str::to_owned).unwrap_or_else(|| err.to_string());
            anyhow::bail!("Ollama retornou erro: {msg}");
        }
        if let Some(msg) = v.get("message") {
            got_any = true;
            if let Some(t) = msg.get("thinking").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                thinking.push_str(t);
                on_event(AgentEvent::ThinkingDelta(t.to_owned()));
            }
            if let Some(c) = msg.get("content").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                content.push_str(c);
                on_event(AgentEvent::ContentDelta(c.to_owned()));
            }
            if let Some(calls) = msg.get("tool_calls").and_then(Value::as_array) {
                tool_calls.extend(calls.iter().cloned());
            }
        }
        if v.get("done").and_then(Value::as_bool) == Some(true) {
            break;
        }
    }
    if !got_any && !cancelled {
        return Ok((None, false));
    }
    let mut msg = json!({ "role": "assistant", "content": content });
    if !thinking.is_empty() {
        msg["thinking"] = json!(thinking);
    }
    if !tool_calls.is_empty() && !cancelled {
        msg["tool_calls"] = json!(tool_calls);
    }
    Ok((Some(msg), cancelled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(ws: &'a Path, cancel: &'a AtomicBool) -> ToolContext<'a> {
        ToolContext { workspace: ws, cancel, command_env: None }
    }

    #[test]
    fn base_url_variants() {
        assert_eq!(base_url("127.0.0.1:11434"), "http://127.0.0.1:11434");
        assert_eq!(base_url("http://localhost:1234/"), "http://localhost:1234");
        assert_eq!(base_url("0.0.0.0"), "http://127.0.0.1:11434");
        assert_eq!(base_url("0.0.0.0:9999"), "http://127.0.0.1:9999");
        assert_eq!(base_url("https://ollama.example.com"), "https://ollama.example.com:11434");
    }

    #[test]
    fn sandbox_resolution() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).expect("mkdir");
        assert_eq!(resolve_workspace_path("a/b.txt", &ws).expect("ok"), ws.join("a").join("b.txt"));
        assert_eq!(resolve_workspace_path("novo/inexistente/../x.txt", &ws).expect("ok"), ws.join("novo").join("x.txt"));
        assert_eq!(resolve_workspace_path(".", &ws).expect("ok"), ws);
        assert_eq!(resolve_workspace_path(&ws.join("in.txt").to_string_lossy(), &ws).expect("ok"), ws.join("in.txt"));
        for bad in ["../fora.txt", "a/../../fora", "../ws-irmao/x", "../../../../../../etc/passwd"] {
            let err = resolve_workspace_path(bad, &ws).expect_err(bad);
            assert!(err.starts_with("Acesso negado: o caminho"), "{err}");
        }
        let outside = tmp.path().join("fora.txt");
        assert!(resolve_workspace_path(&outside.to_string_lossy(), &ws).is_err());
        #[cfg(unix)]
        assert!(resolve_workspace_path("/etc/passwd", &ws).is_err());
        #[cfg(windows)]
        {
            let upper = ws.to_string_lossy().to_uppercase();
            assert!(resolve_workspace_path(&format!("{upper}\\x.txt"), &ws).is_ok());
            assert!(resolve_workspace_path("C:\\Windows\\win.ini", &ws).is_err());
        }
    }

    #[test]
    fn file_tools() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path();
        let cancel = AtomicBool::new(false);
        let c = ctx(ws, &cancel);
        let r = execute_tool("write_file", &json!({"path": "sub/a.txt", "content": "olá mundo"}), &c);
        assert_eq!(r, "Arquivo \"sub/a.txt\" gravado com sucesso (10 bytes).");
        assert_eq!(execute_tool("read_file", &json!({"path": "sub/a.txt"}), &c), "olá mundo");
        let r = execute_tool("edit_file", &json!({"path": "sub/a.txt", "target": "mundo", "replacement": "$1 rust"}), &c);
        assert_eq!(r, "Arquivo \"sub/a.txt\" editado com sucesso.");
        assert_eq!(std::fs::read_to_string(ws.join("sub/a.txt")).expect("read"), "olá $1 rust");
        let r = execute_tool("edit_file", &json!({"path": "sub/a.txt", "target": "xyz", "replacement": "1"}), &c);
        assert!(r.starts_with("Erro: Trecho alvo não encontrado"));
        assert_eq!(execute_tool("read_file", &json!({"path": "sub"}), &c), "Erro: \"sub\" é um diretório, use list_directory.");
        assert_eq!(execute_tool("read_file", &json!({"path": "nada.txt"}), &c), "Erro: Arquivo \"nada.txt\" não encontrado no workspace.");
        std::fs::write(ws.join("f.PNG"), [0u8; 4]).expect("write");
        assert!(execute_tool("read_file", &json!({"path": "f.PNG"}), &c).starts_with("[Arquivo de imagem detectado: \"f.PNG\" (4 bytes)]"));
        std::fs::write(ws.join("x.zip"), [0u8; 4]).expect("write");
        assert!(execute_tool("read_file", &json!({"path": "x.zip"}), &c).starts_with("[Arquivo binário detectado"));

        std::fs::create_dir_all(ws.join(".git")).expect("mkdir");
        let list = execute_tool("list_directory", &json!({}), &c);
        assert_eq!(list, "[FILE] f.PNG (4 bytes)\n[DIR]  sub/\n[FILE] x.zip (4 bytes)");
        assert_eq!(execute_tool("list_directory", &json!({"path": "nao"}), &c), "Erro: Diretório \"nao\" não encontrado.");

        assert_eq!(execute_tool("delete_file", &json!({"path": "sub"}), &c), "\"sub\" excluído com sucesso.");
        assert!(!ws.join("sub").exists());
        assert!(execute_tool("delete_file", &json!({"path": "."}), &c).starts_with("Falha ao executar delete_file: Acesso negado"));
        assert!(execute_tool("read_file", &json!({"path": "../x"}), &c).starts_with("Falha ao executar read_file: Acesso negado"));
        assert_eq!(execute_tool("voar", &json!({}), &c), "Ferramenta desconhecida: \"voar\"");
    }

    #[test]
    fn large_file_is_truncated() {
        let tmp = tempfile::tempdir().expect("tmp");
        let line = "x".repeat(299);
        let text: String = (0..1200).map(|i| format!("{i}{line}\r\n")).collect();
        std::fs::write(tmp.path().join("big.txt"), &text).expect("write");
        let cancel = AtomicBool::new(false);
        let r = execute_tool("read_file", &json!({"path": "big.txt"}), &ctx(tmp.path(), &cancel));
        assert!(r.starts_with("[Arquivo grande - exibindo primeiras 1000 linhas]\n0x"));
        assert!(r.ends_with("\n... [conteúdo restante truncado]"));
        assert_eq!(r.lines().count(), 1002);
    }

    #[cfg(unix)]
    #[test]
    fn run_command_output_and_errors() {
        let tmp = tempfile::tempdir().expect("tmp");
        let cancel = AtomicBool::new(false);
        let c = ctx(tmp.path(), &cancel);
        assert_eq!(execute_tool("run_command", &json!({"command": "echo oi; echo erro >&2"}), &c), "oi\n[stderr]\nerro");
        assert_eq!(execute_tool("run_command", &json!({"command": "exit 4"}), &c), "[Erro de execução: código 4]");
        assert_eq!(execute_tool("run_command", &json!({"command": "true"}), &c), "(comando executado sem saída)");
        let r = execute_tool("run_command", &json!({"command": "echo ask-agent"}), &c);
        assert!(r.starts_with("ask-agent\n\n[INSTRUÇÃO DO SISTEMA:"));
        let r = execute_tool("run_command", &json!({"command": "head -c 25000 /dev/zero | tr '\\0' a"}), &c);
        assert!(r.ends_with("\n... [saída truncada]"));
        assert_eq!(r.chars().count(), OUTPUT_LIMIT_CHARS + "\n... [saída truncada]".chars().count());
    }

    #[test]
    fn images_from_attachments() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join("a.png"), b"abc").expect("write");
        std::fs::write(tmp.path().join("b.txt"), b"abc").expect("write");
        let imgs = extract_base64_images(&["a.png".into(), "b.txt".into(), "nao.jpg".into()], tmp.path());
        assert_eq!(imgs, ["YWJj"]);
    }

    #[test]
    fn system_prompt_has_facts_and_agents_md() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join("AGENTS.md"), "  regra X  \n").expect("write");
        let chunks = [ContextChunk { source: "doc.md".into(), text: "trecho".into() }];
        let s = system_prompt(tmp.path(), "127.0.0.1:11434", Some("extra Y"), &chunks);
        assert!(s.contains(&format!("* Sistema Operacional: {} (shell {}).", os_name(), shell_name())));
        assert!(s.contains("nativo, escrito em Rust"));
        assert!(s.contains("=== REGRAS DO WORKSPACE (AGENTS.md) ===\nregra X"));
        assert!(s.contains("=== INSTRUÇÃO ADICIONAL ===\nextra Y"));
        assert!(s.ends_with("=== CONTEXTO ADICIONAL ===\n[doc.md]\ntrecho"));
        assert!(!s.contains("Electron"));
    }

    #[test]
    fn tool_schema_shape() {
        let t = tool_schemas();
        let names: Vec<&str> = t.as_array().expect("arr").iter().filter_map(|x| x["function"]["name"].as_str()).collect();
        assert_eq!(names, ["read_file", "write_file", "edit_file", "list_directory", "run_command", "delete_file"]);
        assert!(t[3]["function"]["parameters"].get("required").is_none());
        assert_eq!(t[2]["function"]["parameters"]["required"], json!(["path", "target", "replacement"]));
    }

    /// Servidor Ollama falso: 1ª chamada pede `list_directory`; depois responde texto.
    fn mock_ollama() -> (String, std::thread::JoinHandle<Vec<Value>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let port = server.server_addr().to_ip().expect("ip").port();
        let h = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for _ in 0..4 {
                let Ok(Some(mut req)) = server.recv_timeout(Duration::from_secs(10)) else { break };
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let has_tool = v["messages"].as_array().and_then(|m| m.last()).is_some_and(|x| x["role"] == "tool");
                let stream = v["stream"] == json!(true);
                let reply = match (has_tool, stream) {
                    (false, true) => [
                        json!({"message": {"role": "assistant", "content": "", "thinking": "hmm"}, "done": false}),
                        json!({"message": {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "list_directory", "arguments": {"path": "."}}}]}, "done": false}),
                        json!({"message": {"role": "assistant", "content": ""}, "done": true}),
                    ]
                    .iter()
                    .map(|l| format!("{l}\n"))
                    .collect::<String>(),
                    (true, true) => [
                        json!({"message": {"role": "assistant", "content": "Pron"}, "done": false}),
                        json!({"message": {"role": "assistant", "content": "to."}, "done": false}),
                        json!({"message": {"role": "assistant", "content": ""}, "done": true}),
                    ]
                    .iter()
                    .map(|l| format!("{l}\n"))
                    .collect::<String>(),
                    (false, false) => json!({"message": {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "read_file", "arguments": "{\"path\":\"a.txt\"}"}}]}}).to_string(),
                    (true, false) => json!({"message": {"role": "assistant", "content": "  Feito.  "}}).to_string(),
                };
                bodies.push(v);
                let _ = req.respond(tiny_http::Response::from_string(reply));
            }
            bodies
        });
        (format!("127.0.0.1:{port}"), h)
    }

    #[test]
    fn engine_streaming_and_sessions() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join("a.txt"), "conteúdo").expect("write");
        let (host, h) = mock_ollama();
        let agent = OllamaAgent::new(host);
        let cancel = AtomicBool::new(false);

        let mut opts = RunOptions::new("m", "liste", tmp.path());
        opts.session_key = Some("k".into());
        opts.stream = true;
        let mut events = Vec::new();
        let out = agent.run(opts, &cancel, &mut |e| events.push(e)).expect("run");
        assert_eq!(out.text, "Pronto.");
        assert_eq!((out.steps, out.tool_calls, out.cancelled), (2, 1, false));
        assert!(matches!(&events[0], AgentEvent::ThinkingDelta(t) if t == "hmm"));
        assert!(matches!(&events[1], AgentEvent::ToolCall { name, summary } if name == "list_directory" && summary == "."));
        assert!(matches!(&events[2], AgentEvent::ToolResult { preview, .. } if preview.contains("a.txt")));
        assert!(matches!(&events[3], AgentEvent::ContentDelta(t) if t == "Pron"));
        assert!(matches!(events.last(), Some(AgentEvent::Done { steps: 2, tool_calls: 1, cancelled: false })));
        // sistema, usuário, assistente (tool_calls), tool, assistente.
        assert_eq!(agent.session_stats("k").messages, 5);

        // Sem streaming (modo da ponte), argumentos como string JSON, mesma sessão.
        let mut opts = RunOptions::new("m", "leia", tmp.path());
        opts.session_key = Some("k".into());
        let out = agent.run(opts, &cancel, &mut |_| {}).expect("run");
        assert_eq!(out.text, "Feito.");
        assert_eq!(agent.session_stats("k").messages, 9);

        let bodies = h.join().expect("mock");
        assert_eq!(bodies.len(), 4);
        // O histórico foi reenviado e o prompt de sistema veio primeiro.
        let last = bodies[3]["messages"].as_array().expect("msgs");
        assert_eq!(last[0]["role"], "system");
        assert_eq!(last[5]["content"], "leia");
        assert_eq!(last.last().expect("tool")["content"], "conteúdo");
        assert_eq!(bodies[0]["tools"].as_array().map(Vec::len), Some(6));

        agent.clear_session("k");
        assert_eq!(agent.session_stats("k"), SessionStats::default());

        // Cancelado antes de começar: não chama o servidor.
        cancel.store(true, Ordering::SeqCst);
        let out = agent.run(RunOptions::new("m", "x", tmp.path()), &cancel, &mut |_| {}).expect("run");
        assert!(out.cancelled);
    }

    #[test]
    fn unreachable_ollama_gives_friendly_error() {
        // Porta 9 (discard) em loopback: conexão recusada.
        let agent = OllamaAgent::new("127.0.0.1:9");
        let cancel = AtomicBool::new(false);
        let err = agent.run(RunOptions::new("m", "oi", std::env::temp_dir()), &cancel, &mut |_| {}).expect_err("deve falhar");
        assert_eq!(err.to_string(), NOT_RUNNING);
        assert!(list_models("127.0.0.1:9", Duration::from_millis(500)).is_err());
    }
}
