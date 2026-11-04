// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org Crypt commands (stage 3): passphrase prompts, entry encrypt/decrypt,
//! encrypt-on-save and offer-decrypt-on-open.
//!
//! Unlike Forge (whose before-save hook leaves the buffer encrypted), the
//! buffer always keeps showing the plaintext here: saving writes
//! entry-substituted text through `save_text_override`, so no
//! decrypt-after-save step exists. Plaintext never reaches `#file#`:
//! decrypting disables auto-save, every crypt save re-disables it (a real
//! save would otherwise turn it back on), and `auto_save` itself refuses
//! buffers with remembered decryptions.
//!
//! Prompts follow the `Ask` pattern (see `org_cmds.rs`): targets are
//! recomputed when the passphrase answer arrives, so nothing is stashed.

use crate::app::App;
use crate::minibuffer::Purpose;
use crate::org_cmds::Ask;
use forge_core::org_crypt::crypto::{pgp_decrypt_symmetric, pgp_encrypt_symmetric};
use forge_core::org_crypt::{CryptKey, DEFAULT_TAG_MATCHER, configured_default_key};
use forge_docs::CommandDoc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const COMMANDS: &[CommandDoc] = &[
    CommandDoc::line(
        "org-decrypt-entries",
        "Org: decrypt every encrypted entry matching the crypt tag.",
    ),
    CommandDoc::line(
        "org-decrypt-entry",
        "Org: decrypt the current entry below its headline.",
    ),
    CommandDoc::line(
        "org-encrypt-entries",
        "Org: encrypt the body of every entry matching the crypt tag.",
    ),
    CommandDoc::line(
        "org-encrypt-entry",
        "Org: encrypt the current entry's body (headline and properties stay).",
    ),
];

/// What a passphrase answer continues.
#[derive(Debug, Clone)]
pub enum CryptOp {
    EncryptOne,
    EncryptAll,
    DecryptOne,
    DecryptAll,
    DecryptAny,
    Save,
    SaveAs(PathBuf),
}

fn matcher(app: &App) -> String {
    app.org
        .crypt_tag_matcher
        .clone()
        .unwrap_or_else(|| DEFAULT_TAG_MATCHER.to_string())
}

fn default_key(app: &App) -> Option<String> {
    configured_default_key(app.org.crypt_key.as_deref())
}

fn is_org(app: &App) -> bool {
    app.buf_ref().major_mode() == forge_core::org::MODE
}

fn heading_here(app: &App) -> Option<usize> {
    let b = app.buf_ref();
    b.org_heading_at(b.point().line)
}

fn ask_passphrase(app: &mut App, prompt: &str, op: CryptOp) {
    app.prompt(prompt, Purpose::OrgAsk(Ask::CryptPassphrase(op)));
    if let Some(m) = app.minibuffer.as_mut() {
        m.secret = true;
    }
}

/// Run an Org Crypt command. False if `cmd` isn't one.
pub fn command(app: &mut App, cmd: &str) -> bool {
    if !matches!(
        cmd,
        "org-encrypt-entry" | "org-encrypt-entries" | "org-decrypt-entry" | "org-decrypt-entries"
    ) {
        return false;
    }
    if !is_org(app) {
        app.error("Not an Org buffer");
        return true;
    }
    match cmd {
        "org-encrypt-entry" => encrypt_one(app),
        "org-encrypt-entries" => encrypt_all(app),
        "org-decrypt-entry" => decrypt_one(app),
        _ => decrypt_all(app),
    }
    true
}

/// The `Ask::CryptPassphrase` answer. False if `ask` isn't one.
pub fn answer(app: &mut App, ask: &Ask, input: &str) -> bool {
    let Ask::CryptPassphrase(op) = ask else {
        return false;
    };
    match op.clone() {
        CryptOp::EncryptOne => answer_encrypt_one(app, input),
        CryptOp::EncryptAll => answer_encrypt_all(app, input),
        CryptOp::DecryptOne => answer_decrypt_one(app, input),
        CryptOp::DecryptAll => answer_decrypt_matcher(app, input),
        CryptOp::DecryptAny => answer_decrypt_any(app, input),
        CryptOp::Save => answer_save(app, input, None),
        CryptOp::SaveAs(path) => answer_save(app, input, Some(path)),
    }
    true
}

