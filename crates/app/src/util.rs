//! Pure functions shared by the UI (easy to test).

use std::path::PathBuf;

/// File reference in the `@path` style (or `@"path"` when it has spaces).
pub fn file_ref(rel_path: &str) -> String {
    if rel_path.contains(' ') {
        format!("@\"{rel_path}\"")
    } else {
        format!("@{rel_path}")
    }
}

/// Formats a file path for the tab's CLI prompt (`kind`).
///
/// - Claude (`kind == "claude"`): `@path` (or `@"path"` if there is a space).
/// - The other terminals/agents: `"path"` if there is a space, or the bare path.
pub fn tab_file_ref(kind: &str, path: &str) -> String {
    if kind == "claude" {
        file_ref(path)
    } else if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.to_owned()
    }
}

/// Joins several paths to type into the tab's prompt (`kind`), separated by a space and with a
/// trailing space so the user keeps typing without gluing text to the path.
pub fn tab_file_refs_text(kind: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let joined = paths.iter().map(|p| tab_file_ref(kind, p)).collect::<Vec<_>>().join(" ");
    format!("{joined} ")
}

/// Appends the attachment list (absolute paths under `data/attachments`) to the end of the task
/// request when the "New task" form has attachments. The request is read by an agent, so the
/// heading is English.
pub fn append_brief_attachments(brief: &str, attachments: &[PathBuf]) -> String {
    let base = brief.trim();
    if attachments.is_empty() {
        return base.to_owned();
    }
    let list = attachments.iter().map(|p| format!("- {}", p.display())).collect::<Vec<_>>().join("\n");
    if base.is_empty() {
        format!("Attachments:\n{list}")
    } else {
        format!("{base}\n\nAttachments:\n{list}")
    }
}

/// Attachment chip label: the last segment without the `hash-` prefix (`/^[a-z0-9]+-/`).
pub fn chip_label(reference: &str) -> String {
    let last = reference.rsplit(['/', '\\']).next().unwrap_or(reference);
    let prefix_len = last.bytes().take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit()).count();
    if prefix_len > 0 && last.as_bytes().get(prefix_len) == Some(&b'-') {
        last[prefix_len + 1..].to_owned()
    } else {
        last.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_file_reference_per_cli() {
        assert_eq!(tab_file_ref("claude", "data/attachments/a.png"), "@data/attachments/a.png");
        assert_eq!(tab_file_ref("claude", "my folder/a.png"), "@\"my folder/a.png\"");
        assert_eq!(tab_file_ref("antigravity", "data/attachments/a.png"), "data/attachments/a.png");
        assert_eq!(tab_file_ref("opencode", "my folder/a.png"), "\"my folder/a.png\"");
        assert_eq!(
            tab_file_refs_text("claude", &["data/attachments/1.png".into(), "folder 2/b.txt".into()]),
            "@data/attachments/1.png @\"folder 2/b.txt\" "
        );
        assert_eq!(
            tab_file_refs_text("codex", &["data/attachments/1.png".into(), "folder 2/b.txt".into()]),
            "data/attachments/1.png \"folder 2/b.txt\" "
        );
        assert_eq!(tab_file_refs_text("claude", &[]), "");
    }

    #[test]
    fn brief_attachments_appended_at_end() {
        let p1 = PathBuf::from(r"C:\proj\data\attachments\1a-print.png");
        let p2 = PathBuf::from(r"C:\proj\data\attachments\2b-log.txt");
        assert_eq!(append_brief_attachments("Fix bug", &[]), "Fix bug");
        assert_eq!(
            append_brief_attachments("Fix bug", &[p1.clone(), p2.clone()]),
            "Fix bug\n\nAttachments:\n- C:\\proj\\data\\attachments\\1a-print.png\n- C:\\proj\\data\\attachments\\2b-log.txt"
        );
        assert_eq!(append_brief_attachments("", &[p1]), "Attachments:\n- C:\\proj\\data\\attachments\\1a-print.png");
    }

    #[test]
    fn chip_labels() {
        assert_eq!(chip_label("data/attachments/1a2b3c-photo.png"), "photo.png");
        assert_eq!(chip_label("data/attachments/Photo.png"), "Photo.png");
        assert_eq!(chip_label("x/abc-def-ghi.txt"), "def-ghi.txt");
        assert_eq!(chip_label("nohyphen"), "nohyphen");
        assert_eq!(chip_label("C:/Users/x/AB-1.png"), "AB-1.png");
        assert_eq!(chip_label(r"C:\data\attachments\1a2b-photo.png"), "photo.png");
    }
}
