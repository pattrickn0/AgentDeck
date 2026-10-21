//! Funções puras compartilhadas pela UI (fáceis de testar).

use std::path::PathBuf;

/// Referência de arquivo estilo `@caminho` (ou `@"caminho"` quando há espaços).
pub fn file_ref(rel_path: &str) -> String {
    if rel_path.contains(' ') {
        format!("@\"{rel_path}\"")
    } else {
        format!("@{rel_path}")
    }
}

/// Formata um caminho de arquivo para o prompt da CLI da aba (`kind`).
///
/// - No Claude (`kind == "claude"`): `@caminho` (ou `@"caminho"` se houver espaço).
/// - Nos demais terminais/agentes: `"caminho"` se houver espaço, ou `caminho` direto.
pub fn tab_file_ref(kind: &str, path: &str) -> String {
    if kind == "claude" {
        file_ref(path)
    } else if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.to_owned()
    }
}

/// Junta múltiplos caminhos para digitar no prompt da aba (`kind`), separados por espaço e com
/// espaço final para que o usuário continue digitando sem colar no caminho.
pub fn tab_file_refs_text(kind: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let joined = paths.iter().map(|p| tab_file_ref(kind, p)).collect::<Vec<_>>().join(" ");
    format!("{joined} ")
}

/// Acrescenta a lista de anexos (caminhos absolutos em `dados/anexos`) ao final do pedido da
/// tarefa quando houver anexos no formulário "Nova tarefa".
pub fn append_brief_attachments(brief: &str, attachments: &[PathBuf]) -> String {
    let base = brief.trim();
    if attachments.is_empty() {
        return base.to_owned();
    }
    let list = attachments.iter().map(|p| format!("- {}", p.display())).collect::<Vec<_>>().join("\n");
    if base.is_empty() {
        format!("Anexos:\n{list}")
    } else {
        format!("{base}\n\nAnexos:\n{list}")
    }
}

/// Rótulo do chip de anexo: último segmento sem o prefixo `hash-` (`/^[a-z0-9]+-/`).
pub fn chip_label(reference: &str) -> String {
    let last = reference.rsplit(['/', '\\']).next().unwrap_or(reference);
    let prefix_len = last.bytes().take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit()).count();
    if prefix_len > 0 && last.as_bytes().get(prefix_len) == Some(&b'-') {
        last[prefix_len + 1..].to_owned()
    } else {
        last.to_owned()
    }
}

/// Hora local como `toLocaleTimeString('pt-BR')` (HH:MM:SS).
pub fn time_pt_br(epoch_ms: u64) -> String {
    use chrono::TimeZone;
    let ms = if epoch_ms == 0 { chrono::Utc::now().timestamp_millis() } else { epoch_ms as i64 };
    match chrono::Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.format("%H:%M:%S").to_string(),
        chrono::LocalResult::None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_file_reference_per_cli() {
        assert_eq!(tab_file_ref("claude", "dados/anexos/a.png"), "@dados/anexos/a.png");
        assert_eq!(tab_file_ref("claude", "minha pasta/a.png"), "@\"minha pasta/a.png\"");
        assert_eq!(tab_file_ref("antigravity", "dados/anexos/a.png"), "dados/anexos/a.png");
        assert_eq!(tab_file_ref("opencode", "minha pasta/a.png"), "\"minha pasta/a.png\"");
        assert_eq!(
            tab_file_refs_text("claude", &["dados/anexos/1.png".into(), "pasta 2/b.txt".into()]),
            "@dados/anexos/1.png @\"pasta 2/b.txt\" "
        );
        assert_eq!(
            tab_file_refs_text("codex", &["dados/anexos/1.png".into(), "pasta 2/b.txt".into()]),
            "dados/anexos/1.png \"pasta 2/b.txt\" "
        );
        assert_eq!(tab_file_refs_text("claude", &[]), "");
    }

    #[test]
    fn brief_attachments_appended_at_end() {
        let p1 = PathBuf::from(r"C:\proj\dados\anexos\1a-print.png");
        let p2 = PathBuf::from(r"C:\proj\dados\anexos\2b-log.txt");
        assert_eq!(append_brief_attachments("Corrigir bug", &[]), "Corrigir bug");
        assert_eq!(
            append_brief_attachments("Corrigir bug", &[p1.clone(), p2.clone()]),
            "Corrigir bug\n\nAnexos:\n- C:\\proj\\dados\\anexos\\1a-print.png\n- C:\\proj\\dados\\anexos\\2b-log.txt"
        );
        assert_eq!(append_brief_attachments("", &[p1]), "Anexos:\n- C:\\proj\\dados\\anexos\\1a-print.png");
    }

    #[test]
    fn chip_labels() {
        assert_eq!(chip_label("dados/anexos/1a2b3c-foto.png"), "foto.png");
        assert_eq!(chip_label("dados/anexos/Foto.png"), "Foto.png");
        assert_eq!(chip_label("x/abc-def-ghi.txt"), "def-ghi.txt");
        assert_eq!(chip_label("semhifen"), "semhifen");
        assert_eq!(chip_label("C:/Users/x/AB-1.png"), "AB-1.png");
        assert_eq!(chip_label(r"C:\dados\anexos\1a2b-foto.png"), "foto.png");
    }

    #[test]
    fn time_format_shape() {
        let t = time_pt_br(1_700_000_000_000);
        assert_eq!(t.len(), 8);
        assert_eq!(t.as_bytes()[2], b':');
    }
}
