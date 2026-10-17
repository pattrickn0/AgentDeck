//! Uso do Codex: o último `rate_limits` gravado nos `~/.codex/sessions/**/rollout-*.jsonl`.
//!
//! O Codex grava eventos `token_count` com `payload.rate_limits.{primary,secondary}`, cada um com
//! `used_percent` (0..100), `window_minutes` e `resets_in_seconds` (relativo à hora do evento) ou
//! `resets_at` (segundos Unix), conforme a versão.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::{epoch_ms, rfc3339_ms, AgentUsage, UsageWindow};
use crate::util;

/// Quanto do fim do arquivo é lido (os eventos `token_count` se repetem a cada turno).
const TAIL_BYTES: u64 = 256 * 1024;
/// Janelas até um dia são a "sessão" (5 h); acima disso, a semana.
const SESSION_MAX_MINUTES: u64 = 24 * 60;

/// Quantos `rollout-*.jsonl` mais recentes são inspecionados (uma sessão recém-aberta ainda
/// não tem `token_count`, então caímos para a anterior).
const MAX_ROLLOUTS: usize = 5;

/// Lê o uso mais recente do Codex (`None` se não há sessão ou o formato não bate).
pub(crate) fn read(home: &Path) -> Option<AgentUsage> {
    let codex_home = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".codex"));
    read_sessions(&codex_home.join("sessions"), util::now_ms())
}

