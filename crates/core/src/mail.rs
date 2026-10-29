//! Durable mail between agents (`ask-agent --read`).
//!
//! A short message to a tab (up to [`SHORT_MAX`] characters, one line) is typed into its terminal as it
//! always was. A long or multi-line one (a question to a tab, or the late answer to a question) is saved
//! here, and only a one-line notice is typed ([`notice`]); the addressee reads the text with
//! `ask-agent --read <n>`. Typing a long text is fragile (pastes split, Enter timing), clutters the
//! receiver's context and is lost if the app exits before it is typed.
//!
//! On disk, per project: `data/mail/<project>/index.json` (metadata and a preview, no bodies) and one
//! `<n>.txt` per message. The numbers are per project and never reused. What only matters while the app
//! runs (was the notice queued, was it typed, how many reminders went out, when it was read) is not
//! saved: after a restart every unread message is notified again, when its addressee's tab is back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::events::{CoreEvent, Delivery};
use crate::instances::Ident;
use crate::util::{now_ms, terminal_text, truncate_chars};
use crate::Core;

/// A message up to this many characters, on a single line, is typed as it is.
pub const SHORT_MAX: usize = 200;
/// Characters of the text quoted in the notice.
pub const PREVIEW_CHARS: usize = 60;
/// Times the notice is typed again when the receiver ends a turn without reading the mail.
pub const MAX_REMINDERS: u8 = 2;
/// Read messages kept per project; the oldest ones leave (unread ones never do).
const KEEP_READ: usize = 200;

/// Does `text` go through the mail instead of being typed?
pub fn is_long(text: &str) -> bool {
    let text = text.trim();
    text.contains('\n') || text.contains('\r') || text.chars().count() > SHORT_MAX
}

/// First [`PREVIEW_CHARS`] characters of `text` on one line, for the notice.
pub fn preview(text: &str) -> String {
    let flat = terminal_text(text).split_whitespace().collect::<Vec<_>>().join(" ").replace('"', "'");
    let (head, cut) = truncate_chars(&flat, PREVIEW_CHARS);
    if cut {
        format!("{}…", head.trim_end())
    } else {
        head.to_owned()
    }
}

/// The one line typed in the addressee's terminal.
pub fn notice(mail: &Mail) -> String {
    format!(
        "[mail #{} from {}: \"{}\" — read with: ask-agent --read {}]",
        mail.id,
        terminal_text(&mail.from),
        terminal_text(&mail.preview),
        mail.id
    )
}

/// What the message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailKind {
    /// A question to the addressee (`request` is waiting for `ask-agent --reply`).
    Question,
    /// The late answer to a question the addressee asked.
    Answer,
}

/// A saved message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mail {
    pub id: u64,
    /// Who sent it (instance id, or `you`).
    pub from: String,
    /// Addressee (instance id, without the project).
    pub to: String,
    pub kind: MailKind,
    /// Number of the question it belongs to.
    pub request: u64,
    /// Epoch in milliseconds.
    pub ts: u64,
    /// When the addressee read it (epoch in milliseconds).
    #[serde(default)]
    pub read_ms: Option<u64>,
    /// First characters of the text, for the notice.
    pub preview: String,
    /// The notice was handed to the interface and has not been lost since.
    #[serde(skip)]
    notified: bool,
    /// The interface typed the last notice.
    #[serde(skip)]
    typed: bool,
    /// Reminders sent so far.
    #[serde(skip)]
    reminders: u8,
    /// When it was read, in this run (a question's deadline counts from it).
    #[serde(skip)]
    read_at: Option<Instant>,
}

impl Mail {
    pub fn is_read(&self) -> bool {
        self.read_ms.is_some()
    }
}

/// What a question's deadline needs to know about its mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// The question did not go through the mail.
    NoMail,
    /// Sent, not read yet: the receiver has not started working on it.
    Unread,
    /// Read at this moment.
    Read(Instant),
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ProjectMail {
    version: u32,
    /// Next number to use (never decreases).
    next: u64,
    mail: Vec<Mail>,
}

