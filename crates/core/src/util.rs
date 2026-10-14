//! Utilitários compartilhados (tempo, base36, caminhos e textos).

use std::path::{Component, Path, PathBuf};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Equivalente a `Number.prototype.toString(36)` do JavaScript (para inteiros sem sinal).
pub fn base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Torna um caminho absoluto (relativo ao diretório atual) e o normaliza lexicamente
/// (resolve `.` e `..` sem tocar o disco, como `path.resolve` do Node). Funciona com
/// caminhos que ainda não existem.
pub fn absolutize(p: &Path) -> PathBuf {
    let joined =
        if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|cwd| cwd.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    normalize_lexical(&joined)
}

/// Normalização puramente lexical: remove `.`, aplica `..` (sem passar da raiz).
pub fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                // Não sobe além da raiz (como o Node faz com "/..").
                let has_normal = out.components().next_back().is_some_and(|c| matches!(c, Component::Normal(_)));
                if has_normal {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

/// Compara componentes de caminho (case-insensitive no Windows, como o sistema de arquivos).
fn same_component(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// `true` se `path` é `base` ou está dentro de `base` (comparação por componentes,
/// ambos já normalizados).
pub fn path_within(path: &Path, base: &Path) -> bool {
    let mut pc = path.components();
    for bc in base.components() {
        match pc.next() {
            Some(c) if same_component(c.as_os_str(), bc.as_os_str()) => {}
            _ => return false,
        }
    }
    true
}

/// `true` se os dois caminhos (normalizados) são o mesmo.
pub fn same_path(a: &Path, b: &Path) -> bool {
    path_within(a, b) && path_within(b, a)
}

/// Ver [`crate::ref_path`]. Equivalente a `refPath`: relativo ao workspace quando o
/// arquivo está dentro dele, senão o caminho absoluto; sempre com `/`.
pub fn ref_path(workspace: &Path, abs: &Path) -> String {
    let ws = absolutize(workspace);
    let target = absolutize(abs);
    let s = if path_within(&target, &ws) && !same_path(&target, &ws) {
        let rel: PathBuf = target.components().skip(ws.components().count()).collect();
        rel.to_string_lossy().into_owned()
    } else {
        // Fora do workspace: tenta um caminho relativo com `..`? O Node também devolveria
        // "../x", mas o JS descarta qualquer relativo iniciado em ".." e usa o absoluto.
        target.to_string_lossy().into_owned()
    };
    s.replace('\\', "/")
}

/// Últimas `n` linhas não vazias de um texto, ignorando ruídos (`EPERM, retrying`).
pub fn last_meaningful_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.trim().is_empty() && !l.contains("EPERM, retrying"))
        .collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Trunca uma string em `max` caracteres (não bytes), sem quebrar UTF-8.
pub fn truncate_chars(s: &str, max: usize) -> (&str, bool) {
    match s.char_indices().nth(max) {
        Some((idx, _)) => (&s[..idx], true),
        None => (s, false),
    }
}

/// Pasta pessoal do usuário (vazia se desconhecida).
pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// Texto que vai ser digitado num terminal: sem ESC nem outros caracteres de controle (que
/// mexeriam no terminal ou sairiam do modo colagem), quebras de linha como `\n`.
pub fn terminal_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").chars().filter(|&c| c == '\n' || c == '\t' || !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_text_cannot_escape_the_paste() {
        let evil = "oi\x1b[201~!rm -rf ~\r\x1b[200~\u{9b}2J\x07\x00fim\r\nlinha\ttab";
        assert_eq!(terminal_text(evil), "oi[201~!rm -rf ~\n[200~2Jfim\nlinha\ttab");
        assert_eq!(terminal_text("ação ✓ 🎉"), "ação ✓ 🎉");
    }

    #[test]
    fn base36_matches_js() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        // (1700000000000).toString(36) === "loyw3v28"
        assert_eq!(base36(1_700_000_000_000), "loyw3v28");
    }

    #[test]
    fn normalize_handles_dots() {
        let p = normalize_lexical(Path::new("/a/b/../c/./d"));
        assert_eq!(p, PathBuf::from("/a/c/d"));
        assert_eq!(normalize_lexical(Path::new("/../..")), PathBuf::from("/"));
    }

    #[cfg(unix)]
    #[test]
    fn ref_path_inside_and_outside() {
        let ws = Path::new("/w/proj");
        assert_eq!(ref_path(ws, Path::new("/w/proj/src/a.rs")), "src/a.rs");
        assert_eq!(ref_path(ws, Path::new("/w/proj/x/../b.txt")), "b.txt");
        assert_eq!(ref_path(ws, Path::new("/w/dados/anexos/f.png")), "/w/dados/anexos/f.png");
        // Prefixo de string não é prefixo de caminho.
        assert_eq!(ref_path(ws, Path::new("/w/project2/f")), "/w/project2/f");
        assert_eq!(ref_path(ws, ws), "/w/proj");
    }

    #[test]
    fn last_lines_filters_noise() {
        let t = "a\n\nEPERM, retrying x\nb\r\nc\nd\ne\nf\ng\n";
        assert_eq!(last_meaningful_lines(t, 6), "b\nc\nd\ne\nf\ng");
    }

    #[test]
    fn truncate_is_char_based() {
        assert_eq!(truncate_chars("ação", 2), ("aç", true));
        assert_eq!(truncate_chars("ab", 5), ("ab", false));
    }
}
