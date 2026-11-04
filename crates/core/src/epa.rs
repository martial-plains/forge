// SPDX-License-Identifier: MIT OR Apache-2.0
//! Transparent GPG files, after EasyPG (`epa-file`): visiting
//! `notes.org.gpg` decrypts it into the buffer, and saving encrypts it
//! again, to the same recipients (or with the same passphrase, for a
//! symmetric file) the file was encrypted for. The plaintext is never
//! written anywhere else: a buffer visiting an encrypted file is not
//! auto-saved.
//!
//! The work is done by the `gpg` program, so its agent asks for the
//! passphrase and a smart card or a key on a token works as it does in a
//! shell.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// Who a file is encrypted to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Epa {
    /// Key ids or addresses. Empty: a passphrase (symmetric encryption),
    /// unless `forge.opt["epa_file_encrypt_to"]` names recipients.
    pub recipients: Vec<String>,
}

static DEFAULT_RECIPIENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PROGRAM: Mutex<Option<String>> = Mutex::new(None);

/// `epa-file-encrypt-to`: who a new `.gpg` file is encrypted to.
pub fn set_default_recipients(list: Vec<String>) {
    if let Ok(mut d) = DEFAULT_RECIPIENTS.lock() {
        *d = list;
    }
}

pub fn default_recipients() -> Vec<String> {
    DEFAULT_RECIPIENTS.lock().map(|d| d.clone()).unwrap_or_default()
}

/// Use another program than `gpg` (tests, or `gpg2` by name).
pub fn set_program(program: Option<&str>) {
    if let Ok(mut p) = PROGRAM.lock() {
        *p = program.map(str::to_string);
    }
}

fn program() -> String {
    PROGRAM
        .lock()
        .ok()
        .and_then(|p| p.clone())
        .unwrap_or_else(|| "gpg".to_string())
}

/// Is this file name one EasyPG handles (`epa-file-name-regexp`)?
pub fn is_encrypted_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| name_without_gpg(n).is_some())
}

/// `notes.org.gpg` -> `notes.org`, for choosing the major mode.
pub fn name_without_gpg(name: &str) -> Option<&str> {
    name.strip_suffix(".gpg").filter(|s| !s.is_empty())
}

fn run(args: &[&str], stdin: Option<&[u8]>) -> io::Result<std::process::Output> {
    let mut child = Command::new(program())
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("can't run {}: {e}", program())))?;
    if let Some(data) = stdin {
        let mut pipe = child.stdin.take().expect("piped");
        // Written from another thread: gpg may fill its output pipe before
        // it has read everything.
        let data = data.to_vec();
        let writer = std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        });
        let out = child.wait_with_output()?;
        let _ = writer.join();
        return Ok(out);
    }
    child.wait_with_output()
}

fn failure(what: &str, out: &std::process::Output) -> io::Error {
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no message");
    io::Error::other(format!("{what}: {}", last.trim()))
}

/// Who `path` is encrypted to, read from its packets.
fn recipients_of(path: &Path) -> Epa {
    let p = path.to_string_lossy().into_owned();
    let Ok(out) = run(&["--batch", "--quiet", "--list-packets", &p], None) else {
        return Epa::default();
    };
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut recipients = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(":pubkey enc packet:")
            && let Some(id) = rest.split("keyid ").nth(1)
        {
            let id = id.trim().to_string();
            // All zeros: a hidden recipient, which can't be named again.
            if !id.is_empty() && id.chars().any(|c| c != '0') && !recipients.contains(&id) {
                recipients.push(id);
            }
        }
    }
    Epa { recipients }
}

/// Decrypt `path`: its plaintext and who it was encrypted to.
pub fn decrypt(path: &Path) -> io::Result<(Vec<u8>, Epa)> {
    let epa = recipients_of(path);
    let p = path.to_string_lossy().into_owned();
    let out = run(&["--quiet", "--decrypt", &p], None)?;
    if !out.status.success() {
        return Err(failure(&format!("can't decrypt {}", path.display()), &out));
    }
    Ok((out.stdout, epa))
}

/// Encrypt `plaintext` for `epa`. Fails, leaving the caller's file alone,
/// if gpg does.
pub fn encrypt(plaintext: &[u8], epa: &Epa) -> io::Result<Vec<u8>> {
    let recipients = if epa.recipients.is_empty() {
        default_recipients()
    } else {
        epa.recipients.clone()
    };
    let mut args: Vec<&str> = vec!["--quiet", "--output", "-"];
    if recipients.is_empty() {
        args.push("--symmetric");
    } else {
        args.push("--encrypt");
        for r in &recipients {
            args.push("--recipient");
            args.push(r);
        }
    }
    let out = run(&args, Some(plaintext))?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(failure("can't encrypt", &out));
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    /// A stand-in for gpg: "encrypts" by reversing the bytes of stdin,
    /// "decrypts" by reversing the file, and lists a public-key packet.
    fn fake_gpg(dir: &Path) -> std::path::PathBuf {
        let script = dir.join("fakegpg");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case "$*" in
  *--list-packets*) echo ":pubkey enc packet: version 3, algo 1, keyid ABCD1234ABCD1234"; echo ":pubkey enc packet: version 3, algo 1, keyid 0000000000000000";;
  *--decrypt*) for last; do :; done; rev "$last";;
  *--encrypt*) echo "$*" > "$(dirname "$0")/args"; rev;;
  *--symmetric*) rev;;
  *) exit 2;;
esac
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    #[cfg(unix)]
    #[test]
    fn gpg_files_decrypt_on_visit_and_encrypt_on_save() {
        let dir = std::env::temp_dir().join(format!("forge-epa-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = fake_gpg(&dir);
        set_program(Some(program.to_str().unwrap()));

        let file = dir.join("secret.org.gpg");
        std::fs::write(&file, "terces\n").unwrap(); // "secret" reversed, as the fake stores it
        let mut b = Buffer::from_file(&file).unwrap();
        assert_eq!(b.get_contents(), "secret\n", "decrypted into the buffer");
        assert!(b.auto_save().is_ok());
        assert!(
            b.auto_save_path().is_none_or(|a| !a.exists()),
            "no plaintext auto-save"
        );

        b.insert_str("more ");
        b.save().unwrap();
        let on_disk = std::fs::read_to_string(&file).unwrap();
        assert!(!on_disk.contains("secret"), "the file stays encrypted: {on_disk:?}");
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert!(args.contains("--recipient ABCD1234ABCD1234"), "same recipients: {args}");
        assert!(!args.contains("0000000000000000"), "hidden recipients are skipped");

        let mut again = Buffer::from_file(&file).unwrap();
        assert!(again.get_contents().contains("more secret"));
        again.revert().unwrap();

        // A new .gpg file is symmetric unless recipients are set.
        let fresh = dir.join("new.gpg");
        let mut n = Buffer::from_file(&fresh).unwrap();
        n.insert_str("hi");
        n.save().unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap().trim(), "ih");

        set_program(None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names() {
        assert!(is_encrypted_name(Path::new("/a/b.org.gpg")));
        assert!(!is_encrypted_name(Path::new("/a/b.org")));
        assert!(!is_encrypted_name(Path::new("/a/.gpg")));
        assert_eq!(name_without_gpg("n.org.gpg"), Some("n.org"));
    }
}
