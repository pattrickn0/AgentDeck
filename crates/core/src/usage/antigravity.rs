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

use super::{epoch_ms, AgentUsage, UsageWindow};
use crate::util;

/// Endpoint de produção e diário da API Cloud Code.
const API_HOSTS: &[&str] = &["cloudcode-pa.googleapis.com", "daily-cloudcode-pa.googleapis.com"];

/// Timeout para as requisições HTTP da cota.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

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
    #[cfg(windows)]
    if let Some(json_str) = wincred::read_generic("gemini:antigravity") {
        out.push(json_str);
    }

    // Fallback: ~/.gemini/oauth_creds.json na pasta home passada
    let path = home.join(".gemini").join("oauth_creds.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        out.push(content);
    }

    // Fallback adicional: home do sistema operacional (se diferente da home configurada)
    if let Some(sys_home) = dirs::home_dir() {
        if sys_home != home {
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

/// Faz o parse da resposta de `v1internal:retrieveUserQuotaSummary`.
pub(crate) fn parse_quota_summary(json: &Value, now_ms: u64) -> Option<AgentUsage> {
    let groups = json.get("groups").and_then(Value::as_array)?;
    // Procura o grupo dos modelos Gemini, ou o primeiro que tiver buckets.
    let group = groups
        .iter()
        .find(|g| g.get("displayName").and_then(Value::as_str).is_some_and(|n| n.to_ascii_lowercase().contains("gemini")))
        .or_else(|| groups.iter().find(|g| g.get("buckets").and_then(Value::as_array).is_some_and(|b| !b.is_empty())))?;

    let buckets = group.get("buckets").and_then(Value::as_array)?;
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

    (session.is_some() || weekly.is_some()).then_some(AgentUsage { session, weekly, updated_ms: now_ms, local: false })
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

    Some(AgentUsage { session: Some(best?), weekly: None, updated_ms: now_ms, local: false })
}

fn fetch_with_bases(bases: &[String], token: &str, now_ms: u64) -> Option<AgentUsage> {
    let agent = http_agent();

    // Tenta primeiro o endpoint de resumo por grupos (com janela semanal e de 5h).
    // Um 401 no primeiro host (ex.: prod vs daily) não impede tentar o segundo host.
    for base in bases {
        let url = format!("{base}/v1internal:retrieveUserQuotaSummary");
        let mut resp = match agent
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", "antigravity-cli")
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
            .header("User-Agent", "antigravity-cli")
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

/// Consulta a cota do Antigravity (`None` em qualquer falha: sem login, rede, formato).
/// Bloqueante; roda na thread `cd-usage`.
pub(crate) fn fetch(home: &Path) -> Option<AgentUsage> {
    let now_ms = util::now_ms();
    let token = valid_access_token(home, now_ms)?;
    let bases: Vec<String> = API_HOSTS.iter().map(|h| format!("https://{h}")).collect();
    fetch_with_bases(&bases, &token, now_ms)
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

    #[test]
    fn http_401_on_first_host_tries_second_host() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let l1 = TcpListener::bind("127.0.0.1:0").expect("bind 1");
        let addr1 = l1.local_addr().expect("addr 1");
        let l2 = TcpListener::bind("127.0.0.1:0").expect("bind 2");
        let addr2 = l2.local_addr().expect("addr 2");

        let t1 = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l1.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });
        let t2 = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l2.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let body = r#"{"groups":[{"displayName":"Gemini Models","buckets":[{"window":"5h","remainingFraction":0.25}]}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });

        let bases = vec![format!("http://{addr1}"), format!("http://{addr2}")];
        let usage = fetch_with_bases(&bases, "tok", NOW).expect("segundo host deve responder");
        assert_eq!(usage.session, Some(UsageWindow { used: 0.75, resets_at_ms: None }));
        let _ = t1.join();
        let _ = t2.join();
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
        let usage = fetch(&home).expect("usage");
        assert!(usage.session.is_some() || usage.weekly.is_some());
    }
}
