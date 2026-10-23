//! Uso do Antigravity: cota consultada via API interna do Google Cloud Code.
//!
//! # Fonte e Endpoints
//! O Antigravity CLI (`agy`) utiliza a API interna `cloudcode-pa.googleapis.com`
//! (com fallback para `daily-cloudcode-pa.googleapis.com`):
//! - `POST /v1internal:retrieveUserQuotaSummary`: endpoint principal de cota,
//!   retorna os limites por grupo (ex.: "Gemini Models") divididos em janelas:
//!   - `window: "5h"` (limite da sessão de 5 horas);
//!   - `window: "weekly"` (limite semanal).
//!     Cada bucket traz `remainingFraction` (0.0..=1.0) e `resetTime` (RFC 3339).
//! - `POST /v1internal:fetchAvailableModels`: endpoint alternativo que lista os
//!   modelos disponíveis e seus respectivos `quotaInfo.{remainingFraction, resetTime}`.
//!
//! O servidor escolhe o que devolve pelo `User-Agent`: só para `antigravity/cli/<versão>` de um CLI
//! recente (1.4.5 sim, 1.0.0 não) vêm os grupos do Gemini ("Gemini Models", "Gemini 4 Argon"); para
//! qualquer outro vem só "Claude and GPT models". Por isso o pedido imita o CLI, com a versão instalada.
//! Dos grupos, valem os do Gemini (é para isso que o Antigravity é usado aqui), cada janela com o de
//! maior uso.
//!
//! # Credenciais
//! No Windows, o CLI armazena o token ativo no Gerenciador de Credenciais
//! do Windows (`gemini:antigravity`). Em outros sistemas ou como fallback,
//! lê `~/.gemini/oauth_creds.json`.
//! Só o `access_token` válido é usado: quem o renova é o próprio CLI (enquanto roda).
//! Vencido, a cota fica "sem dados" até ele renovar; o app não guarda credenciais OAuth
//! de terceiros nem renova tokens por conta própria.
//!
//! ATENÇÃO: Esta é uma API interna do Google Cloud e seu formato pode mudar entre versões.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::{epoch_ms, AgentUsage, UsageGroup, UsageWindow};
use crate::util;

/// Endpoint de produção e diário da API Cloud Code.
const API_HOSTS: &[&str] = &["cloudcode-pa.googleapis.com", "daily-cloudcode-pa.googleapis.com"];

/// Timeout para as requisições HTTP da cota.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Versão do CLI no `User-Agent` quando não dá para perguntar ao `agy` instalado (uma conferida que
/// recebe os grupos do Gemini).
const FALLBACK_CLI_VERSION: &str = "1.4.5";

/// O `User-Agent` do CLI (`antigravity/cli/1.4.5 (aidev_client; os_type=windows; arch=amd64)`).
fn user_agent(version: Option<&str>) -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    let os = if cfg!(target_os = "macos") { "darwin" } else { std::env::consts::OS };
    format!("antigravity/cli/{} (aidev_client; os_type={os}; arch={arch})", version.unwrap_or(FALLBACK_CLI_VERSION))
}

/// A versão na saída de `agy --version` ("1.4.5").
fn parse_version(out: &str) -> Option<String> {
    out.split_whitespace()
        .map(|t| t.trim_start_matches('v'))
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.') && t.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .map(str::to_owned)
}

/// A versão do `agy` instalado (`None` se não rodar ou não disser). Bloqueante (~0,2 s).
pub(crate) fn installed_version(agy: &crate::agents::Resolved) -> Option<String> {
    let (program, args) = agy.command();
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).arg("--version");
    let out = crate::process::run_captured(cmd, None, Duration::from_secs(10), None, None).ok()?;
    parse_version(&out.stdout)
}

#[cfg(windows)]
mod wincred {
    use std::ffi::c_void;

