// SPDX-License-Identifier: MIT OR Apache-2.0
//! The system's file picker, like file dialog: a frontend that has
//! one (the GNOME and Windows apps) is asked to show it, and what
//! the person picks is opened the way the minibuffer's answer would be.
//!
//! The editor can't show a native dialog itself, so a command leaves a
//! [`Request`] for the frontend to find ([`pending`]); the frontend shows
//! the dialog and answers with [`finish`]. Where nothing can show one (the
//! terminal) the commands fall back to the minibuffer prompts, so they work
//! everywhere and menus can offer them unconditionally.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::app::App;

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::line("find-directory-dialog", "Open a folder in Dired with the system file picker."),
    forge_docs::CommandDoc::line("find-file-dialog", "Visit a file chosen with the system file picker."),
    forge_docs::CommandDoc::line("write-file-dialog", "Save this buffer under a name chosen with the system file picker."),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    OpenFile,
    OpenFolder,
    SaveAs,
}

/// What the frontend is asked to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u32,
    pub kind: Kind,
    pub title: String,
    /// The folder to start in.
    pub start: String,
    /// For Save As: the name to offer.
    pub name: String,
}

static CAPABLE: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU32 = AtomicU32::new(1);
static PENDING: Mutex<Option<Request>> = Mutex::new(None);

/// A frontend that can show a native picker says so once, at startup.
pub fn set_capable(yes: bool) {
    CAPABLE.store(yes, Ordering::SeqCst);
}

pub fn capable() -> bool {
    CAPABLE.load(Ordering::SeqCst)
}

/// The picker waiting to be shown, if any.
pub fn pending() -> Option<Request> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn ask(r: Request) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    let (kind, title, fallback) = match cmd {
        "find-file-dialog" => (Kind::OpenFile, "Open File", "find-file"),
        "find-directory-dialog" => (Kind::OpenFolder, "Open Folder", "dired"),
        "write-file-dialog" => (Kind::SaveAs, "Save As", "write-file"),
        _ => return false,
    };
    if !capable() {
        app.msg("No system file picker here; using the minibuffer");
        app.run_command(fallback);
        return true;
    }
    let (start, name) = {
        let b = app.buf_ref();
        let dir = b.path().and_then(|p| p.parent().map(PathBuf::from)).or_else(|| std::env::current_dir().ok());
        (dir.map(|d| d.display().to_string()).unwrap_or_default(), b.name().to_string())
    };
    ask(Request { id: NEXT.fetch_add(1, Ordering::SeqCst), kind, title: title.into(), start, name });
    true
}

/// The frontend's answer: the path picked, or None if it was cancelled.
/// Answers to an old request are ignored.
pub fn finish(app: &mut App, id: u32, path: Option<String>) {
    let request = {
        let mut slot = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            Some(r) if r.id == id => slot.take(),
            _ => None,
        }
    };
    let (Some(r), Some(path)) = (request, path.filter(|p| !p.is_empty())) else { return };
    let path = PathBuf::from(path);
    match r.kind {
        Kind::OpenFile => match app.editor.open_file(&path.display().to_string()) {
            Ok((id, _)) => app.switch_to(id),
            Err(e) => app.error(format!("Cannot open {}: {e}", path.display())),
        },
        Kind::OpenFolder => crate::dired::open(app, &path),
        Kind::SaveAs => {
            if crate::org_crypt::write_file_with_crypt(app, &path) {
                return;
            }
            match app.buf().save_as(&path) {
                Ok(()) => {
                    if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned())
                        && app.buf_ref().name() != name
                    {
                        let unique = app.editor.unique_name(&name);
                        app.buf().set_name(unique);
                    }
                    app.msg(format!("Wrote {}", path.display()));
                }
                Err(e) => app.error(format!("Error writing {}: {e}", path.display())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test: the picker's state is shared by the whole process.
    #[test]
    fn the_picker_is_asked_for_and_its_answer_opens_things() {
        set_capable(false);
        let mut fallback = crate::start(&crate::StartOptions::default()).unwrap();
        assert!(execute(&mut fallback, "find-file-dialog"));
        assert!(pending().is_none());
        assert!(fallback.minibuffer.is_some(), "the find-file prompt");

        set_capable(true);
        let dir = std::env::temp_dir().join(format!("forge-dialog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("picked.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let mut a = crate::start(&crate::StartOptions::default()).unwrap();
        assert!(execute(&mut a, "find-file-dialog"));
        let r = pending().expect("a request");
        assert_eq!(r.kind, Kind::OpenFile);
        // A stale answer is ignored.
        finish(&mut a, r.id + 99, Some(file.display().to_string()));
        assert!(pending().is_some());
        finish(&mut a, r.id, Some(file.display().to_string()));
        assert!(pending().is_none());
        assert_eq!(a.buf_ref().name(), "picked.txt");
        // Cancelling opens nothing.
        assert!(execute(&mut a, "find-directory-dialog"));
        let r = pending().unwrap();
        finish(&mut a, r.id, None);
        assert!(pending().is_none());
        // A folder opens in Dired.
        assert!(execute(&mut a, "find-directory-dialog"));
        let r = pending().unwrap();
        finish(&mut a, r.id, Some(dir.display().to_string()));
        assert!(a.buf_ref().name().contains("forge-dialog"), "{}", a.buf_ref().name());
        set_capable(false);
        let _ = std::fs::remove_dir_all(dir);
    }
}