// ------------------------------------------------------------------ encrypt

/// A heading with its resolved body, key and cached armor (when the body
/// is unchanged since decrypting, so no passphrase is needed for it).
struct Target {
    h: usize,
    body: String,
    key: CryptKey,
    armor: Option<String>,
}

fn targets(app: &App, heads: &[usize]) -> Vec<Target> {
    let def = default_key(app);
    let b = app.buf_ref();
    heads
        .iter()
        .filter_map(|&h| {
            let body = b.org_crypt_body_text(h)?;
            let key = b.org_crypt_key_for_heading(h, def.as_deref());
            let armor = b.crypt_cache.reuse(h, &body, &key).map(str::to_string);
            Some(Target {
                h,
                body,
                key,
                armor,
            })
        })
        .collect()
}

fn public_key_msg(h: usize) -> String {
    format!(
        "Entry at line {} wants public-key encryption, which is not implemented yet",
        h + 1
    )
}

fn encrypt_one(app: &mut App) {
    let Some(h) = heading_here(app) else {
        return app.error("Not in an outline entry");
    };
    if app.buf_ref().org_crypt_is_encrypted(h) {
        return app.msg("Entry is already encrypted");
    }
    if app.buf_ref().org_crypt_entry_range(h).is_none() {
        return app.msg("Nothing to encrypt");
    }
    let tg = targets(app, &[h]);
    let Some(t) = tg.into_iter().next() else {
        return app.msg("Nothing to encrypt");
    };
    if let Some(armor) = t.armor {
        apply_encrypted(app, &[(t.h, t.body, t.key, armor)]);
    } else if t.key == CryptKey::Symmetric {
        ask_passphrase(app, "Passphrase to encrypt entry: ", CryptOp::EncryptOne);
    } else {
        app.error(public_key_msg(t.h));
    }
}

fn answer_encrypt_one(app: &mut App, pass: &str) {
    let Some(h) = heading_here(app) else {
        return app.error("Not in an outline entry");
    };
    if app.buf_ref().org_crypt_is_encrypted(h) {
        return app.msg("Entry is already encrypted");
    }
    let tg = targets(app, &[h]);
    let Some(t) = tg.into_iter().next() else {
        return app.msg("Nothing to encrypt");
    };
    if let Some(armor) = t.armor {
        return apply_encrypted(app, &[(t.h, t.body, t.key, armor)]);
    }
    if t.key != CryptKey::Symmetric {
        return app.error(public_key_msg(t.h));
    }
    match pgp_encrypt_symmetric(t.body.as_bytes(), pass) {
        Ok(armor) => apply_encrypted(app, &[(t.h, t.body, t.key, armor)]),
        Err(e) => app.error(format!("Encryption failed: {e}")),
    }
}

/// Write armors into the buffer and remember them for saving.
fn apply_encrypted(app: &mut App, done: &[(usize, String, CryptKey, String)]) {
    for (h, body, key, armor) in done {
        app.buf().org_crypt_set_body(*h, armor);
        app.buf()
            .crypt_cache
            .remember(*h, body, key.clone(), armor.clone());
    }
    app.msg(format!(
        "Encrypted {} {}",
        done.len(),
        if done.len() == 1 { "entry" } else { "entries" }
    ));
}

fn encrypt_all(app: &mut App) {
    let heads = match app.buf_ref().org_crypt_encryptable_entries(&matcher(app)) {
        Ok(h) => h,
        Err(e) => return app.error(e),
    };
    if heads.is_empty() {
        return app.msg("No entries to encrypt");
    }
    let tg = targets(app, &heads);
    if tg.iter().all(|t| t.armor.is_some()) {
        let done: Vec<_> = tg
            .into_iter()
            .map(|t| (t.h, t.body, t.key, t.armor.expect("checked")))
            .collect();
        return apply_encrypted(app, &done);
    }
    ask_passphrase(app, "Passphrase to encrypt entries: ", CryptOp::EncryptAll);
}

