//! App sessions and data retention (equivalent to `registerSessionAndCleanup`).
//!
//! Each run of the app is a "session" identified by its start instant in base36.
//! We keep the last 5 in `dados/sessions.json`; attachments and prompts (`s_<session>_...`) of
//! older sessions are deleted, and leftover `.wav` audio too.

use std::collections::HashSet;
use std::path::Path;

/// How many sessions to keep.
pub const MAX_SESSIONS: usize = 5;

/// New session id: `Date.now().toString(36)`.
pub fn new_session_id() -> String {
    crate::util::base36(crate::util::now_ms())
}

/// Registers `current` in `dados/sessions.json` (keeping the last 5) and cleans
/// `dados/anexos` and `dados/prompts`. I/O errors are ignored (as in the JS).
pub fn register_and_cleanup(data_dir: &Path, current: &str) {
    let file = data_dir.join("sessions.json");
    let mut history: Vec<String> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .map(|arr| arr.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    if !history.iter().any(|s| s == current) {
        history.push(current.to_owned());
    }
    while history.len() > MAX_SESSIONS {
        history.remove(0);
    }
    let _ = std::fs::create_dir_all(data_dir);
    if let Ok(text) = serde_json::to_string_pretty(&history) {
        let _ = std::fs::write(&file, text);
    }

    let active: HashSet<&str> = history.iter().map(String::as_str).collect();

    // 1. Attachments: delete .wav (there is no use in keeping audio) and files of old sessions.
    let att_dir = data_dir.join("anexos");
    let _ = std::fs::create_dir_all(&att_dir);
    purge(&att_dir, &active, true);
    // 2. Long prompts of old sessions.
    purge(&data_dir.join("prompts"), &active, false);
}

fn purge(dir: &Path, active: &HashSet<&str>, wav_too: bool) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let full = entry.path();
        if wav_too && name.ends_with(".wav") {
            let _ = std::fs::remove_file(&full);
            continue;
        }
        if let Some(sess) = session_of(&name) {
            if !active.contains(sess) {
                let _ = std::fs::remove_file(&full);
            }
        }
    }
}

/// Extracts `<session>` from names `s_<session>_...` (regex `/^s_([a-z0-9]+)_/i`).
pub fn session_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("s_").or_else(|| name.strip_prefix("S_"))?;
    let end = rest.find('_')?;
    let sess = &rest[..end];
    (!sess.is_empty() && sess.chars().all(|c| c.is_ascii_alphanumeric())).then_some(sess)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_name_parsing() {
        assert_eq!(session_of("s_lx9abc_123-a.png"), Some("lx9abc"));
        assert_eq!(session_of("s__x"), None);
        assert_eq!(session_of("audio-1.wav"), None);
        assert_eq!(session_of("s_ab-c_x"), None);
    }

    #[test]
    fn keeps_last_five_and_purges_old_files() {
        let tmp = tempfile::tempdir().expect("tmp");
        let data = tmp.path();
        std::fs::create_dir_all(data.join("anexos")).expect("mkdir");
        std::fs::create_dir_all(data.join("prompts")).expect("mkdir");
        std::fs::write(data.join("sessions.json"), r#"["a1","a2","a3","a4","a5"]"#).expect("write");
        for f in ["s_a1_x.png", "s_a2_y.png", "audio-1.wav", "s_a6_z.txt", "other.txt"] {
            std::fs::write(data.join("anexos").join(f), "x").expect("write");
        }
        std::fs::write(data.join("prompts").join("s_a1_prompt-1.md"), "x").expect("write");
        std::fs::write(data.join("prompts").join("s_a3_prompt-1.md"), "x").expect("write");

        register_and_cleanup(data, "a6");

        let hist: Vec<String> = serde_json::from_str(&std::fs::read_to_string(data.join("sessions.json")).expect("read")).expect("json");
        assert_eq!(hist, ["a2", "a3", "a4", "a5", "a6"]);
        let exists = |p: &str| data.join(p).exists();
        assert!(!exists("anexos/s_a1_x.png"));
        assert!(exists("anexos/s_a2_y.png"));
        assert!(!exists("anexos/audio-1.wav"));
        assert!(exists("anexos/s_a6_z.txt"));
        assert!(exists("anexos/other.txt"));
        assert!(!exists("prompts/s_a1_prompt-1.md"));
        assert!(exists("prompts/s_a3_prompt-1.md"));
    }
}