/// [`read`] a partir da pasta `sessions` e de um "agora" fixo (testes).
fn read_sessions(dir: &Path, now_ms: u64) -> Option<AgentUsage> {
    for (path, mtime) in newest_rollouts(dir, MAX_ROLLOUTS) {
        let Some(text) = read_tail(&path) else { continue };
        let Ok(dur) = mtime.duration_since(SystemTime::UNIX_EPOCH) else { continue };
        let mtime_ms = dur.as_millis() as u64;
        // Do fim para o começo: o último evento com `rate_limits` preenchido.
        let found = text.lines().rev().filter(|l| l.contains("\"rate_limits\"")).find_map(|line| parse_line(line, mtime_ms, now_ms));
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Os até `limit` arquivos `rollout-*.jsonl` modificados mais recentemente em `sessions/AAAA/MM/DD/`
/// (qualquer profundidade até 4, pois uma sessão retomada volta a gravar no arquivo do dia em que começou).
fn newest_rollouts(dir: &Path, limit: usize) -> Vec<(PathBuf, SystemTime)> {
    fn walk(dir: &Path, depth: u32, all: &mut Vec<(PathBuf, SystemTime)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name();
            let name = name.to_string_lossy();
            if ft.is_dir() && depth < 4 {
                walk(&e.path(), depth + 1, all);
            } else if ft.is_file() && name.starts_with("rollout-") && name.ends_with(".jsonl") {
                let Ok(mtime) = e.metadata().and_then(|m| m.modified()) else { continue };
                all.push((e.path(), mtime));
            }
        }
    }
    let mut all = Vec::new();
    walk(dir, 0, &mut all);
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    all.truncate(limit);
    all
}

/// Os últimos [`TAIL_BYTES`] do arquivo, sem a primeira linha (se cortada no meio).
/// Se a última linha sozinha tiver `>= TAIL_BYTES` (sem `\n` antes do fim), mantém o trecho
/// lido em vez de esvaziar `text`.
fn read_tail(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.take(TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        let trimmed = text.trim_end_matches(['\r', '\n']);
        if let Some(i) = trimmed.find('\n') {
            text.drain(..=i);
        }
    }
    Some(text)
}

/// Uso de uma linha do rollout, se ela traz `rate_limits` não nulo.
fn parse_line(line: &str, mtime_ms: u64, now_ms: u64) -> Option<AgentUsage> {
    let v: Value = serde_json::from_str(line).ok()?;
    let payload = v.get("payload").unwrap_or(&v);
    let limits = [payload.get("rate_limits"), payload.get("info").and_then(|i| i.get("rate_limits")), v.get("rate_limits")]
        .into_iter()
        .flatten()
        .find(|l| l.is_object())?;
    // `resets_in_seconds` conta a partir do evento (o `timestamp` da linha; sem ele, o mtime).
    let event_ms = v.get("timestamp").and_then(Value::as_str).and_then(rfc3339_ms).unwrap_or(mtime_ms);
    let mut usage = AgentUsage { updated_ms: now_ms, ..Default::default() };
    for (key, fallback_weekly) in [("primary", false), ("secondary", true)] {
        let Some(w) = limits.get(key).filter(|w| w.is_object()) else { continue };
        let Some(pct) = w.get("used_percent").and_then(Value::as_f64).filter(|p| p.is_finite()) else { continue };
        let resets_at_ms = w.get("resets_at").and_then(epoch_ms).or_else(|| {
            let secs = w.get("resets_in_seconds").and_then(Value::as_f64).filter(|s| s.is_finite() && *s >= 0.0)?;
            Some(event_ms + (secs * 1000.0) as u64)
        });
        let mut window = UsageWindow { used: (pct / 100.0).clamp(0.0, 1.0) as f32, resets_at_ms };
        // A janela já renovou desde o evento: o uso dela recomeçou do zero.
        if resets_at_ms.is_some_and(|r| r <= now_ms) {
            window = UsageWindow { used: 0.0, resets_at_ms: None };
        }
        let weekly = w.get("window_minutes").and_then(Value::as_u64).map_or(fallback_weekly, |m| m > SESSION_MAX_MINUTES);
        let slot = if weekly { &mut usage.weekly } else { &mut usage.session };
        if slot.is_none() {
            *slot = Some(window);
        }
    }
    (usage.session.is_some() || usage.weekly.is_some()).then_some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_758_200_000_000;

    fn token_count(ts: &str, limits: &str) -> String {
        format!(r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{limits}}}}}"#)
    }

    fn write(dir: &Path, rel: &str, lines: &[String]) -> PathBuf {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("pai")).expect("mkdir");
        std::fs::write(&path, lines.join("\n") + "\n").expect("write");
        path
    }

    #[test]
    fn last_rate_limits_of_the_newest_rollout() {
        let tmp = tempfile::tempdir().expect("tmp");
        let old = write(
            tmp.path(),
            "2025/09/17/rollout-a.jsonl",
            &[token_count("2025-09-17T10:00:00Z", r#"{"primary":{"used_percent":90.0,"window_minutes":300}}"#)],
        );
        let file = std::fs::File::options().write(true).open(&old).expect("abrir");
        file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000)).expect("mtime");
        write(
            tmp.path(),
            "2025/09/18/rollout-b.jsonl",
            &[
                r#"{"timestamp":"2025-09-18T12:00:00Z","type":"session_meta","payload":{}}"#.to_owned(),
                token_count(
                    "2025-09-18T12:00:00Z",
                    r#"{"primary":{"used_percent":10.0,"window_minutes":300,"resets_in_seconds":7200},"secondary":{"used_percent":40.0,"window_minutes":10080,"resets_in_seconds":86400}}"#,
                ),
                // O último evento tem os limites invertidos (só `window_minutes` decide) e `resets_at`.
                token_count(
                    "2025-09-18T12:30:00Z",
                    r#"{"primary":{"used_percent":55.5,"window_minutes":10080,"resets_at":1758800000},"secondary":{"used_percent":12.0,"window_minutes":300,"resets_in_seconds":3600}}"#,
                ),
                // Turno sem limites (chave de API): ignorado.
                token_count("2025-09-18T12:40:00Z", "null"),
                r#"{"timestamp":"2025-09-18T12:41:00Z","type":"response_item","payl"#.to_owned(),
            ],
        );
        let u = read_sessions(tmp.path(), NOW).expect("uso");
        let ts = rfc3339_ms("2025-09-18T12:30:00Z").expect("ts");
        assert_eq!(u.session, Some(UsageWindow { used: 0.12, resets_at_ms: Some(ts + 3_600_000) }));
        assert_eq!(u.weekly, Some(UsageWindow { used: 0.555, resets_at_ms: Some(1_758_800_000_000) }));
        assert_eq!(u.updated_ms, NOW);
        assert!(!u.local);
    }

    #[test]
    fn falls_back_to_previous_rollout_when_newest_has_no_rate_limits() {
        let tmp = tempfile::tempdir().expect("tmp");
        let prev = write(
            tmp.path(),
            "2025/09/18/rollout-prev.jsonl",
            &[token_count("2025-09-18T12:00:00Z", r#"{"primary":{"used_percent":45.0,"window_minutes":300}}"#)],
        );
        let f_prev = std::fs::File::options().write(true).open(&prev).expect("abrir prev");
        f_prev.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2_000)).expect("mtime prev");

        // Sessão recém-aberta: só tem session_meta, sem rate_limits.
        let cur = write(
            tmp.path(),
            "2025/09/18/rollout-cur.jsonl",
            &[r#"{"timestamp":"2025-09-18T12:05:00Z","type":"session_meta","payload":{}}"#.to_owned()],
        );
        let f_cur = std::fs::File::options().write(true).open(&cur).expect("abrir cur");
        f_cur.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(3_000)).expect("mtime cur");

        let u = read_sessions(tmp.path(), NOW).expect("uso da sessão anterior");
        assert_eq!(u.session, Some(UsageWindow { used: 0.45, resets_at_ms: None }));
    }

    #[test]
    fn read_tail_does_not_empty_text_when_last_line_exceeds_tail_bytes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let huge = "a".repeat((TAIL_BYTES as usize) + 1024);
        let path = write(tmp.path(), "2025/09/18/rollout-huge.jsonl", &[r#"{"type":"session_meta"}"#.to_owned(), huge]);
        let tail = read_tail(&path).expect("tail");
        assert!(!tail.is_empty(), "read_tail não pode esvaziar text quando a última linha excede 256 KB");
    }

    #[test]
    fn expired_windows_restart_and_missing_minutes_follow_the_key() {
        let line = token_count(
            "2025-09-17T00:00:00Z",
            r#"{"primary":{"used_percent":80,"resets_in_seconds":60},"secondary":{"used_percent":30}}"#,
        );
        let u = parse_line(&line, 0, NOW).expect("uso");
        assert_eq!(u.session, Some(UsageWindow { used: 0.0, resets_at_ms: None }));
        assert_eq!(u.weekly, Some(UsageWindow { used: 0.3, resets_at_ms: None }));
        // Sem `timestamp`, `resets_in_seconds` conta do mtime.
        let line = r#"{"payload":{"rate_limits":{"primary":{"used_percent":1,"resets_in_seconds":10}}}}"#;
        assert_eq!(parse_line(line, NOW, NOW).expect("uso").session.expect("sessão").resets_at_ms, Some(NOW + 10_000));
    }

    #[test]
    fn only_the_tail_is_read() {
        let tmp = tempfile::tempdir().expect("tmp");
        let filler = format!(r#"{{"type":"response_item","payload":{{"text":"{}"}}}}"#, "x".repeat(1000));
        let mut lines = vec![token_count("2025-09-18T12:00:00Z", r#"{"primary":{"used_percent":99,"window_minutes":300}}"#)];
        lines.extend(std::iter::repeat_n(filler, 300));
        write(tmp.path(), "2025/09/18/rollout-c.jsonl", &lines);
        // O evento ficou antes dos últimos 256 KB: sem dados.
        assert_eq!(read_sessions(tmp.path(), NOW), None);
        assert_eq!(read_sessions(&tmp.path().join("nada"), NOW), None);
    }
}