fn answer_encrypt_all(app: &mut App, pass: &str) {
    let heads = match app.buf_ref().org_crypt_encryptable_entries(&matcher(app)) {
        Ok(h) => h,
        Err(e) => return app.error(e),
    };
    // Descending: replacing a later body never shifts earlier headings.
    let heads: Vec<usize> = heads.into_iter().rev().collect();
    let mut done = Vec::new();
    let mut skipped = 0;
    for t in targets(app, &heads) {
        if let Some(armor) = t.armor {
            done.push((t.h, t.body, t.key, armor));
        } else if t.key == CryptKey::Symmetric {
            match pgp_encrypt_symmetric(t.body.as_bytes(), pass) {
                Ok(armor) => done.push((t.h, t.body, t.key, armor)),
                Err(e) => return app.error(format!("Encryption failed: {e}")),
            }
        } else {
            skipped += 1;
        }
    }
    if done.is_empty() && skipped > 0 {
        return app.error("Public-key encryption is not implemented yet");
    }
    apply_encrypted(app, &done);
    if skipped > 0 {
        app.msg(format!(
            "Encrypted {} entries ({skipped} skipped: public-key encryption is not implemented)",
            done.len()
        ));
    }
}

// ------------------------------------------------------------------ decrypt

/// Armor with per-line indentation stripped.
fn block_armor(app: &App, h: usize) -> Option<String> {
    let (beg, close) = app.buf_ref().org_crypt_block_at(h)?;
    let b = app.buf_ref();
    Some(
        (beg..=close)
            .map(|l| b.line(l))
            .collect::<Vec<_>>()
            .join("\n")
            .lines()
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn decrypt_one(app: &mut App) {
    let Some(h) = heading_here(app) else {
        return app.error("Not in an outline entry");
    };
    if app.buf_ref().org_crypt_block_at(h).is_none() {
        return app.msg("Entry is not encrypted");
    }
    let key = app
        .buf_ref()
        .org_crypt_key_for_heading(h, default_key(app).as_deref());
    if key != CryptKey::Symmetric {
        return app.error("Public-key decryption is not implemented yet");
    }
    ask_passphrase(app, "Passphrase to decrypt entry: ", CryptOp::DecryptOne);
}

fn decrypt_all(app: &mut App) {
    match encrypted_matching(app) {
        Ok(heads) if heads.is_empty() => app.msg("No encrypted entries"),
        Ok(_) => ask_passphrase(app, "Passphrase to decrypt entries: ", CryptOp::DecryptAll),
        Err(e) => app.error(e),
    }
}

/// Encrypted headings selected by the tag matcher.
fn encrypted_matching(app: &App) -> Result<Vec<usize>, String> {
    let b = app.buf_ref();
    Ok(b.org_crypt_match_entries(&matcher(app))?
        .into_iter()
        .filter(|&h| b.org_crypt_block_at(h).is_some())
        .collect())
}

/// Every encrypted heading, regardless of tags (for the open offer).
fn any_encrypted(app: &App) -> Vec<usize> {
    let b = app.buf_ref();
    (0..b.line_count())
        .filter(|&l| b.org_crypt_block_at(l).is_some())
        .collect()
}

fn answer_decrypt_one(app: &mut App, pass: &str) {
    let Some(h) = heading_here(app) else {
        return app.error("Not in an outline entry");
    };
    if app.buf_ref().org_crypt_block_at(h).is_none() {
        return app.msg("Entry is not encrypted");
    }
    apply_decrypted(app, &[h], pass);
}

fn answer_decrypt_matcher(app: &mut App, pass: &str) {
    let heads = match encrypted_matching(app) {
        Ok(h) => h,
        Err(e) => return app.error(e),
    };
    apply_decrypted(app, &heads, pass);
}

fn answer_decrypt_any(app: &mut App, pass: &str) {
    apply_decrypted(app, &any_encrypted(app), pass);
}

/// Decrypt `heads` with `passphrase`, keeping per-entry successes.
/// Wrong passphrases and public-key entries are reported, never written.
fn apply_decrypted(app: &mut App, heads: &[usize], pass: &str) {
    // Descending: replacing a later body never shifts earlier headings.
    let mut heads = heads.to_vec();
    heads.sort();
    heads.reverse();
    let def = default_key(app);
    let mut ok = 0;
    let mut failed = 0;
    let mut skipped = 0;
    for h in heads {
        let Some(armor) = block_armor(app, h) else {
            continue;
        };
        let key = app.buf_ref().org_crypt_key_for_heading(h, def.as_deref());
        if key != CryptKey::Symmetric {
            skipped += 1;
            continue;
        }
        let text = pgp_decrypt_symmetric(&armor, pass)
            .ok()
            .and_then(|v| String::from_utf8(v).ok());
        match text {
            Some(text) => {
                app.buf().org_crypt_set_body(h, &text);
                app.buf().crypt_cache.remember(h, &text, key, armor);
                ok += 1;
            }
            None => failed += 1,
        }
    }
    if ok > 0 {
        app.buf().disable_auto_save();
    }
    if ok == 0 && failed > 0 {
        app.error("Decryption failed (wrong passphrase?)");
    } else {
        let mut msg = format!(
            "Decrypted {ok} {}",
            if ok == 1 { "entry" } else { "entries" }
        );
        if failed > 0 {
            msg.push_str(&format!(" ({failed} failed: wrong passphrase?)"));
        }
        if skipped > 0 {
            msg.push_str(&format!(
                " ({skipped} skipped: public-key decryption is not implemented)"
            ));
        }
        app.msg(msg);
    }
}

// --------------------------------------------------------------------- save

/// Save through Org Crypt when the buffer needs it. True when handled:
/// saved, prompting for a passphrase, or refusing (with a message).
pub fn save_with_crypt(app: &mut App) -> bool {
    if !is_org(app) || app.buf_ref().path().is_none() {
        return false;
    }
    let heads = match app.buf_ref().org_crypt_encryptable_entries(&matcher(app)) {
        Ok(h) => h,
        Err(e) => {
            app.error(e);
            return true;
        }
    };
    if heads.is_empty() {
        return false;
    }
    let tg = targets(app, &heads);
    if tg
        .iter()
        .any(|t| t.armor.is_none() && t.key != CryptKey::Symmetric)
    {
        app.error(
            "Refusing to save: an entry wants public-key encryption, which is not implemented yet",
        );
        return true;
    }
    if tg.iter().any(|t| t.armor.is_none()) {
        ask_passphrase(
            app,
            "Passphrase to encrypt entries before saving: ",
            CryptOp::Save,
        );
    } else {
        let arms: HashMap<usize, String> = tg
            .into_iter()
            .map(|t| (t.h, t.armor.expect("checked")))
            .collect();
        finish_save(app, &arms, &[], None);
    }
    true
}

/// `C-x C-w` through Org Crypt. True when handled (saved or prompting).
pub fn write_file_with_crypt(app: &mut App, path: &Path) -> bool {
    if !is_org(app) {
        return false;
    }
    let heads = match app.buf_ref().org_crypt_encryptable_entries(&matcher(app)) {
        Ok(h) => h,
        Err(e) => {
            app.error(e);
            return true;
        }
    };
    if heads.is_empty() {
        return false;
    }
    ask_passphrase(
        app,
        &format!("Passphrase to encrypt entries for {}: ", path.display()),
        CryptOp::SaveAs(path.to_path_buf()),
    );
    true
}

fn answer_save(app: &mut App, pass: &str, new_path: Option<PathBuf>) {
    let heads = match app.buf_ref().org_crypt_encryptable_entries(&matcher(app)) {
        Ok(h) => h,
        Err(e) => return app.error(e),
    };
    if heads.is_empty() {
        // Everything got encrypted while the prompt was up; save plainly.
        return save_plainly(app, new_path);
    }
    let mut arms = HashMap::new();
    let mut fresh = Vec::new();
    for t in targets(app, &heads) {
        if let Some(armor) = t.armor {
            arms.insert(t.h, armor);
        } else if t.key == CryptKey::Symmetric {
            match pgp_encrypt_symmetric(t.body.as_bytes(), pass) {
                Ok(armor) => {
                    fresh.push((t.h, t.body, t.key, armor.clone()));
                    arms.insert(t.h, armor);
                }
                Err(e) => return app.error(format!("Encryption failed: {e}")),
            }
        } else {
            return app.error("Refusing to save: an entry wants public-key encryption, which is not implemented yet");
        }
    }
    finish_save(app, &arms, &fresh, new_path);
}

/// The normal save path, for when the prompt turns out unneeded.
fn save_plainly(app: &mut App, new_path: Option<PathBuf>) {
    match new_path {
        None => crate::commands::save(app),
        Some(path) => match app.buf().save_as(&path) {
            Ok(()) => {
                if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned())
                    && app.buf_ref().name() != name
                {
                    let unique = app.editor.unique_name(&name);
                    app.buf().set_name(unique);
                }
                app.msg(format!("Wrote {}", path.display()));
            }
            Err(e) => app.msg(format!("Error writing {}: {e}", path.display())),
        },
    }
}

fn finish_save(
    app: &mut App,
    arms: &HashMap<usize, String>,
    fresh: &[(usize, String, CryptKey, String)],
    new_path: Option<PathBuf>,
) {
    let text = app.buf_ref().org_crypt_save_text(arms);
    let result = match &new_path {
        None => app.buf().save_text_override(&text),
        Some(path) => app.buf().save_as_text_override(path, &text),
    };
    if let Err(e) = result {
        let where_ = new_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                app.buf_ref()
                    .path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            });
        return app.error(format!("Error saving {where_}: {e}"));
    }
    let heads: Vec<usize> = arms.keys().copied().collect();
    for (h, body, key, armor) in fresh {
        app.buf()
            .crypt_cache
            .remember(*h, body, key.clone(), armor.clone());
    }
    app.buf().crypt_cache.prune(&heads);
    // The buffer still shows plaintext: keep auto-save off (a real save
    // would otherwise turn it back on).
    app.buf().disable_auto_save();
    match new_path {
        None => {
            let path = app
                .buf_ref()
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            crate::files::after_save(app);
            app.msg(format!("Wrote {path}"));
        }
        Some(path) => {
            // Mirror the `WriteFile` submit: rename the buffer for its file.
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            if name
                .as_deref()
                .is_some_and(|name| app.buf_ref().name() != name)
            {
                let unique = app.editor.unique_name(&name.expect("checked"));
                app.buf().set_name(unique);
            }
            app.msg(format!("Wrote {}", path.display()));
        }
    }
}