/// The mail of every project, read from disk on first use.
pub struct MailStore {
    dir: PathBuf,
    projects: Mutex<HashMap<String, ProjectMail>>,
}

/// Project id that works as a folder name (`p` + hexadecimal, in practice).
fn safe_project_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl MailStore {
    /// `dir` is `<data>/mail`.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, projects: Mutex::new(HashMap::new()) }
    }

    fn folder(&self, project: &str) -> Option<PathBuf> {
        safe_project_id(project).then(|| self.dir.join(project))
    }

    fn body_path(folder: &Path, id: u64) -> PathBuf {
        folder.join(format!("{id}.txt"))
    }

    /// Applies `f` to the project's mail (read from disk the first time). With `save`, writes the index afterwards.
    /// A project id that is not a folder name has no mail: `None`.
    fn with<R>(&self, project: &str, save: bool, f: impl FnOnce(&mut ProjectMail) -> R) -> Option<R> {
        let folder = self.folder(project)?;
        let mut all = self.projects.lock();
        let entry = all.entry(project.to_owned()).or_insert_with(|| load_index(&folder));
        let out = f(entry);
        if save {
            if let Err(e) = save_index(&folder, entry) {
                log::warn!("could not save the mail index in {}: {e}", folder.display());
            }
        }
        Some(out)
    }

    /// Saves a message for `to` and returns it. Error = it could not be written (the caller types the text instead).
    pub fn save(&self, project: &str, from: &str, to: &str, kind: MailKind, request: u64, body: &str) -> Result<Mail, String> {
        let folder = self.folder(project).ok_or("invalid project")?;
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let mut all = self.projects.lock();
        let list = all.entry(project.to_owned()).or_insert_with(|| load_index(&folder));
        let id = list.next.max(list.mail.iter().map(|m| m.id + 1).max().unwrap_or(1)).max(1);
        // The body first: an index entry never points to a missing file.
        std::fs::write(Self::body_path(&folder, id), body).map_err(|e| e.to_string())?;
        let mail = Mail {
            id,
            from: from.to_owned(),
            to: to.to_owned(),
            kind,
            request,
            ts: now_ms(),
            read_ms: None,
            preview: preview(body),
            notified: true,
            typed: false,
            reminders: 0,
            read_at: None,
        };
        list.next = id + 1;
        list.mail.push(mail.clone());
        forget_old_read(&folder, list);
        if let Err(e) = save_index(&folder, list) {
            log::warn!("could not save the mail index in {}: {e}", folder.display());
        }
        Ok(mail)
    }

    pub fn get(&self, project: &str, id: u64) -> Option<Mail> {
        self.with(project, false, |p| p.mail.iter().find(|m| m.id == id).cloned()).flatten()
    }

    /// The text of message `id`.
    pub fn body(&self, project: &str, id: u64) -> Option<String> {
        let folder = self.folder(project)?;
        std::fs::read(Self::body_path(&folder, id)).ok().map(|b| crate::util::decode_text(&b))
    }

    /// Marks the message as read (the first reading counts) and returns it.
    pub fn mark_read(&self, project: &str, id: u64) -> Option<Mail> {
        self.with(project, true, |p| {
            let mail = p.mail.iter_mut().find(|m| m.id == id)?;
            if mail.read_ms.is_none() {
                mail.read_ms = Some(now_ms());
                mail.read_at = Some(Instant::now());
            }
            Some(mail.clone())
        })
        .flatten()
    }

    /// Marks as read the answer to question `request` that was shown in full somewhere else (`--inbox`).
    pub fn mark_answer_read(&self, project: &str, request: u64) {
        let hit = self.with(project, false, |p| p.mail.iter().find(|m| m.kind == MailKind::Answer && m.request == request).map(|m| m.id));
        if let Some(id) = hit.flatten() {
            self.mark_read(project, id);
        }
    }

    /// `to`'s messages, oldest first: the unread ones, or the last `limit` with `all`.
    pub fn list_for(&self, project: &str, to: &str, all: bool, limit: usize) -> Vec<Mail> {
        self.with(project, false, |p| {
            let mine: Vec<&Mail> = p.mail.iter().filter(|m| m.to == to && (all || !m.is_read())).collect();
            let skip = mine.len().saturating_sub(if all { limit } else { usize::MAX });
            mine.into_iter().skip(skip).cloned().collect()
        })
        .unwrap_or_default()
    }

    /// Messages of `to` whose notice has to be typed again when the tab is back (never notified in this run, or lost).
    pub fn take_unnotified(&self, project: &str, to: &str) -> Vec<Mail> {
        self.with(project, false, |p| {
            p.mail
                .iter_mut()
                .filter(|m| m.to == to && !m.is_read() && !m.notified)
                .map(|m| {
                    m.notified = true;
                    m.clone()
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// The turn of `to` ended: the unread messages whose notice was typed and that still have a reminder left.
    pub fn take_reminders(&self, project: &str, to: &str) -> Vec<Mail> {
        self.with(project, false, |p| {
            p.mail
                .iter_mut()
                .filter(|m| m.to == to && !m.is_read() && m.typed && m.reminders < MAX_REMINDERS)
                .map(|m| {
                    m.reminders += 1;
                    m.typed = false;
                    m.notified = true;
                    m.clone()
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// The interface typed the notice of message `id`.
    pub fn typed(&self, project: &str, id: u64) {
        self.with(project, false, |p| p.mail.iter_mut().filter(|m| m.id == id).for_each(|m| m.typed = true));
    }

    /// The notice of message `id` did not reach the terminal: it goes again when the tab is back.
    pub fn dropped(&self, project: &str, id: u64) {
        self.with(project, false, |p| {
            p.mail.iter_mut().filter(|m| m.id == id).for_each(|m| {
                m.notified = false;
                m.typed = false;
            })
        });
    }

    /// Is message `id` still unread?
    pub fn unread(&self, project: &str, id: u64) -> bool {
        self.get(project, id).is_some_and(|m| !m.is_read())
    }

    /// The addressee `old` became `new` (a renamed tab keeps its mail).
    pub fn rename(&self, project: &str, old: &str, new: &str) {
        self.with(project, true, |p| p.mail.iter_mut().filter(|m| m.to == old).for_each(|m| m.to = new.to_owned()));
    }

    /// State of the mail of question `request`.
    pub fn waiting(&self, project: &str, request: u64) -> Waiting {
        let found = self.with(project, false, |p| {
            p.mail.iter().rev().find(|m| m.kind == MailKind::Question && m.request == request).map(|m| (m.is_read(), m.read_at))
        });
        match found.flatten() {
            None => Waiting::NoMail,
            Some((true, Some(at))) => Waiting::Read(at),
            // Read before this run's clock (cannot be waited on any more) or not read at all.
            Some((true, None)) => Waiting::Read(Instant::now()),
            Some((false, _)) => Waiting::Unread,
        }
    }
}

fn load_index(folder: &Path) -> ProjectMail {
    let path = folder.join("index.json");
    let mut list = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<ProjectMail>(&text).unwrap_or_else(|e| {
            // Unreadable: keep a copy before the next write puts an empty list on top.
            let backup = path.with_extension(format!("json.unreadable-{}", now_ms()));
            match std::fs::copy(&path, &backup) {
                Ok(_) => log::error!("unreadable mail index {} ({e}); copy kept in {}", path.display(), backup.display()),
                Err(e2) => log::error!("unreadable mail index {} ({e}; and the copy failed: {e2})", path.display()),
            }
            ProjectMail::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectMail::default(),
        Err(e) => {
            log::error!("could not read the mail index {}: {e}", path.display());
            ProjectMail::default()
        }
    };
    list.version = 1;
    list.next = list.next.max(list.mail.iter().map(|m| m.id + 1).max().unwrap_or(1)).max(1);
    list
}

/// Writes beside and swaps: a crash in the middle does not leave the file half written.
fn save_index(folder: &Path, list: &ProjectMail) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let path = folder.join("index.json");
    let text = serde_json::to_string_pretty(list).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Drops the oldest read messages past [`KEEP_READ`], with their files.
fn forget_old_read(folder: &Path, list: &mut ProjectMail) {
    let read = list.mail.iter().filter(|m| m.is_read()).count();
    if read <= KEEP_READ {
        return;
    }
    let mut excess = read - KEEP_READ;
    list.mail.retain(|m| {
        if excess > 0 && m.is_read() {
            excess -= 1;
            let _ = std::fs::remove_file(MailStore::body_path(folder, m.id));
            false
        } else {
            true
        }
    });
}

// ---------- The core's side ----------

impl Core {
    /// The interface typed the notice of mail `id` in the addressee's terminal.
    pub fn mail_typed(&self, project: &str, id: u64) {
        self.inner.mail.typed(project, id);
    }

    /// The notice of mail `id` did not reach the terminal (queue discarded, tab closed or ended): it is typed
    /// again when the addressee's tab is back.
    pub fn mail_dropped(&self, project: &str, id: u64) {
        self.inner.mail.dropped(project, id);
    }

    /// Mail `id` was not read yet: its notice (queued, perhaps for a long time) is still worth typing.
    pub fn mail_unread(&self, project: &str, id: u64) -> bool {
        self.inner.mail.unread(project, id)
    }

    /// The turn of instance `id` ended: for each message it left unread after being told about it, the notice
    /// is typed once more (at most [`MAX_REMINDERS`] times).
    pub fn mail_turn_ended(&self, project: &str, id: &str) {
        for mail in self.inner.mail.take_reminders(project, id) {
            self.emit_mail_notice(project, &mail);
        }
    }

    /// The process of instance `id`'s tab started: the unread messages nobody told it about in this run
    /// (the app was restarted, or the notice was lost) are announced.
    pub fn mail_tab_started(&self, project: &str, id: &str) {
        for mail in self.inner.mail.take_unnotified(project, id) {
            self.emit_mail_notice(project, &mail);
        }
    }

    /// Notice of an old message: it goes to the tab only if it is open (it never opens one), and is no question of the
    /// queue (`request: 0`).
    fn emit_mail_notice(&self, project: &str, mail: &Mail) {
        let Some(ident) = Ident::parse(&mail.to) else { return };
        self.emit(CoreEvent::Deliver(Delivery {
            request: 0,
            project: project.to_owned(),
            ident,
            create: false,
            from: mail.from.clone(),
            prompt: notice(mail),
            answer: true,
            boot: None,
            mail: Some(mail.id),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, MailStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = MailStore::new(tmp.path().join("mail"));
        (tmp, store)
    }

    #[test]
    fn only_a_long_or_multi_line_text_becomes_mail() {
        assert!(!is_long("short question"));
        assert!(!is_long(&"a".repeat(SHORT_MAX)));
        assert!(is_long(&"a".repeat(SHORT_MAX + 1)));
        assert!(is_long("two\nlines"));
        assert!(!is_long("a trailing newline does not count\n"));
    }

    #[test]
    fn the_notice_is_one_clean_line_with_the_first_sixty_characters() {
        let (_tmp, store) = store();
        let body = format!("say \"hi\"\x1b[31m\nand then {}", "x".repeat(100));
        let mail = store.save("p1", "claude@dev", "codex", MailKind::Question, 7, &body).unwrap();
        let line = notice(&mail);
        assert!(line.starts_with("[mail #1 from claude@dev: \"say 'hi'[31m and then xxx"), "{line}");
        assert!(line.ends_with("…\" — read with: ask-agent --read 1]"), "{line}");
        assert!(!line.contains('\n') && !line.contains('\x1b'), "{line:?}");
        assert_eq!(preview("short"), "short");
    }

    #[test]
    fn the_message_is_saved_on_disk_and_survives_a_restart() {
        let (tmp, store) = store();
        let a = store.save("p1", "claude", "codex", MailKind::Question, 3, "first\nbody").unwrap();
        let b = store.save("p1", "claude", "codex", MailKind::Answer, 4, "second\nbody").unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        assert!(tmp.path().join("mail/p1/1.txt").is_file() && tmp.path().join("mail/p1/index.json").is_file());
        store.mark_read("p1", 1);

        let again = MailStore::new(tmp.path().join("mail"));
        let unread = again.list_for("p1", "codex", false, 20);
        assert_eq!(unread.iter().map(|m| m.id).collect::<Vec<_>>(), [2]);
        assert_eq!(again.body("p1", 2).as_deref(), Some("second\nbody"));
        // Nothing was notified in this run: the notice goes again when the tab is back.
        assert_eq!(again.take_unnotified("p1", "codex").len(), 1);
        assert!(again.take_unnotified("p1", "codex").is_empty());
        // The numbers are never reused.
        assert_eq!(again.save("p1", "claude", "codex", MailKind::Question, 5, "x\ny").unwrap().id, 3);
        assert!(again.list_for("p2", "codex", true, 20).is_empty());
    }

    #[test]
    fn reminders_need_a_typed_notice_and_stop_at_two() {
        let (_tmp, store) = store();
        let m = store.save("p1", "claude", "codex", MailKind::Question, 3, "a\nb").unwrap();
        // Queued, not typed yet: the end of a turn does not remind.
        assert!(store.take_reminders("p1", "codex").is_empty());
        store.typed("p1", m.id);
        assert_eq!(store.take_reminders("p1", "codex").len(), 1);
        // The reminder has to be typed before the next one.
        assert!(store.take_reminders("p1", "codex").is_empty());
        store.typed("p1", m.id);
        assert_eq!(store.take_reminders("p1", "codex").len(), 1);
        store.typed("p1", m.id);
        assert!(store.take_reminders("p1", "codex").is_empty(), "at most two reminders");
        // Reading stops everything.
        let n = store.save("p1", "claude", "codex", MailKind::Question, 4, "a\nb").unwrap();
        store.typed("p1", n.id);
        store.mark_read("p1", n.id);
        assert!(store.take_reminders("p1", "codex").is_empty());
    }

    #[test]
    fn a_lost_notice_is_sent_again_when_the_tab_is_back() {
        let (_tmp, store) = store();
        let m = store.save("p1", "claude", "codex", MailKind::Question, 3, "a\nb").unwrap();
        assert!(store.take_unnotified("p1", "codex").is_empty(), "the first notice is already on its way");
        store.dropped("p1", m.id);
        assert_eq!(store.take_unnotified("p1", "codex").len(), 1);
    }

    #[test]
    fn a_question_waits_from_the_reading() {
        let (_tmp, store) = store();
        assert_eq!(store.waiting("p1", 9), Waiting::NoMail);
        let m = store.save("p1", "claude", "codex", MailKind::Question, 9, "a\nb").unwrap();
        assert_eq!(store.waiting("p1", 9), Waiting::Unread);
        store.mark_read("p1", m.id);
        assert!(matches!(store.waiting("p1", 9), Waiting::Read(_)));
    }

    #[test]
    fn renaming_moves_the_mail_and_old_read_mail_is_forgotten() {
        let (tmp, store) = store();
        store.save("p1", "claude", "codex", MailKind::Question, 3, "a\nb").unwrap();
        store.rename("p1", "codex", "codex@dev");
        assert!(store.list_for("p1", "codex", true, 20).is_empty());
        assert_eq!(store.list_for("p1", "codex@dev", false, 20).len(), 1);
        for n in 0..KEEP_READ + 5 {
            let m = store.save("p1", "claude", "x", MailKind::Answer, n as u64, "a\nb").unwrap();
            store.mark_read("p1", m.id);
        }
        store.save("p1", "claude", "x", MailKind::Answer, 0, "a\nb").unwrap();
        assert_eq!(store.list_for("p1", "x", true, 1000).len(), KEEP_READ + 1);
        assert!(!tmp.path().join("mail/p1/2.txt").exists(), "the oldest read message left with its file");
        assert!(tmp.path().join("mail/p1/1.txt").exists(), "an unread one never leaves");
    }

    #[test]
    fn a_project_id_that_is_not_a_folder_name_has_no_mail() {
        let (_tmp, store) = store();
        assert!(store.save("../x", "a", "b", MailKind::Question, 1, "a\nb").is_err());
        assert!(store.list_for("../x", "b", true, 20).is_empty());
    }
}
