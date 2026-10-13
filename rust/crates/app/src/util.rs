//! Funções puras compartilhadas pela UI (fáceis de testar).

/// Referência de arquivo para o compositor: `@rel` ou `@"rel"` quando há espaços.
pub fn file_ref(rel_path: &str) -> String {
    if rel_path.contains(' ') {
        format!("@\"{rel_path}\"")
    } else {
        format!("@{rel_path}")
    }
}

/// Insere a referência no texto do compositor com o espaçamento do renderer original:
/// vazio → `ref `; termina em espaço → `val + ref `; senão → `val + ' ' + ref + ' '`.
pub fn insert_ref(value: &str, rel_path: &str) -> String {
    let r = file_ref(rel_path);
    if value.is_empty() {
        format!("{r} ")
    } else if value.ends_with(' ') {
        format!("{value}{r} ")
    } else {
        format!("{value} {r} ")
    }
}

/// Rótulo do chip de anexo: último segmento sem o prefixo `hash-` (`/^[a-z0-9]+-/`).
pub fn chip_label(reference: &str) -> String {
    let last = reference.rsplit('/').next().unwrap_or(reference);
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

/// Nome da pasta exibido na raiz da árvore (`ws.split(/[\\/]/).filter(Boolean).pop() || 'ARQUIVOS'`).
pub fn folder_name(path: &str) -> String {
    path.split(['/', '\\']).rfind(|s| !s.is_empty()).unwrap_or("ARQUIVOS").to_owned()
}

/// Valor de "Rodadas" limitado a 1..=6 (`Math.max(1, Math.min(6, Number(v) || 1))`).
pub fn clamp_rounds(text: &str) -> u32 {
    match text.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && v != 0.0 => v.clamp(1.0, 6.0) as u32,
        _ => 1,
    }
}

/// Texto completo enviado: instrução + referências `@anexo` separadas por espaço.
pub fn compose_full(text: &str, attachments: &[String]) -> String {
    let refs = attachments.iter().map(|a| format!("@{a}")).collect::<Vec<_>>().join(" ");
    [text.to_owned(), refs].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_ref_insertion_spacing() {
        assert_eq!(insert_ref("", "src/main.rs"), "@src/main.rs ");
        assert_eq!(insert_ref("analisa ", "a.md"), "analisa @a.md ");
        assert_eq!(insert_ref("analisa", "a.md"), "analisa @a.md ");
        assert_eq!(insert_ref("", "minha pasta/x.txt"), "@\"minha pasta/x.txt\" ");
        assert_eq!(insert_ref("@a.md ", "b.md"), "@a.md @b.md ");
    }

    #[test]
    fn chip_labels() {
        assert_eq!(chip_label("dados/anexos/1a2b3c-foto.png"), "foto.png");
        assert_eq!(chip_label("dados/anexos/Foto.png"), "Foto.png");
        assert_eq!(chip_label("x/abc-def-ghi.txt"), "def-ghi.txt");
        assert_eq!(chip_label("semhifen"), "semhifen");
        assert_eq!(chip_label("C:/Users/x/AB-1.png"), "AB-1.png");
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("/home/u/workspace"), "workspace");
        assert_eq!(folder_name("C:\\Users\\u\\proj\\"), "proj");
        assert_eq!(folder_name(""), "ARQUIVOS");
    }

    #[test]
    fn rounds_clamp() {
        assert_eq!(clamp_rounds("2"), 2);
        assert_eq!(clamp_rounds("0"), 1);
        assert_eq!(clamp_rounds("99"), 6);
        assert_eq!(clamp_rounds("-3"), 1);
        assert_eq!(clamp_rounds("abc"), 1);
        assert_eq!(clamp_rounds("3.7"), 3);
    }

    #[test]
    fn compose_text_with_attachments() {
        assert_eq!(compose_full("faz", &[]), "faz");
        assert_eq!(compose_full("faz", &["a.png".into(), "b c.txt".into()]), "faz @a.png @b c.txt");
    }

    #[test]
    fn time_format_shape() {
        let t = time_pt_br(1_700_000_000_000);
        assert_eq!(t.len(), 8);
        assert_eq!(t.as_bytes()[2], b':');
    }
}