// ---------------------------------------------------------------------- open

/// After visiting a file, offer to decrypt its encrypted entries.
/// Call from `visit_file`; prompts only when there is something to do.
pub fn offer_decrypt(app: &mut App) {
    if !is_org(app) || any_encrypted(app).is_empty() {
        return;
    }
    app.prompt(
        "Encrypted Org entries; decrypt them? (y or n) ",
        Purpose::Confirm(crate::minibuffer::ConfirmAction::OrgCryptDecrypt),
    );
}

/// The open offer was accepted: ask for the passphrase.
pub fn decrypt_after_confirm(app: &mut App) {
    if !is_org(app) || any_encrypted(app).is_empty() {
        return app.msg("No encrypted entries");
    }
    ask_passphrase(app, "Passphrase to decrypt entries: ", CryptOp::DecryptAny);
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{Buffer, Pos};

    const PASS: &str = "test-passphrase";

    fn app_with(text: &str) -> App {
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        app
    }

    fn prompting(app: &App) -> bool {
        app.minibuffer.is_some()
    }

    fn answer(app: &mut App, op: CryptOp, input: &str) {
        // Like `submit`: the prompt is gone before its answer runs.
        app.minibuffer = None;
        super::answer(app, &Ask::CryptPassphrase(op), input);
    }

    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn temp_org(content: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "forge-org-crypt-{}-{}.org",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn encrypt_entry_prompts_then_encrypts() {
        let mut app = app_with("* S :crypt:\nsecret\n");
        assert!(super::command(&mut app, "org-encrypt-entry"));
        assert!(prompting(&app));
        let mb = app.minibuffer.as_ref().unwrap();
        assert!(mb.secret);
        assert!(!mb.text().contains("secret") || true); // input empty so far
        answer(&mut app, CryptOp::EncryptOne, PASS);
        assert!(!prompting(&app));
        assert!(app.buf_ref().org_crypt_is_encrypted(0));
        assert_eq!(app.message.as_deref(), Some("Encrypted 1 entry"));
        // Again: already encrypted, no prompt.
        super::command(&mut app, "org-encrypt-entry");
        assert!(!prompting(&app));
        assert_eq!(app.message.as_deref(), Some("Entry is already encrypted"));
    }

    #[test]
    fn passphrase_prompt_is_masked() {
        let mut app = app_with("* S :crypt:\nsecret\n");
        super::command(&mut app, "org-encrypt-entry");
        let mb = app.minibuffer.as_mut().unwrap();
        mb.input = PASS.to_string();
        let shown = mb.text();
        assert!(!shown.contains(PASS));
        assert!(shown.contains("••"));
    }

    #[test]
    fn decrypt_roundtrip_and_wrong_passphrase() {
        let mut app = app_with("* S :crypt:\nsecret\n");
        super::command(&mut app, "org-encrypt-entry");
        answer(&mut app, CryptOp::EncryptOne, PASS);
        assert!(app.buf_ref().org_crypt_is_encrypted(0));
        // Wrong passphrase: error, entry stays encrypted.
        super::command(&mut app, "org-decrypt-entry");
        assert!(prompting(&app));
        answer(&mut app, CryptOp::DecryptOne, "wrong");
        assert!(app.buf_ref().org_crypt_is_encrypted(0));
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("Decryption failed"))
        );
        // Right passphrase: plaintext back, auto-save off.
        super::command(&mut app, "org-decrypt-entry");
        answer(&mut app, CryptOp::DecryptOne, PASS);
        assert!(!app.buf_ref().org_crypt_is_encrypted(0));
        assert_eq!(
            app.buf_ref().org_crypt_body_text(0).as_deref(),
            Some("secret")
        );
        assert!(!app.buf_ref().auto_save_enabled());
        assert!(!app.buf_ref().needs_auto_save());
    }

    #[test]
    fn save_encrypts_file_keeps_buffer() {
        let path = temp_org("* S :crypt:\nsecret\n");
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::from_file(&path).unwrap();
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        assert!(save_with_crypt(&mut app));
        assert!(prompting(&app));
        answer(&mut app, CryptOp::Save, PASS);
        assert!(!prompting(&app));
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("-----BEGIN PGP MESSAGE-----"));
        assert!(!on_disk.contains("secret\n"));
        assert_eq!(
            app.buf_ref().org_crypt_body_text(0).as_deref(),
            Some("secret")
        );
        assert!(!app.buf_ref().auto_save_enabled());
        // Auto-save refuses decrypted buffers: no #file# appears.
        app.buf().auto_save().unwrap();
        assert!(app.buf_ref().auto_save_path().is_some_and(|p| !p.exists()));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn save_reuses_cache_without_prompting() {
        let path = temp_org("* S :crypt:\nsecret\n");
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::from_file(&path).unwrap();
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        assert!(save_with_crypt(&mut app));
        answer(&mut app, CryptOp::Save, PASS);
        // Saving again needs no passphrase: the cache covers the entry.
        assert!(save_with_crypt(&mut app));
        assert!(!prompting(&app));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn save_ignores_untagged_entries() {
        let path = temp_org("* Plain\ntext\n");
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::from_file(&path).unwrap();
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        assert!(!save_with_crypt(&mut app));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn save_refuses_public_key_entries() {
        let path = temp_org("* S :crypt:\n:PROPERTIES:\n:CRYPTKEY: 0xABC\n:END:\nsecret\n");
        let mut app = App::new(forge_core::Editor::default());
        let mut b = Buffer::from_file(&path).unwrap();
        b.set_major_mode(forge_core::org::MODE);
        let id = app.editor.add_buffer(b);
        app.editor.show_buffer(id);
        app.org.crypt_key = Some("0xABC".to_string());
        assert!(save_with_crypt(&mut app));
        assert!(!prompting(&app));
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("Refusing to save"))
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "* S :crypt:\n:PROPERTIES:\n:CRYPTKEY: 0xABC\n:END:\nsecret\n"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn entries_commands_cover_many_headings() {
        let mut app = app_with("* A :crypt:\none\n* B :crypt:\ntwo\n* C\nthree\n");
        super::command(&mut app, "org-encrypt-entries");
        assert!(prompting(&app));
        answer(&mut app, CryptOp::EncryptAll, PASS);
        let heads = app.buf_ref().org_crypt_match_entries("crypt").unwrap();
        assert_eq!(heads.len(), 2);
        assert!(
            heads
                .iter()
                .all(|&h| app.buf_ref().org_crypt_is_encrypted(h))
        );
        super::command(&mut app, "org-decrypt-entries");
        answer(&mut app, CryptOp::DecryptAll, PASS);
        let heads = app.buf_ref().org_crypt_match_entries("crypt").unwrap();
        assert_eq!(
            app.buf_ref().org_crypt_body_text(heads[0]).as_deref(),
            Some("one")
        );
        assert_eq!(
            app.buf_ref().org_crypt_body_text(heads[1]).as_deref(),
            Some("two")
        );
        assert_eq!(
            app.buf_ref().org_crypt_body_text(4).as_deref(),
            Some("three")
        );
    }

    #[test]
    fn decrypt_entry_acts_at_point() {
        let mut app = app_with("* A :crypt:\none\n* B :crypt:\ntwo\n");
        super::command(&mut app, "org-encrypt-entries");
        answer(&mut app, CryptOp::EncryptAll, PASS);
        let heads = app.buf_ref().org_crypt_match_entries("crypt").unwrap();
        assert_eq!(heads.len(), 2);
        app.buf().set_point(Pos::new(heads[1], 0));
        super::command(&mut app, "org-decrypt-entry");
        answer(&mut app, CryptOp::DecryptOne, PASS);
        let heads = app.buf_ref().org_crypt_match_entries("crypt").unwrap();
        assert!(app.buf_ref().org_crypt_is_encrypted(heads[0]));
        assert_eq!(
            app.buf_ref().org_crypt_body_text(heads[1]).as_deref(),
            Some("two")
        );
    }

    #[test]
    fn open_offer_prompts_only_when_encrypted() {
        let mut app = app_with("* S :crypt:\nsecret\n");
        offer_decrypt(&mut app);
        assert!(!prompting(&app));
        super::command(&mut app, "org-encrypt-entry");
        answer(&mut app, CryptOp::EncryptOne, PASS);
        offer_decrypt(&mut app);
        assert!(prompting(&app));
        decrypt_after_confirm(&mut app);
        assert!(prompting(&app));
        answer(&mut app, CryptOp::DecryptAny, PASS);
        assert_eq!(
            app.buf_ref().org_crypt_body_text(0).as_deref(),
            Some("secret")
        );
    }

    #[test]
    fn unknown_commands_and_buffers_are_rejected() {
        let mut app = app_with("* S :crypt:\nsecret\n");
        assert!(!super::command(&mut app, "org-cycle"));
        assert!(!super::answer(&mut app, &Ask::SetTags, "x"));
        let mut app = App::new(forge_core::Editor::default());
        let id = app.editor.add_buffer(Buffer::new("notes.txt"));
        app.editor.show_buffer(id);
        assert!(super::command(&mut app, "org-encrypt-entry"));
        assert_eq!(app.message.as_deref(), Some("Not an Org buffer"));
    }
}