    #[repr(C)]
    struct CredentialW {
        flags: u32,
        cred_type: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: [u32; 2],
        credential_blob_size: u32,
        credential_blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn CredReadW(target_name: *const u16, cred_type: u32, flags: u32, credential: *mut *mut CredentialW) -> i32;
        fn CredFree(buffer: *mut c_void);
    }

    pub fn read_generic(target: &str) -> Option<String> {
        let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        let mut cred_ptr: *mut CredentialW = std::ptr::null_mut();
        // CRED_TYPE_GENERIC = 1
        // SAFETY: `wide` termina em NUL e vive até o fim da chamada; `cred_ptr` só é lido se ela der certo.
        let ok = unsafe { CredReadW(wide.as_ptr(), 1, 0, &mut cred_ptr) };
        if ok == 0 || cred_ptr.is_null() {
            return None;
        }
        // SAFETY: `cred_ptr` aponta para a CREDENTIALW alocada por CredReadW, com `credential_blob_size` bytes no blob.
        let res = unsafe {
            let size = (*cred_ptr).credential_blob_size as usize;
            if size == 0 || (*cred_ptr).credential_blob.is_null() {
                None
            } else {
                let slice = std::slice::from_raw_parts((*cred_ptr).credential_blob, size);
                String::from_utf8(slice.to_vec()).ok()
            }
        };
        // SAFETY: libera uma única vez o que CredReadW alocou; o blob já foi copiado.
        unsafe {
            CredFree(cred_ptr as *mut c_void);
        }
        res
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawCreds {
    access_token: String,
    expires_at_ms: Option<u64>,
}

impl RawCreds {
    fn is_valid(&self, now_ms: u64) -> bool {
        !self.access_token.is_empty() && self.expires_at_ms.is_none_or(|exp| exp > now_ms.saturating_add(60_000))
    }
}

fn parse_creds_json(json_str: &str) -> Option<RawCreds> {
    let v: Value = serde_json::from_str(json_str).ok()?;
    // Formato do keyring (Go): {"token": {"access_token": ..., "expiry": ...}}
    if let Some(token_obj) = v.get("token").filter(|t| t.is_object()) {
        let access_token = token_obj.get("access_token")?.as_str()?.to_owned();
        let expires_at_ms = token_obj.get("expiry").and_then(epoch_ms);
        return Some(RawCreds { access_token, expires_at_ms });
    }
    // Formato de ~/.gemini/oauth_creds.json
    let access_token = v.get("access_token")?.as_str()?.to_owned();
    let expires_at_ms = v.get("expiry_date").or_else(|| v.get("expiry")).and_then(epoch_ms);
    Some(RawCreds { access_token, expires_at_ms })
}

/// Escolhe o primeiro `access_token` ainda válido por mais de um minuto entre as fontes candidatas.
fn first_valid_token<I>(sources: I, now_ms: u64) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    sources.into_iter().filter_map(|s| parse_creds_json(&s)).find(|c| c.is_valid(now_ms)).map(|c| c.access_token)
}

fn credential_sources(home: &Path) -> Vec<String> {
    let mut out = Vec::new();
    // O Credential Manager é do usuário do sistema: com uma home isolada (testes, `StartOptions`)
    // ele não entra.
    #[cfg(windows)]
    if dirs::home_dir().is_some_and(|h| util::same_path(&h, home)) {
        if let Some(json_str) = wincred::read_generic("gemini:antigravity") {
            out.push(json_str);
        }
    }

    // Fallback: ~/.gemini/oauth_creds.json na pasta home passada
    let path = home.join(".gemini").join("oauth_creds.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        out.push(content);
    }

    // Fallback adicional: home do sistema operacional (se diferente da home configurada)
    if let Some(sys_home) = dirs::home_dir() {
        if !util::same_path(&sys_home, home) {
            let path = sys_home.join(".gemini").join("oauth_creds.json");
            if let Ok(content) = std::fs::read_to_string(&path) {
                out.push(content);
            }
        }
    }

    out
}

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(HTTP_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .build()
        .into()
}

/// O `access_token` do CLI, se ainda vale por mais de um minuto (sem data de validade, tenta assim mesmo).
fn valid_access_token(home: &Path, now_ms: u64) -> Option<String> {
    first_valid_token(credential_sources(home), now_ms)
}

fn make_window(used: f32, resets_at_ms: Option<u64>, now_ms: u64) -> UsageWindow {
    if resets_at_ms.is_some_and(|r| r <= now_ms) {
        UsageWindow { used: 0.0, resets_at_ms: None }
    } else {
        UsageWindow { used, resets_at_ms }
    }
}

fn merge_window(slot: &mut Option<UsageWindow>, candidate: UsageWindow) {
    match slot {
        Some(cur) if candidate.used <= cur.used => {}
        _ => *slot = Some(candidate),
    }
}

/// A sessão e a semana de um grupo (de cada tipo, o bucket de maior uso).
fn group_windows(buckets: &[Value], now_ms: u64) -> (Option<UsageWindow>, Option<UsageWindow>) {
    let mut session = None;
    let mut weekly = None;

    for b in buckets {
        let Some(rem) = b.get("remainingFraction").and_then(Value::as_f64).filter(|f| f.is_finite()) else {
            continue;
        };
        let used = (1.0 - rem).clamp(0.0, 1.0) as f32;
        let resets_at_ms = b.get("resetTime").and_then(epoch_ms);
        let window = make_window(used, resets_at_ms, now_ms);

        let win_type = b.get("window").and_then(Value::as_str).unwrap_or("");
        let bucket_id = b.get("bucketId").and_then(Value::as_str).unwrap_or("");

        if win_type == "5h" || bucket_id.contains("5h") {
            merge_window(&mut session, window);
        } else if win_type == "weekly" || bucket_id.contains("weekly") {
            merge_window(&mut weekly, window);
        }
    }
    (session, weekly)
}

/// Faz o parse da resposta de `v1internal:retrieveUserQuotaSummary`: cada grupo com as suas janelas (os do Gemini
/// primeiro) e, como resumo, as dos grupos do Gemini (sem nenhum, as de todos), cada uma com o de maior uso.
pub(crate) fn parse_quota_summary(json: &Value, now_ms: u64) -> Option<AgentUsage> {
    let mut found: Vec<(bool, UsageGroup)> = Vec::new();
    for g in json.get("groups").and_then(Value::as_array)? {
        let Some(buckets) = g.get("buckets").and_then(Value::as_array) else { continue };
        let (session, weekly) = group_windows(buckets, now_ms);
        if session.is_none() && weekly.is_none() {
            continue;
        }
        let name = g.get("displayName").and_then(Value::as_str).unwrap_or("Modelos").to_owned();
        found.push((name.to_ascii_lowercase().contains("gemini"), UsageGroup { name, session, weekly }));
    }
    let any_gemini = found.iter().any(|(gemini, _)| *gemini);
    let mut session = None;
    let mut weekly = None;
    for (_, g) in found.iter().filter(|(gemini, _)| *gemini || !any_gemini) {
        g.session.into_iter().for_each(|w| merge_window(&mut session, w));
        g.weekly.into_iter().for_each(|w| merge_window(&mut weekly, w));
    }
    found.sort_by_key(|(gemini, _)| !gemini);
    let groups = found.into_iter().map(|(_, g)| g).collect();
    (session.is_some() || weekly.is_some()).then_some(AgentUsage { session, weekly, updated_ms: now_ms, local: false, groups })
}

/// Fallback: faz o parse de `v1internal:fetchAvailableModels`.
pub(crate) fn parse_available_models(json: &Value, now_ms: u64) -> Option<AgentUsage> {
    let models = json.get("models").and_then(Value::as_object)?;

    // Se houver modelo padrão indicado, tenta ele primeiro.
    if let Some(default_id) = json.get("defaultAgentModelId").and_then(Value::as_str) {
        if let Some(minfo) = models.get(default_id) {
            if let Some(q) = minfo.get("quotaInfo") {
                if let Some(rem) = q.get("remainingFraction").and_then(Value::as_f64).filter(|f| f.is_finite()) {
                    let used = (1.0 - rem).clamp(0.0, 1.0) as f32;
                    let resets_at_ms = q.get("resetTime").and_then(epoch_ms);
                    return Some(AgentUsage {
                        session: Some(make_window(used, resets_at_ms, now_ms)),
                        weekly: None,
                        updated_ms: now_ms,
                        local: false,
                        groups: Vec::new(),
                    });
                }
            }
        }
    }

    // Senão, procura o modelo de maior uso efetivo (depois de zerar janelas já vencidas).
    let mut best: Option<UsageWindow> = None;

    for minfo in models.values() {
        let Some(q) = minfo.get("quotaInfo").filter(|q| q.is_object()) else { continue };
        let Some(rem) = q.get("remainingFraction").and_then(Value::as_f64).filter(|f| f.is_finite()) else { continue };
        let used = (1.0 - rem).clamp(0.0, 1.0) as f32;
        let resets_at_ms = q.get("resetTime").and_then(epoch_ms);
        merge_window(&mut best, make_window(used, resets_at_ms, now_ms));
    }

    Some(AgentUsage { session: Some(best?), weekly: None, updated_ms: now_ms, local: false, groups: Vec::new() })
}

fn fetch_with_bases(bases: &[String], token: &str, ua: &str, now_ms: u64) -> Option<AgentUsage> {
    let agent = http_agent();

    // Tenta primeiro o endpoint de resumo por grupos (com janela semanal e de 5h).
    // Um 401 no primeiro host (ex.: prod vs daily) não impede tentar o segundo host.
    for base in bases {
        let url = format!("{base}/v1internal:retrieveUserQuotaSummary");
        let mut resp = match agent
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", ua)
            .send_json(json!({}))
        {
            Ok(r) => r,
            Err(_) => continue,
        };

        if resp.status().as_u16() == 200 {
            if let Ok(val) = resp.body_mut().read_json::<Value>() {
                if let Some(usage) = parse_quota_summary(&val, now_ms) {
                    return Some(usage);
                }
            }
        }
    }

    // Fallback: tenta fetchAvailableModels
    for base in bases {
        let url = format!("{base}/v1internal:fetchAvailableModels");
        let mut resp = match agent
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", ua)
            .send_json(json!({ "project": "aicode-consumers" }))
        {
            Ok(r) => r,
            Err(_) => continue,
        };

        if resp.status().as_u16() == 200 {
            if let Ok(val) = resp.body_mut().read_json::<Value>() {
                if let Some(usage) = parse_available_models(&val, now_ms) {
                    return Some(usage);
                }
            }
        }
    }

    None
}

/// Consulta a cota do Antigravity (`None` em qualquer falha: sem login, rede, formato), como o CLI na versão
/// `cli_version` (sem ela, [`FALLBACK_CLI_VERSION`]). Bloqueante; roda na thread `cd-usage`.
pub(crate) fn fetch(home: &Path, cli_version: Option<&str>) -> Option<AgentUsage> {
    let now_ms = util::now_ms();
    let token = valid_access_token(home, now_ms)?;
    let bases: Vec<String> = API_HOSTS.iter().map(|h| format!("https://{h}")).collect();
    fetch_with_bases(&bases, &token, &user_agent(cli_version), now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_750_000_000_000;

    #[test]
    fn parse_quota_summary_both_windows() {
        let json_str = r#"{
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [
                        {
                            "bucketId": "gemini-weekly",
                            "window": "weekly",
                            "resetTime": "2026-10-19T15:43:01Z",
                            "remainingFraction": 0.75
                        },
                        {
                            "bucketId": "gemini-5h",
                            "window": "5h",
                            "resetTime": "2026-10-15T05:52:53Z",
                            "remainingFraction": 0.60
                        }
                    ]
                }
            ]
        }"#;
        let v: Value = serde_json::from_str(json_str).expect("json");
        let usage = parse_quota_summary(&v, NOW).expect("usage");

