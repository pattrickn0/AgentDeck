//! Shared utilities (time, base36, paths and text).

use std::path::{Component, Path, PathBuf};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Equivalent to JavaScript's `Number.prototype.toString(36)` (for unsigned integers).
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

/// Makes a path absolute (relative to current directory) and normalizes it lexically
/// (resolves `.` and `..` without touching disk, like Node's `path.resolve`). Works with
/// paths that do not exist yet.
pub fn absolutize(p: &Path) -> PathBuf {
    let joined =
        if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|cwd| cwd.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    normalize_lexical(&joined)
}

/// Strips Windows verbatim prefix (`\\?\C:\…` → `C:\…`, `\\?\UNC\srv\…` → `\\srv\…`).
pub fn strip_verbatim(p: &Path) -> PathBuf {
    let Some(s) = p.to_str() else { return p.to_path_buf() };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\").or_else(|| s.strip_prefix(r"\\?\unc\")) {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p.to_path_buf()
}

/// Purely lexical normalization: removes `.`, applies `..` (without going past root).
pub fn normalize_lexical(p: &Path) -> PathBuf {
    let p = strip_verbatim(p);
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                // Does not ascend beyond root (like Node does with "/..").
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

/// Compares path components (case-insensitive and without verbatim prefix `\\?\` on Windows).
pub(crate) fn same_component(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if cfg!(windows) {
        fn norm(s: &std::ffi::OsStr) -> String {
            let s = s.to_string_lossy();
            if let Some(r) = s.strip_prefix(r"\\?\UNC\").or_else(|| s.strip_prefix(r"\\?\unc\")) {
                return format!(r"\\{}", r.to_lowercase());
            }
            s.strip_prefix(r"\\?\").unwrap_or(&s).to_lowercase()
        }
        norm(a) == norm(b)
    } else {
        a == b
    }
}

/// `true` if `path` is `base` or is inside `base` (by-component comparison,
/// both already normalized).
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

/// `true` if both (normalized) paths are the same.
pub fn same_path(a: &Path, b: &Path) -> bool {
    path_within(a, b) && path_within(b, a)
}

/// See [`crate::ref_path`]. Equivalent to `refPath`: relative to workspace when the
/// file is inside it, otherwise the absolute path; always with `/`.
pub fn ref_path(workspace: &Path, abs: &Path) -> String {
    let ws = absolutize(workspace);
    let target = absolutize(abs);
    let s = if path_within(&target, &ws) && !same_path(&target, &ws) {
        let rel: PathBuf = target.components().skip(ws.components().count()).collect();
        rel.to_string_lossy().into_owned()
    } else {
        // Outside workspace: attempt relative path with `..`? Node would also return
        // "../x", but JS discards any relative path starting with ".." and uses absolute.
        target.to_string_lossy().into_owned()
    };
    s.replace('\\', "/")
}

/// Last `n` non-empty lines of text, ignoring noise (`EPERM, retrying`).
pub fn last_meaningful_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.trim().is_empty() && !l.contains("EPERM, retrying"))
        .collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Truncates a string to `max` characters (not bytes), without breaking UTF-8.
pub fn truncate_chars(s: &str, max: usize) -> (&str, bool) {
    match s.char_indices().nth(max) {
        Some((idx, _)) => (&s[..idx], true),
        None => (s, false),
    }
}

/// User's home directory (empty if unknown).
pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// Text to be typed into a terminal: no ESC or other control characters (which
/// would alter terminal state or exit bracketed paste), newlines as `\n`.
pub fn terminal_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").chars().filter(|&c| c == '\n' || c == '\t' || !c.is_control()).collect()
}

/// Text from bytes that came from a file or standard input: UTF-8 (with or without BOM), UTF-16
/// with BOM (PowerShell 5.1 writes like this with `>` and `Out-File`) or, lacking any hint, UTF-8
/// with replacements (never fails, like `from_utf8_lossy`).
///
/// This is what keeps accents and dashes intact when an agent writes a task report
/// to a file: via the shell (PowerShell, `$OutputEncoding` in US-ASCII) every non-ASCII character
/// would become `?` before even arriving here.
pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return utf16_to_string(&bytes[2..], true);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return utf16_to_string(&bytes[2..], false);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Combines UTF-16 byte pairs into a string (odd trailing bytes are ignored).
fn utf16_to_string(bytes: &[u8], little_endian: bool) -> String {
    let (chunks, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = chunks.iter().map(|c| if little_endian { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) }).collect();
    String::from_utf16_lossy(&units)
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
    fn decode_text_reads_utf8_and_utf16_with_acents() {
        assert_eq!(decode_text("ação — ícone".as_bytes()), "ação — ícone");
        // UTF-8 BOM and UTF-16 BOM (LE, the PowerShell 5.1 one with `>`/`Out-File`, and BE).
        assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, b'a', b'\xC3', b'\xA7']), "aç");
        let utf16le: Vec<u8> = [0xFF, 0xFE].into_iter().chain("ação".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(decode_text(&utf16le), "ação");
        let utf16be: Vec<u8> = [0xFE, 0xFF].into_iter().chain("ação".encode_utf16().flat_map(u16::to_be_bytes)).collect();
        assert_eq!(decode_text(&utf16be), "ação");
        // Garbage that is neither UTF-8 nor UTF-16: replacements, no panic.
        assert_eq!(decode_text(b"a\xE7\xE3o"), "a\u{FFFD}\u{FFFD}o");
        assert_eq!(decode_text(&[]), "");
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
        // String prefix is not a path prefix.
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
