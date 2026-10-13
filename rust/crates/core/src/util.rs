//! Utilitários compartilhados.

use std::path::Path;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Ver [`crate::ref_path`].
pub fn ref_path(workspace: &Path, abs: &Path) -> String {
    let s = match abs.strip_prefix(workspace) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => abs.to_string_lossy().into_owned(),
    };
    s.replace('\\', "/")
}