        assert_eq!(usage.session, Some(UsageWindow { used: 0.40, resets_at_ms: super::super::rfc3339_ms("2026-10-15T05:52:53Z") }));
        assert_eq!(usage.weekly, Some(UsageWindow { used: 0.25, resets_at_ms: super::super::rfc3339_ms("2026-10-19T15:43:01Z") }));
        assert_eq!(usage.updated_ms, NOW);
        assert!(!usage.local);
    }

    #[test]
    fn parse_quota_summary_full_usage() {
        let json_str = r#"{
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [
                        {
                            "bucketId": "gemini-weekly",
                            "window": "weekly",
                            "resetTime": "2026-10-19T15:43:01Z",
                            "remainingFraction": 0.0
                        },
                        {
                            "bucketId": "gemini-5h",
                            "window": "5h",
                            "resetTime": "2026-10-15T05:52:53Z",
                            "remainingFraction": 0.0
                        }
                    ]
                }
            ]
        }"#;
        let v: Value = serde_json::from_str(json_str).expect("json");
        let usage = parse_quota_summary(&v, NOW).expect("usage");

        assert_eq!(usage.session.expect("session").used, 1.0);
        assert_eq!(usage.weekly.expect("weekly").used, 1.0);
    }

    #[test]
    fn parse_quota_summary_zeros_expired_and_picks_max_bucket() {
        let v = json!({
            "groups": [{
                "displayName": "Gemini Models",
                "buckets": [
                    {
                        "bucketId": "gemini-pro-5h",
                        "window": "5h",
                        "resetTime": "2026-10-15T05:52:53Z",
                        "remainingFraction": 0.20
                    },
                    {
                        "bucketId": "gemini-flash-5h",
                        "window": "5h",
                        "resetTime": "2026-10-15T06:00:00Z",
                        "remainingFraction": 0.90
                    },
                    {
                        "bucketId": "gemini-weekly-expired",
                        "window": "weekly",
                        "resetTime": "2020-01-01T00:00:00Z",
                        "remainingFraction": 0.10
                    }
                ]
            }]
        });
        let usage = parse_quota_summary(&v, NOW).expect("usage");
        // Entre 80% usado (pro) e 10% usado (flash), mantém o de maior uso (0.80).
        let session = usage.session.expect("session");
        assert!((session.used - 0.80).abs() < 1e-6);
        assert_eq!(session.resets_at_ms, super::super::rfc3339_ms("2026-10-15T05:52:53Z"));
        // Janela com resetTime no passado zera.
        assert_eq!(usage.weekly, Some(UsageWindow { used: 0.0, resets_at_ms: None }));
    }

    #[test]
    fn parse_available_models_picks_default_or_min() {
        let json_str = r#"{
            "defaultAgentModelId": "gemini-flash",
            "models": {
                "gemini-flash": {
                    "quotaInfo": {
                        "remainingFraction": 0.8,
                        "resetTime": "2026-10-15T06:00:00Z"
                    }
                },
                "gemini-pro": {
                    "quotaInfo": {
                        "remainingFraction": 0.5,
                        "resetTime": "2026-10-15T07:00:00Z"
                    }
                }
            }
        }"#;
        let v: Value = serde_json::from_str(json_str).expect("json");
        let usage = parse_available_models(&v, NOW).expect("usage");
        // defaultAgentModelId foi especificado
        assert_eq!(usage.session.expect("session").used, 0.2);
        assert!(usage.weekly.is_none());

        // Sem defaultAgentModelId: escolhe a menor fração restante (maior uso: 0.5 -> used 0.5)
        let json_no_default = r#"{
            "models": {
                "gemini-flash": {
                    "quotaInfo": {
                        "remainingFraction": 0.8,
                        "resetTime": "2026-10-15T06:00:00Z"
                    }
                },
                "gemini-pro": {
                    "quotaInfo": {
                        "remainingFraction": 0.5,
                        "resetTime": "2026-10-15T07:00:00Z"
                    }
                }
            }
        }"#;
        let v2: Value = serde_json::from_str(json_no_default).expect("json");
        let usage2 = parse_available_models(&v2, NOW).expect("usage");
        assert_eq!(usage2.session.expect("session").used, 0.5);

        // Com resetTime no passado, zera a janela.
        let v_expired = json!({
            "defaultAgentModelId": "gemini-pro",
            "models": {
                "gemini-pro": {
                    "quotaInfo": {
                        "remainingFraction": 0.0,
                        "resetTime": "2020-01-01T00:00:00Z"
                    }
                }
            }
        });
        let usage_exp = parse_available_models(&v_expired, NOW).expect("usage");
        assert_eq!(usage_exp.session, Some(UsageWindow { used: 0.0, resets_at_ms: None }));
    }

    #[test]
    fn parse_creds_both_formats_and_fallback_when_first_expired() {
        // Formato keyring (vencido em relação a NOW)
        let keyring_expired = r#"{
            "token": {
                "access_token": "ya29.expired",
                "refresh_token": "1//refresh",
                "expiry": "2020-01-01T00:00:00Z"
            },
            "auth_method": "consumer"
        }"#;
        let creds1 = parse_creds_json(keyring_expired).expect("creds keyring");
        assert_eq!(creds1.access_token, "ya29.expired");
        assert!(!creds1.is_valid(NOW));

        // Formato ~/.gemini/oauth_creds.json (válido)
        let file_valid = r#"{
            "access_token": "ya29.file",
            "refresh_token": "1//file_refresh",
            "expiry_date": 1791860192679
        }"#;
        let creds2 = parse_creds_json(file_valid).expect("creds file");
        assert_eq!(creds2.access_token, "ya29.file");
        assert_eq!(creds2.expires_at_ms, Some(1791860192679));
        assert!(creds2.is_valid(NOW));

        // Se a primeira fonte (ex.: Credential Manager) estiver vencida, cai para a segunda (oauth_creds.json).
        let token = first_valid_token([keyring_expired.to_owned(), file_valid.to_owned()], NOW);
        assert_eq!(token.as_deref(), Some("ya29.file"));
    }

    /// Lê a requisição HTTP inteira (cabeçalhos + corpo pelo Content-Length). Responder e fechar com
    /// bytes ainda não lidos faz o Windows mandar RST em vez de FIN, e o cliente vê "conexão
    /// redefinida" no lugar da resposta (o teste falhava só com a máquina carregada).
    fn read_request(s: &mut std::net::TcpStream) {
        use std::io::Read;
        let mut data = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let Ok(n) = s.read(&mut buf) else { return };
            if n == 0 {
                return;
            }
            data.extend_from_slice(&buf[..n]);
            if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
                let len =
                    head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                if data.len() >= end + 4 + len {
                    return;
                }
            }
        }
    }

    #[test]
    fn http_401_on_first_host_tries_second_host() {
        use std::io::Write;
        use std::net::TcpListener;

        let l1 = TcpListener::bind("127.0.0.1:0").expect("bind 1");
        let addr1 = l1.local_addr().expect("addr 1");
        let l2 = TcpListener::bind("127.0.0.1:0").expect("bind 2");
        let addr2 = l2.local_addr().expect("addr 2");

        let t1 = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l1.accept() {
                read_request(&mut s);
                let _ = s.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });
        let t2 = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l2.accept() {
                read_request(&mut s);
                let body = r#"{"groups":[{"displayName":"Gemini Models","buckets":[{"window":"5h","remainingFraction":0.25}]}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });

        let bases = vec![format!("http://{addr1}"), format!("http://{addr2}")];
        let usage = fetch_with_bases(&bases, "tok", &user_agent(None), NOW).expect("segundo host deve responder");
        assert_eq!(usage.session, Some(UsageWindow { used: 0.75, resets_at_ms: None }));
        let _ = t1.join();
        let _ = t2.join();
    }

    #[test]
    fn gemini_groups_win_over_claude_and_each_window_keeps_the_most_used() {
        // Como o servidor responde ao User-Agent do CLI (22/10): o grupo de Claude/GPT não conta.
        let v = json!({
            "groups": [
                {"displayName": "Gemini Models", "buckets": [
                    {"bucketId": "gemini-weekly", "window": "weekly", "resetTime": "2026-10-25T13:51:49Z", "remainingFraction": 1},
                    {"bucketId": "gemini-5h", "window": "5h", "resetTime": "2026-10-23T06:54:25Z", "remainingFraction": 0.9}
                ]},
                {"displayName": "Claude and GPT models", "buckets": [
                    {"bucketId": "3p-5h", "window": "5h", "resetTime": "2026-10-23T06:54:25Z", "remainingFraction": 0.1}
                ]},
                {"displayName": "Gemini 4 Argon", "buckets": [
                    {"bucketId": "argon-weekly", "window": "weekly", "resetTime": "2026-10-24T12:30:35Z", "remainingFraction": 0.06}
                ]}
            ]
        });
        let usage = parse_quota_summary(&v, NOW).expect("usage");
        let session = usage.session.expect("session");
        assert!((session.used - 0.1).abs() < 1e-6, "a sessão é a do Gemini, não a do Claude");
        let weekly = usage.weekly.expect("weekly");
        assert!((weekly.used - 0.94).abs() < 1e-6, "a semana é a do Argon, a mais gasta");
        assert_eq!(weekly.resets_at_ms, super::super::rfc3339_ms("2026-10-24T12:30:35Z"));
        // Todos os grupos ficam para o painel, os do Gemini primeiro; o Argon só tem a semana.
        let names: Vec<&str> = usage.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Gemini Models", "Gemini 4 Argon", "Claude and GPT models"]);
        assert!(usage.groups[1].session.is_none() && usage.groups[1].weekly.is_some());
        assert!((usage.groups[2].session.expect("3p").used - 0.9).abs() < 1e-6);
        // Sem grupo do Gemini, vale o que vier.
        let only_claude =
            json!({"groups": [{"displayName": "Claude and GPT models", "buckets": [{"window": "5h", "remainingFraction": 0.5}]}]});
        assert_eq!(parse_quota_summary(&only_claude, NOW).and_then(|u| u.session).map(|s| s.used), Some(0.5));
    }

    #[test]
    fn the_request_looks_like_the_installed_cli() {
        assert_eq!(parse_version("1.4.5\r\n").as_deref(), Some("1.4.5"));
        assert_eq!(parse_version("agy v1.10.2 (abc)").as_deref(), Some("1.10.2"));
        assert_eq!(parse_version("erro: não sei"), None);
        let ua = user_agent(Some("1.5.0"));
        assert!(ua.starts_with("antigravity/cli/1.5.0 (aidev_client; os_type="), "{ua}");
        assert!(user_agent(None).starts_with(&format!("antigravity/cli/{FALLBACK_CLI_VERSION} ")));
    }

    #[test]
    fn invalid_json_returns_none() {
        let v = json!({});
        assert!(parse_quota_summary(&v, NOW).is_none());
        assert!(parse_available_models(&v, NOW).is_none());
    }

    #[test]
    #[ignore = "usa a rede"]
    fn fetch_real_quota() {
        let home = dirs::home_dir().expect("home");
        let usage = fetch(&home, None).expect("usage");
        eprintln!("{usage:?}");
        assert!(usage.session.is_some() || usage.weekly.is_some());
    }
}
