// SPDX-License-Identifier: MIT OR Apache-2.0
//! Editor commands, named. Keys map to these names (keymap.rs)
//! and M-x runs them by name. Most take a prefix argument (C-u, M-5):
//! `app.count()` is its value, 1 if none was given.

use crate::app::{App, ReadKeyFor};
use crate::help;
use crate::keymap::describe_key_sequence;
use crate::minibuffer::{self, Choice, ConfirmAction, FileOp, Purpose};
use crate::query::ReplaceRequest;
use crate::{bookmarks, kmacro, plugins, registers, windows};
use forge_core::{Buffer, Pos, fill, text_nav};

/// Every command M-x knows about, with its documentation (shown by C-h f
/// and C-h k). Sorted by name.
pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::line(
        "add-log-current-defun",
        "Show the name of the defun at point, enclosing defuns joined by dots.",
    ),
    forge_docs::CommandDoc::new("apropos-command", "List the commands whose name matches every word you give (or a regexp),").body("with their keys and a line of documentation (C-h a)."),
    forge_docs::CommandDoc::line(
        "apropos-documentation",
        "List the commands whose documentation matches a regexp (C-h d).",
    ),
    forge_docs::CommandDoc::line(
        "async-shell-command",
        "Run a shell command in the background, its output in *Async Shell Command*.",
    ),
    forge_docs::CommandDoc::line(
        "back-to-indentation",
        "Go to the first character on the line that is not whitespace.",
    ),
    forge_docs::CommandDoc::line(
        "backward-char",
        "Move point one character backward. With an argument, move that many.",
    ),
    forge_docs::CommandDoc::new("backward-kill-sentence", "Kill the text back to the start of the sentence (with an argument N,").body("N sentences).\n\nRepeated kills are joined into one kill-ring entry."),
    forge_docs::CommandDoc::new("backward-kill-word", "Kill characters backward until the start of a word (with an argument,").body("that many words).\n\nRepeated kills are joined into one kill-ring entry."),
    forge_docs::CommandDoc::line(
        "backward-list",
        "Move backward over ARG lists, ending after their opening delimiters (C-M-p).",
    ),
    forge_docs::CommandDoc::line(
        "backward-page",
        "Move point to the previous page boundary (the formfeed, C-x [).",
    ),
    forge_docs::CommandDoc::line(
        "backward-paragraph",
        "Move point to the beginning of the paragraph before this one (M-{).",
    ),
    forge_docs::CommandDoc::line(
        "backward-sentence",
        "Move point back to the beginning of the current sentence (M-a).",
    ),
    forge_docs::CommandDoc::line(
        "backward-sexp",
        "Move backward over ARG sexps (C-M-b), using the buffer's tree-sitter grammar.",
    ),
    forge_docs::CommandDoc::line(
        "backward-word",
        "Move point backward to the start of a word. With an argument, that many words.",
    ),
    forge_docs::CommandDoc::line("balance-windows", "Make all windows the same size."),
    forge_docs::CommandDoc::line(
        "beginning-of-buffer",
        "Go to the top of the buffer; the mark stays where point was.",
    ),
    forge_docs::CommandDoc::line(
        "beginning-of-defun",
        "Go to the start of the function around point, or of the one before (C-M-a).",
    ),
    forge_docs::CommandDoc::line(
        "beginning-of-thing",
        "Ask for a kind of thing and go to the start of the current or previous one.",
    ),
    forge_docs::CommandDoc::line("bookmark-delete", "Forget a bookmark (TAB completes)."),
    forge_docs::CommandDoc::new("bookmark-jump", "Jump to a bookmark (TAB completes), opening its file when needed.").body("Point's old place goes on the mark, so C-x C-x jumps back."),
    forge_docs::CommandDoc::new("bookmark-set", "Remember this place under a name (C-x r m), kept between sessions.").body("C-x r b jumps back to it."),
    forge_docs::CommandDoc::line(
        "bounds-of-thing-at-point",
        "Ask for a thing and show the first and last character of the one at point.",
    ),
    forge_docs::CommandDoc::new("call-last-kbd-macro", "Run the last keyboard macro.").body("With an argument N, run it N times (0: until something fails)."),
    forge_docs::CommandDoc::new("capitalize-word", "Capitalize the word after point and move past it.").body("With an argument N, do N words; a negative N does the words before point without moving."),
    forge_docs::CommandDoc::new("center-line", "Center the current line within fill-column (N lines with an argument),").body("moving past them. Bound in Text mode and related modes."),
    forge_docs::CommandDoc::line(
        "clear-rectangle",
        "Replace the rectangle between mark and point with spaces.",
    ),
    forge_docs::CommandDoc::new(
        "clipboard-kill-region",
        "Kill the region and save it to the system clipboard.",
    )
    .body("Deletes the active region, puts it in the kill ring, and copies it to the system clipboard."),
    forge_docs::CommandDoc::new(
        "clipboard-kill-ring-save",
        "Copy the region to the kill ring and the system clipboard.",
    )
    .body("Copies the active region without deleting it, puts it in the kill ring, and copies it to the system clipboard."),
    forge_docs::CommandDoc::new(
        "clipboard-yank",
        "Insert the contents of the system clipboard at point.",
    )
    .body("Fetches text from the system clipboard, puts it in the kill ring, and inserts it at point."),
    forge_docs::CommandDoc::new("comment-dwim", "Comment or uncomment the region; with no region, go to this line's comment,").body("start one on a blank line, or add one at the end of the line."),
    forge_docs::CommandDoc::new("comment-line", "Comment or uncomment this line and move to the next (with an argument,").body("that many lines), or every line of the region."),
    forge_docs::CommandDoc::line(
        "compilation-next-error",
        "Move to the next line with a location.",
    ),
    forge_docs::CommandDoc::line(
        "compilation-previous-error",
        "Move to the previous line with a location.",
    ),
    forge_docs::CommandDoc::new("compile", "Run a command (default: the project's build) in the project root,").body("output in *compilation*; M-g n visits the errors."),
    forge_docs::CommandDoc::line(
        "compile-goto-error",
        "Visit the location on this line of *compilation* or *grep*.",
    ),
    forge_docs::CommandDoc::new("completion-at-point", "Complete the word before point: words from the open buffers and plugin sources").body("(a popup with corfu-like plugins)."),
    forge_docs::CommandDoc::new("context-menu-mode", "Toggle the context menu: right-click (or S-F10) pops up a menu of what makes").body("sense at the pointer.\n\nOff by default. forge.opt.context_menu = true turns it on from init.rhai."),
    forge_docs::CommandDoc::new("context-menu-open", "Open the context menu at point, so it can be used from the keyboard (S-F10).").body("The menu keys then work as in the menu bar: C-n, C-p, RET, ESC."),
    forge_docs::CommandDoc::line(
        "context-menu-select-all",
        "Context menu Select > All: mark the whole buffer.",
    ),
    forge_docs::CommandDoc::line(
        "context-menu-select-line",
        "Context menu Select > Line: mark the line that was clicked.",
    ),
    forge_docs::CommandDoc::line(
        "context-menu-select-none",
        "Context menu Select > None: deactivate the region.",
    ),
    forge_docs::CommandDoc::line(
        "context-menu-select-string",
        "Context menu Select > String: mark the double-quoted string that was clicked.",
    ),
    forge_docs::CommandDoc::line("copy-directory", "Copy a directory and all in it (into the target if that is a directory)."),
    forge_docs::CommandDoc::line(
        "copy-rectangle-as-kill",
        "Copy the rectangle between mark and point, for C-x r y.",
    ),
    forge_docs::CommandDoc::line(
        "copy-rectangle-to-register",
        "Copy the rectangle between mark and point into a register (C-u: delete it too).",
    ),
    forge_docs::CommandDoc::new("copy-to-register", "Copy the region into a register, named by the next key you type (C-u:").body("delete it too)."),
    forge_docs::CommandDoc::line("count-lines-page", "Show the number of lines on the current page (C-x l)."),
    forge_docs::CommandDoc::new("count-words", "Show how many lines, words and characters the region has,").body("or the whole buffer without one."),
    forge_docs::CommandDoc::line(
        "count-words-region",
        "Show how many lines, words and characters the region has.",
    ),
    forge_docs::CommandDoc::new("dabbrev-expand", "Complete the word at point from words in open buffers, nearest first.").body("Again: the next one."),
    forge_docs::CommandDoc::line(
        "delete-active-region",
        "Delete the text of the active region without saving it in the kill ring.",
    ),
    forge_docs::CommandDoc::new("delete-backward-char", "Delete the character before point, or the active region.").body("With an argument, kill that many characters (they go on the kill ring)."),
    forge_docs::CommandDoc::new("delete-char", "Delete the character after point, or the active region.").body("With an argument, kill that many characters (they go on the kill ring)."),
    forge_docs::CommandDoc::line("delete-directory", "Delete a directory; one with files in it asks before deleting them all."),
    forge_docs::CommandDoc::new("delete-frame", "Delete the selected frame (C-x 5 0). Refuses to delete the last frame.").body("Its buffers and windows stay; undelete-frame brings it back."),
    forge_docs::CommandDoc::new("delete-indentation", "Join this line to the previous one, fixing up the whitespace between them.").body("With C-u, join the next line to this one. With an active region, join all its lines."),
    forge_docs::CommandDoc::line(
        "delete-other-frames",
        "Delete every frame but the selected one (C-x 5 1).",
    ),
    forge_docs::CommandDoc::line(
        "delete-other-windows",
        "Make the selected window fill the frame.",
    ),
    forge_docs::CommandDoc::line(
        "delete-rectangle",
        "Delete the rectangle between mark and point without saving it.",
    ),
    forge_docs::CommandDoc::line(
        "delete-region",
        "Delete the region without saving it on the kill ring.",
    ),
    forge_docs::CommandDoc::new("delete-trailing-whitespace", "Delete blanks at the ends of lines (and blank lines at the end of the buffer),").body("in the region or the whole buffer."),
    forge_docs::CommandDoc::line(
        "delete-window",
        "Remove the selected window; the next one is selected.",
    ),
    forge_docs::CommandDoc::line(
        "describe-bindings",
        "Show every key binding active in the current buffer, grouped by keymap.",
    ),
    forge_docs::CommandDoc::new("describe-command", "Describe a command: its keys, menu entry and documentation.").body("Same as describe-function."),
    forge_docs::CommandDoc::line(
        "describe-face",
        "Describe a face and how the theme styles it. Same as describe-symbol.",
    ),
    forge_docs::CommandDoc::line(
        "describe-function",
        "Ask for a command name and show its documentation and key bindings.",
    ),
    forge_docs::CommandDoc::line(
        "describe-key",
        "Read a key sequence and show the command it runs, with its documentation.",
    ),
    forge_docs::CommandDoc::line(
        "describe-key-briefly",
        "Read a key sequence and name the command it runs in the echo area.",
    ),
    forge_docs::CommandDoc::line(
        "describe-mode",
        "Show the current major and minor modes and the keys they bind.",
    ),
    forge_docs::CommandDoc::new("describe-plugin", "Ask for a loaded plugin and show what it declares: its commands, permissions,").body("settings and their values."),
    forge_docs::CommandDoc::new("describe-symbol", "Ask for any name — a command, option, setting,").body("mode or face — and show everything known about it. The symbol at point is the default."),
    forge_docs::CommandDoc::new("describe-variable", "Describe an option: what it is for, its current value and what it takes.").body("Same as describe-symbol (C-h v)."),
    forge_docs::CommandDoc::line(
        "digit-argument",
        "Add a digit to the prefix argument being typed (M-0 .. M-9).",
    ),
    forge_docs::CommandDoc::new("dired", "Show a directory in a Dired buffer (default: this buffer's directory),").body("laid out like eza -l --header. RET visits (as does clicking a name, which is a region map: dired-entry), m/u/d mark, x deletes flagged, C/R/D copy, rename, delete, + makes a directory, ^ goes up, g reverts. keys are the defaults (* and % mark sets, ! shell, S H M Z, T touch, C-x C-q edits names). s cycles --sort, i lists one directory inline, and I/r///B switch inodes, reverse, only-dirs and icons. The eza toggles that clash with keys are on C-c C-: a dotfiles, d directories-first, t --tree, c gradient, g git-ignore, f only-files, l --level, k kill the inline listing."),
    forge_docs::CommandDoc::line("dired-change-marks", "In Dired: change every mark OLD to NEW (asks OLD NEW)."),
    forge_docs::CommandDoc::line("dired-clean-directory", "In Dired: flag numbered backups, keeping the newest two of each."),
    forge_docs::CommandDoc::line("dired-copy-filename-as-kill", "In Dired: put the file names on the kill ring (C-u: full paths)."),
    forge_docs::CommandDoc::line(
        "dired-create-directory",
        "In Dired: make a directory (and any missing parents).",
    ),
    forge_docs::CommandDoc::line("dired-diff", "In Dired: diff this file against another."),
    forge_docs::CommandDoc::line("dired-display-file", "In Dired: show the file in another window, keeping point here."),
    forge_docs::CommandDoc::line("dired-do-async-shell-command", "In Dired: run a shell command on the marked files in the background."),
    forge_docs::CommandDoc::line("dired-do-chgrp", "In Dired: change the group of the marked files."),
    forge_docs::CommandDoc::line("dired-do-chmod", "In Dired: change the mode of the marked files (755 or u+x,go-w)."),
    forge_docs::CommandDoc::line("dired-do-chown", "In Dired: change the owner of the marked files."),
    forge_docs::CommandDoc::line("dired-do-compress", "In Dired: gzip the marked files, or gunzip the .gz ones."),
    forge_docs::CommandDoc::line("dired-do-compress-to", "In Dired: pack the marked files into a .tar.gz, .tar or .zip."),
    forge_docs::CommandDoc::new("dired-do-copy", "In Dired: copy the marked files (or the one on this line) to a directory or new").body("name."),
    forge_docs::CommandDoc::line("dired-do-copy-regexp", "In Dired: copy the marked files with REGEXP => NEWNAME."),
    forge_docs::CommandDoc::new("dired-do-delete", "In Dired: delete the marked files (or the one on this line), asking first.").body("Directories must be empty."),
    forge_docs::CommandDoc::line("dired-do-find-marked-files", "In Dired: visit all the marked files."),
    forge_docs::CommandDoc::line("dired-do-find-regexp", "In Dired: search the marked files for a regexp."),
    forge_docs::CommandDoc::line("dired-do-find-regexp-and-replace", "In Dired: replace REGEXP => TEXT in the marked files, after one yes."),
    forge_docs::CommandDoc::line(
        "dired-do-flagged-delete",
        "In Dired: delete the files flagged with d, asking first.",
    ),
    forge_docs::CommandDoc::line("dired-do-hardlink", "In Dired: make hard links to the marked files."),
    forge_docs::CommandDoc::line("dired-do-hardlink-regexp", "In Dired: hard-link the marked files with REGEXP => NEWNAME."),
    forge_docs::CommandDoc::line("dired-do-kill-lines", "In Dired: remove the marked lines from the listing (files stay)."),
    forge_docs::CommandDoc::line("dired-do-print", "In Dired: print the marked files with lpr."),
    forge_docs::CommandDoc::line("dired-do-redisplay", "In Dired: redisplay the listing, keeping the marks."),
    forge_docs::CommandDoc::line("dired-do-relsymlink", "In Dired: make relative symbolic links to the marked files."),
    forge_docs::CommandDoc::line("dired-do-relsymlink-regexp", "In Dired: relative-symlink the marked files, REGEXP => NEWNAME."),
    forge_docs::CommandDoc::line(
        "dired-do-rename",
        "In Dired: move or rename the marked files (or the one on this line).",
    ),
    forge_docs::CommandDoc::line("dired-do-rename-regexp", "In Dired: rename the marked files with REGEXP => NEWNAME."),
    forge_docs::CommandDoc::new("dired-do-shell-command", "In Dired: run a shell command on the marked files (* = the files,").body("? runs it once per file); without either, the names are appended."),
    forge_docs::CommandDoc::line("dired-do-symlink", "In Dired: make symbolic links to the marked files."),
    forge_docs::CommandDoc::line("dired-do-symlink-regexp", "In Dired: symlink the marked files with REGEXP => NEWNAME."),
    forge_docs::CommandDoc::line("dired-do-touch", "In Dired: set the modification time of the marked files."),
    forge_docs::CommandDoc::line("dired-downcase", "In Dired: rename the marked files to lower case."),
    forge_docs::CommandDoc::line("dired-find-alternate-file", "In Dired: visit the file, replacing this Dired buffer."),
    forge_docs::CommandDoc::line(
        "dired-find-file",
        "In Dired: visit the file on this line, or show the directory in Dired.",
    ),
    forge_docs::CommandDoc::line(
        "dired-find-file-other-window",
        "In Dired: visit the file on this line in the other window.",
    ),
    forge_docs::CommandDoc::line("dired-flag-auto-save-files", "In Dired: flag auto-save files (#name#) for deletion."),
    forge_docs::CommandDoc::line("dired-flag-backup-files", "In Dired: flag backup files (name~) for deletion."),
    forge_docs::CommandDoc::new("dired-flag-file-deletion", "In Dired: flag this file for deletion (D) and move down;").body("x deletes the flagged files."),
    forge_docs::CommandDoc::line("dired-flag-files-regexp", "In Dired: flag for deletion the files whose names match a regexp."),
    forge_docs::CommandDoc::line("dired-goto-file", "In Dired: move to a file by name."),
    forge_docs::CommandDoc::line("dired-hide-all", "In Dired: hide the entries of every listing, or show them all."),
    forge_docs::CommandDoc::line("dired-hide-details-mode", "In Dired: show or hide the size, owner and date columns."),
    forge_docs::CommandDoc::line("dired-hide-subdir", "In Dired: hide or show the entries of the listing point is in."),
    forge_docs::CommandDoc::line(
        "dired-jump",
        "Show this buffer's directory in Dired, with point on this file.",
    ),
    forge_docs::CommandDoc::new("dired-kill-subdir", "In Dired: remove the inline listing point is in;").body("the directory stays in its parent."),
    forge_docs::CommandDoc::line("dired-mark", "In Dired: mark this file (*) and move down."),
    forge_docs::CommandDoc::line("dired-mark-directories", "In Dired: mark the directories (*)."),
    forge_docs::CommandDoc::line("dired-mark-executables", "In Dired: mark the executable files (*)."),
    forge_docs::CommandDoc::line("dired-mark-extension", "In Dired: mark the files with a given extension."),
    forge_docs::CommandDoc::line("dired-mark-files-containing-regexp", "In Dired: mark the files whose contents match a regexp."),
    forge_docs::CommandDoc::line("dired-mark-files-regexp", "In Dired: mark the files whose names match a regexp."),
    forge_docs::CommandDoc::line("dired-mark-subdir-files", "In Dired: mark every file of the listing point is in."),
    forge_docs::CommandDoc::line("dired-mark-symlinks", "In Dired: mark the symbolic links (*)."),
    forge_docs::CommandDoc::new("dired-maybe-insert-subdir", "In Dired: list the directory on this line inline as an eza tree,").body("below its parent in its own section (or go to its listing if it has one). C-c C-t draws everything as a tree instead."),
    forge_docs::CommandDoc::line("dired-next-dirline", "In Dired: move to the next directory line."),
    forge_docs::CommandDoc::line("dired-next-line", "In Dired: move to the next file line."),
    forge_docs::CommandDoc::line("dired-next-marked-file", "In Dired: move to the next marked file."),
    forge_docs::CommandDoc::line(
        "dired-next-subdir",
        "In Dired: move to the first file of the next inline listing.",
    ),
    forge_docs::CommandDoc::line("dired-omit-mode", "In Dired: hide or show backup and auto-save files."),
    forge_docs::CommandDoc::line(
        "dired-other-frame",
        "Read a directory in Dired and show it in another frame (C-x 5 d).",
    ),
    forge_docs::CommandDoc::line(
        "dired-other-tab",
        "Read a directory in Dired and show it in another tab (C-x t d).",
    ),
    forge_docs::CommandDoc::line("dired-prev-dirline", "In Dired: move to the previous directory line."),
    forge_docs::CommandDoc::line("dired-prev-marked-file", "In Dired: move to the previous marked file."),
    forge_docs::CommandDoc::line(
        "dired-prev-subdir",
        "In Dired: move to the first file of the previous inline listing.",
    ),
    forge_docs::CommandDoc::line("dired-previous-line", "In Dired: move to the previous file line."),
    forge_docs::CommandDoc::line(
        "dired-revert",
        "In Dired: read the directory again, keeping the marks.",
    ),
    forge_docs::CommandDoc::line("dired-show-file-type", "In Dired: show what kind of file this is (file(1))."),
    forge_docs::CommandDoc::new("dired-summary", "In Dired: show a one-line reminder of the commonest keys (?).").body("C-h m lists them all."),
    forge_docs::CommandDoc::new("dired-toggle-color-scale", "In Dired: shade sizes and dates by how large and how old they are among the").body("files listed, as eza's --color-scale does. Off by default."),
    forge_docs::CommandDoc::new("dired-toggle-dirs-first", "In Dired: list the directories before everything else, whatever the sort order,").body("as eza's --group-directories-first does."),
    forge_docs::CommandDoc::line(
        "dired-toggle-git-ignore",
        "In Dired: hide git-ignored files, as eza's --git-ignore does.",
    ),
    forge_docs::CommandDoc::line(
        "dired-toggle-hidden",
        "In Dired: show the dotfiles (eza's --all). They are hidden by default.",
    ),
    forge_docs::CommandDoc::new("dired-toggle-icons", "In Dired: show Nerd Font icons before names,").body("as eza's --icons does (FORGE_DIRED_ICONS=0 turns them off entirely). On by default."),
    forge_docs::CommandDoc::line(
        "dired-toggle-inodes",
        "In Dired: show each file's inode number (eza's --inode).",
    ),
    forge_docs::CommandDoc::new("dired-toggle-ls-style", "In Dired: switch between `ls -l` lines and eza's columns.").body("New Dired buffers start as `ls -al` lines; set forge.opt.dired_listing = \"eza\" for the columns instead."),
    forge_docs::CommandDoc::line("dired-toggle-marks", "In Dired: mark the unmarked files and unmark the marked ones."),
    forge_docs::CommandDoc::line(
        "dired-toggle-only-dirs",
        "In Dired: show only directories, as eza's -D does.",
    ),
    forge_docs::CommandDoc::line(
        "dired-toggle-only-files",
        "In Dired: show only files, as eza's -f does.",
    ),
    forge_docs::CommandDoc::line(
        "dired-toggle-reverse",
        "In Dired: reverse the sort order, as eza's --reverse does.",
    ),
    forge_docs::CommandDoc::new("dired-toggle-sort", "In Dired: cycle the sort order between name, date (newest first),").body("size (largest first) and extension, keeping marks and point (eza's --sort)."),
    forge_docs::CommandDoc::new("dired-toggle-tree", "In Dired: draw everything as a tree, recursing into every subdirectory,").body("as eza's --tree does (C-c C-t). i draws just one directory that way."),
    forge_docs::CommandDoc::new("dired-tree-set-level", "In Dired: set how deep the tree goes (1-8), as eza's --level does.").body("Turns the tree on."),
    forge_docs::CommandDoc::line(
        "dired-unmark",
        "In Dired: remove the mark on this line and move down.",
    ),
    forge_docs::CommandDoc::line(
        "dired-unmark-all-marks",
        "In Dired: remove every mark and flag.",
    ),
    forge_docs::CommandDoc::line("dired-unmark-backward", "In Dired: move up a file and unmark it."),
    forge_docs::CommandDoc::line(
        "dired-up-directory",
        "In Dired: show the parent directory, with point on this one.",
    ),
    forge_docs::CommandDoc::line("dired-upcase", "In Dired: rename the marked files to upper case."),
    forge_docs::CommandDoc::line("dired-view-file", "In Dired: visit the file read-only (or open the directory)."),
    forge_docs::CommandDoc::new("disable-theme", "Turn off the applied theme, showing the built-in palette again."),
    forge_docs::CommandDoc::line(
        "display-fill-column-indicator-mode",
        "Toggle a thin line at the fill column in this buffer.",
    ),
    forge_docs::CommandDoc::new("display-line-numbers-mode", "Toggle line numbers in this buffer.").body("With a prefix argument, relative line numbers."),
    forge_docs::CommandDoc::line(
        "down-list",
        "Move forward into ARG lists, just after their opening delimiters (C-M-d).",
    ),
    forge_docs::CommandDoc::new("downcase-word", "Convert the word after point to lower case and move past it.").body("With an argument N, do N words; a negative N does the words before point without moving."),
    forge_docs::CommandDoc::line(
        "electric-pair-mode",
        "Toggle inserting closing brackets and quotes as you type the opening ones.",
    ),
    forge_docs::CommandDoc::new("electric-quote-local-mode", "Toggle typing curved quotes in this buffer: `` and '' become").body("curved double quotes, a lone ` and ' become curved single quotes. Off by default."),
    forge_docs::CommandDoc::new("electric-quote-mode", "Toggle typing curved quotes in every buffer (electric-quote-local-mode").body("in each one). A prefix argument turns it on, never off."),
    forge_docs::CommandDoc::new("end-kbd-macro", "Finish defining a keyboard macro.").body("With an argument N, also run it N-1 more times."),
    forge_docs::CommandDoc::line(
        "end-of-buffer",
        "Go to the bottom of the buffer; the mark stays where point was.",
    ),
    forge_docs::CommandDoc::line(
        "end-of-defun",
        "Move to the end of the current or next defun (C-M-e).",
    ),
    forge_docs::CommandDoc::line(
        "end-of-thing",
        "Ask for a thing and move to the end of the current or next one.",
    ),
    forge_docs::CommandDoc::line(
        "enlarge-window",
        "Make the selected window taller by one line (with an argument, that many).",
    ),
    forge_docs::CommandDoc::line(
        "enlarge-window-horizontally",
        "Make the selected window wider by one column (with an argument, that many).",
    ),
    forge_docs::CommandDoc::new("eval-expression", "Read a Rhai expression or statement in the minibuffer and run it in init.rhai's").body("Rhai,\n\nshowing the value. forge.* is available: M-: forge.buffer.current().line_count()"),
    forge_docs::CommandDoc::line(
        "eval-rhai-last-sexp",
        "Evaluate the Rhai expression before point and show its value (C-x C-e).",
    ),
    forge_docs::CommandDoc::line(
        "eval-rhai-print-last-sexp",
        "Evaluate the Rhai expression before point and insert its value (C-j).",
    ),
    forge_docs::CommandDoc::line(
        "exchange-point-and-mark",
        "Put the mark where point is and point where the mark was.",
    ),
    forge_docs::CommandDoc::new("execute-extended-command", "Read a command name in the minibuffer (TAB completes) and run it.").body("A prefix argument typed before M-x goes to that command."),
    forge_docs::CommandDoc::new("fill-paragraph", "Refill the paragraph at point so no line is longer than fill-column.").body("Keeps indentation, comment markers (// # ; >) and list bullets. With an active region, fill every paragraph in it. With an argument, justify the lines too."),
    forge_docs::CommandDoc::new("fill-region", "Re-wrap every paragraph in the region, as M-q does for one.").body("With an argument, justify the lines too."),
    forge_docs::CommandDoc::new("fill-region-as-paragraph", "Fill the region as one paragraph, combining its lines").body("and deleting blank lines inside it. With an argument, justify too."),
    forge_docs::CommandDoc::new("fill-reindent-defun", "Refill the comment or string at point, or reindent the whole defun."),
    forge_docs::CommandDoc::new("find-file", "Visit a file in its own buffer.").body("Relative paths and completion start in the visiting buffer's directory; conflicting file locks are reported and can be taken with confirmation."),
    forge_docs::CommandDoc::line(
        "find-file-other-frame",
        "Visit a file in another frame, making one if needed (C-x 5 f).",
    ),
    forge_docs::CommandDoc::line(
        "find-file-other-tab",
        "Visit a file in another tab (C-x t f).",
    ),
    forge_docs::CommandDoc::line(
        "find-file-other-window",
        "Visit a file in another window (splitting if there is only one).",
    ),
    forge_docs::CommandDoc::line(
        "first-error",
        "Visit the first location in the last *compilation* or *grep* output.",
    ),
    forge_docs::CommandDoc::new("forward-char", "Move point one character forward. With an argument, move that many.").body("A character is what you see as one: an emoji or an accented letter counts once."),
    forge_docs::CommandDoc::line(
        "forward-list",
        "Move forward over ARG lists (C-M-n), using the buffer's tree-sitter grammar.",
    ),
    forge_docs::CommandDoc::line(
        "forward-page",
        "Move point to the end of the current page (the formfeed, C-x ]).",
    ),
    forge_docs::CommandDoc::line(
        "forward-paragraph",
        "Move point to the beginning of the next paragraph (M-}).",
    ),
    forge_docs::CommandDoc::line(
        "forward-sentence",
        "Move point forward to the end of the sentence (M-e).",
    ),
    forge_docs::CommandDoc::line(
        "forward-sexp",
        "Move forward over ARG sexps (C-M-f), using the buffer's tree-sitter grammar.",
    ),
    forge_docs::CommandDoc::line(
        "forward-word",
        "Move point forward to the end of a word. With an argument, that many words.",
    ),
    forge_docs::CommandDoc::line(
        "frames-on-display",
        "Say which frames are on the current display.",
    ),
    forge_docs::CommandDoc::line(
        "global-display-fill-column-indicator-mode",
        "Toggle the fill column line in every buffer.",
    ),
    forge_docs::CommandDoc::new("global-display-line-numbers-mode", "Toggle line numbers in every buffer that doesn't choose (not in special").body("buffers). With a prefix argument, relative."),
    forge_docs::CommandDoc::line(
        "global-hl-line-mode",
        "Toggle highlighting the line point is on, in every buffer.",
    ),
    forge_docs::CommandDoc::new("global-set-key", "Read a key sequence and a command, and bind them in the global keymap.").body("The binding lasts for this session; put forge.keymap.set(...) in init.rhai to keep it."),
    forge_docs::CommandDoc::new("goto-char", "Move point to a buffer position (a character count from the start,").body("counting from 1)."),
    forge_docs::CommandDoc::new("goto-line", "Ask for a line number and move point there, leaving the mark where point was.").body("With a numeric argument, go to that line without asking."),
    forge_docs::CommandDoc::line(
        "grep",
        "Run grep (or a command like it) in the project root; M-g n visits the matches.",
    ),
    forge_docs::CommandDoc::line("grep-edit", "Make the *grep* buffer writable, so the matches can be edited (the `e` key)."),
    forge_docs::CommandDoc::line("grep-edit-exit", "Leave Grep Edit mode; the *grep* buffer is read-only again (C-c C-c)."),
    forge_docs::CommandDoc::new("grep-find", "Run a command that finds and greps in a directory tree.").body("Also available as find-grep. The whole command line is asked for, as for M-x grep."),
    forge_docs::CommandDoc::line("grep-find-toggle-abbreviation", "Show the whole grep command, or the abbreviated one again."),
    forge_docs::CommandDoc::line(
        "help-follow-link",
        "Follow the link at point in a help buffer (RET and <mouse-1>).",
    ),
    forge_docs::CommandDoc::line(
        "help-for-help",
        "Show what each C-h key does (C-h ? and C-h C-h).",
    ),
    forge_docs::CommandDoc::line(
        "help-next-link",
        "Move point to the next link in a help buffer (TAB).",
    ),
    forge_docs::CommandDoc::line(
        "help-prev-link",
        "Move point to the previous link in a help buffer (S-TAB).",
    ),
    forge_docs::CommandDoc::line(
        "help-with-tutorial",
        "Open the interactive tutorial, resuming where you left off (C-h t).",
    ),
    forge_docs::CommandDoc::line(
        "hl-line-mode",
        "Toggle highlighting the line point is on in this buffer.",
    ),
    forge_docs::CommandDoc::new("horizontal-scroll-bar-mode", "Toggle a horizontal scroll bar on the bottom of every window.").body("Clicking left or right of the thumb scrolls by a few columns; dragging the thumb scrolls continuously. With a prefix argument, turn it on."),
    forge_docs::CommandDoc::line("image-decrease-size", "Make the image at point smaller (i -)."),
    forge_docs::CommandDoc::line("image-decrease-speed", "Play an animation slower (a -)."),
    forge_docs::CommandDoc::line("image-dired-display-thumbs", "In Dired: show the marked (or all) images in a pictures buffer."),
    forge_docs::CommandDoc::line("image-dired-thumbnail-open", "In the image-dired buffer: visit the picture on this line."),
    forge_docs::CommandDoc::line("image-flip-horizontally", "Flip the image horizontally (i h)."),
    forge_docs::CommandDoc::line("image-flip-vertically", "Flip the image vertically (i v)."),
    forge_docs::CommandDoc::line("image-goto-frame", "Go to a frame of an animated image (F)."),
    forge_docs::CommandDoc::line("image-increase-size", "Make the image at point larger (i +)."),
    forge_docs::CommandDoc::line("image-increase-speed", "Play an animation faster (a +)."),
    forge_docs::CommandDoc::new("image-mode-copy-file-name-as-kill", "Copy the image file's name to the kill ring (w)."),
    forge_docs::CommandDoc::new("image-mode-mark-file", "Mark the image's file in its Dired buffer (m)."),
    forge_docs::CommandDoc::new("image-mode-unmark-file", "Remove the mark on the image's file (u)."),
    forge_docs::CommandDoc::new("image-next-file", "Visit the next image file in the directory (n)."),
    forge_docs::CommandDoc::new("image-next-frame", "Show the next frame of an animated image (f)."),
    forge_docs::CommandDoc::new("image-previous-file", "Visit the previous image file in the directory (p)."),
    forge_docs::CommandDoc::new("image-previous-frame", "Show the previous frame of an animated image (b)."),
    forge_docs::CommandDoc::new("image-reset-speed", "Reset an animation's speed to normal (a 0)."),
    forge_docs::CommandDoc::new("image-reverse-speed", "Reverse an animation's speed (a r)."),
    forge_docs::CommandDoc::new("image-rotate", "Rotate the image 90 degrees clockwise (i r)."),
    forge_docs::CommandDoc::new("image-save", "Save the image to a file (i o)."),
    forge_docs::CommandDoc::new("image-toggle-animation", "Start or stop an animated image (RET)."),
    forge_docs::CommandDoc::new("image-toggle-display", "Toggle between the image and its raw text (C-c C-c)."),
    forge_docs::CommandDoc::new("image-toggle-hex-display", "Toggle between the image and its hex (C-c C-x)."),
    forge_docs::CommandDoc::new("image-transform-fit-to-window", "Fit the image to the window (s w)."),
    forge_docs::CommandDoc::new("image-transform-reset-to-initial", "Reset the image's transform to the initial state (s 0)."),
    forge_docs::CommandDoc::new("image-transform-reset-to-original", "Reset the image's transform to the original (s o)."),
    forge_docs::CommandDoc::new("image-transform-set-percent", "Scale the image to a percentage (s p)."),
    forge_docs::CommandDoc::new("image-transform-set-scale", "Scale the image by a factor (s s)."),
    forge_docs::CommandDoc::line(
        "imenu",
        "Jump to a function, type or method in this buffer (from the syntax tree).",
    ),
    forge_docs::CommandDoc::new("increment-register", "Add the prefix argument (default 1) to a number register,").body("or append the region to a text register."),
    forge_docs::CommandDoc::new("indent-for-tab-command", "Indent this line the way the major mode says (with an active region,").body("every line in it).\n\nIn a mode without indentation rules, insert spaces up to the next tab stop."),
    forge_docs::CommandDoc::line(
        "indent-region",
        "Re-indent every line in the region by the major mode's rules.",
    ),
    forge_docs::CommandDoc::new("info", "Show the manual in an Info-style browser (*Manual*; C-h i).").body("n/p move to the next and previous node, u goes up a level, t the Top node, g jumps to a node by name, RET follows a link, TAB moves between links, l/r walk the history, m picks from the menu, s searches, i looks up an index entry."),
    forge_docs::CommandDoc::line("info-backward-node", "Go to the previous node of the manual, in document order ([)."),
    forge_docs::CommandDoc::line("info-follow-reference", "Follow the link at point: a node opens it, a symbol is described (RET)."),
    forge_docs::CommandDoc::line("info-forward-node", "Go to the next node of the manual, in document order (])."),
    forge_docs::CommandDoc::line("info-goto-node", "Ask for a node of the manual (TAB completes) and open it (g)."),
    forge_docs::CommandDoc::line("info-history-forward", "Go forward again to the node you left with info-last (r)."),
    forge_docs::CommandDoc::line("info-index", "Ask for an index term of the manual and open the node for it (i)."),
    forge_docs::CommandDoc::line("info-last", "Go back to the node before the last move, like a browser's Back (l)."),
    forge_docs::CommandDoc::line("info-lookup-command", "Ask for a command and open the manual node that documents it (C-h R)."),
    forge_docs::CommandDoc::line("info-menu", "Pick one of the current node's menu items and open it (m)."),
    forge_docs::CommandDoc::line("info-next", "Go to the next node: the next menu item, then the next in order (n)."),
    forge_docs::CommandDoc::line("info-next-reference", "Move point to the next link in the manual (TAB)."),
    forge_docs::CommandDoc::line("info-prev", "Go to the previous node: the previous menu item, then the previous in order (p)."),
    forge_docs::CommandDoc::line("info-prev-reference", "Move point to the previous link in the manual (S-TAB)."),
    forge_docs::CommandDoc::line("info-quit", "Leave the manual, back to the buffer before it (q)."),
    forge_docs::CommandDoc::line("info-search", "Search the manual for a regexp and open the node that matches (s)."),
    forge_docs::CommandDoc::line("info-toc", "Show the whole manual as one menu of links (T)."),
    forge_docs::CommandDoc::line("info-top", "Go to the manual's Top node (t)."),
    forge_docs::CommandDoc::line("info-up", "Go up to the node this one is a section of (u)."),
    forge_docs::CommandDoc::new("insert-directory", "Insert a directory listing (ls -al style) at point,").body("for a directory or a wildcard such as src/*.rs. Set insert_directory_program to use your own ls."),
    forge_docs::CommandDoc::new("insert-file", "Insert the contents of a file at point, leaving point before it and the mark").body("after (C-x i)."),
    forge_docs::CommandDoc::new("insert-register", "Insert a register's contents: point before,").body("mark after (C-u: the other way round)."),
    forge_docs::CommandDoc::new("isearch-backward", "Search backward incrementally as you type.").body("C-r finds the previous match, C-s searches forward, M-% replaces, RET ends, C-g goes back."),
    forge_docs::CommandDoc::new("isearch-forward", "Search forward incrementally as you type.").body("C-s finds the next match, C-r searches backward, M-% replaces, RET ends, C-g goes back."),
    forge_docs::CommandDoc::line("join-line", "Same as delete-indentation."),
    forge_docs::CommandDoc::line(
        "jump-to-register",
        "Go to the position saved in a register (C-x r SPC), switching buffers if needed.",
    ),
    forge_docs::CommandDoc::line(
        "keyboard-quit",
        "Cancel what is going on and deactivate the mark.",
    ),
    forge_docs::CommandDoc::line(
        "kill-buffer",
        "Ask for a buffer name and kill that buffer (default: the current one).",
    ),
    forge_docs::CommandDoc::line("kill-compilation", "Stop the running compilation or grep."),
    forge_docs::CommandDoc::line("kill-grep", "Kill the running grep."),
    forge_docs::CommandDoc::new("kill-line", "Kill the rest of the line, or the line break if only whitespace is left.").body("With an argument N, kill N whole lines forward (0: back to the line start; negative: backward).\nRepeated kills are joined into one kill-ring entry."),
    forge_docs::CommandDoc::line(
        "kill-rectangle",
        "Delete the rectangle between mark and point and save it for C-x r y.",
    ),
    forge_docs::CommandDoc::line("kill-region", "Kill the text between point and mark."),
    forge_docs::CommandDoc::line(
        "kill-ring-save",
        "Copy the region to the kill ring without deleting it.",
    ),
    forge_docs::CommandDoc::new("kill-sentence", "Kill from point to the end of the sentence (with an argument N, that many").body("sentences; negative, back to the start of the Nth previous).\n\nRepeated kills are joined into one kill-ring entry."),
    forge_docs::CommandDoc::new("kill-sexp", "Kill ARG sexps forward from point (C-M-k).").body("A negative argument kills backward. Repeated kills are joined into one kill-ring entry."),
    forge_docs::CommandDoc::line(
        "kill-this-buffer",
        "Kill the current buffer (asking first when it has unsaved changes).",
    ),
    forge_docs::CommandDoc::new("kill-word", "Kill from point to the end of the word (with an argument,").body("that many words)."),
    forge_docs::CommandDoc::new("kmacro-end-and-call-macro", "End the macro being defined, if any, then run the last macro.").body("After it, pressing e runs it again. With an argument N, run it N times (0: until something fails)."),
    forge_docs::CommandDoc::line(
        "kmacro-end-or-call-macro",
        "End the macro being defined, or run the last one.",
    ),
    forge_docs::CommandDoc::new("lgrep", "Search the files you name for a pattern.").body("The pattern and the files are asked for separately; the command is built from forge.opt.grep_template."),
    forge_docs::CommandDoc::line(
        "list-bookmarks",
        "Show the bookmarks and the files they point at.",
    ),
    forge_docs::CommandDoc::line("list-buffers", "Show a list of all buffers."),
    forge_docs::CommandDoc::new("list-packages", "Show the packages in the package indexes (forge.opt.package_index):").body("RET installs or upgrades, d deletes, g refreshes, U upgrades all."),
    forge_docs::CommandDoc::new("list-plugins", "Show the loaded plugins: their commands, the permissions they have,").body("their memory limit."),
    forge_docs::CommandDoc::new("list-projects", "Show the projects that have key bindings of their own,").body("and whether each one may run its .forge/init.rhai."),
    forge_docs::CommandDoc::line("list-registers", "Show what every register holds."),
    forge_docs::CommandDoc::new("list-themes", "Show the defined themes and which one is applied (*Themes*)."),
    forge_docs::CommandDoc::new("load-plugin", "Load a plugin (a WebAssembly component, *.wasm) and run its init.").body("Its commands, modes and key bindings become available; loading one with the same name replaces it."),
    forge_docs::CommandDoc::new("load-theme", "Ask for a defined theme and apply it.").body("Themes come from init.rhai, ~/.config/forge/themes/, or a plugin (forge.theme.define)."),
    forge_docs::CommandDoc::new("local-set-key", "Read a key sequence and a command, and bind them in this buffer only.").body("Buffer-local bindings take priority over every other keymap."),
    forge_docs::CommandDoc::line("make-directory", "Make a directory, and any parents it needs (also mkdir)."),
    forge_docs::CommandDoc::line("make-empty-file", "Make an empty file, and its directory if needed; an error if it exists."),
    forge_docs::CommandDoc::new("make-frame", "Make a new frame (a separate top-level display) showing the current buffer (C-x").body("5 2)."),
    forge_docs::CommandDoc::line(
        "make-frame-command",
        "Make a new frame showing the current buffer (C-x 5 2).",
    ),
    forge_docs::CommandDoc::new("make-frame-on-display", "Ask for a display and make a new frame on it.").body("Forge has one display, so anything else is an error."),
    forge_docs::CommandDoc::line(
        "make-frame-on-monitor",
        "Ask for a monitor and make a new frame on it.",
    ),
    forge_docs::CommandDoc::new("manual", "Show the manual in an Info-style browser (*Manual*; C-h r).").body("Alias of info, kept so the manual has a name of its own."),
    forge_docs::CommandDoc::line(
        "mark-defun",
        "Put point at the start of the current or next defun and mark its end (C-M-h).",
    ),
    forge_docs::CommandDoc::line(
        "mark-page",
        "Put point at the beginning of the page and the mark at the end (C-x C-p).",
    ),
    forge_docs::CommandDoc::line(
        "mark-paragraph",
        "Put point at the beginning of the paragraph and the mark at the end (M-h).",
    ),
    forge_docs::CommandDoc::line(
        "mark-whole-buffer",
        "Put point at the beginning of the buffer and the mark at the end.",
    ),
    forge_docs::CommandDoc::line(
        "mark-word",
        "Put the mark at the end of the word after point (M-@).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-choose",
        "Run the highlighted menu item's command and close the menu (RET, TAB in a menu).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-close",
        "Close the open menu, leaving the keyboard alone (ESC in a menu).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-first-item",
        "Highlight the first item of the open menu (<home>).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-last-item",
        "Highlight the last item of the open menu (<end>).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-next-item",
        "Highlight the next item of the open menu (C-n, <down>).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-next-menu",
        "Open the next menu in the bar (C-f, <right> in a menu).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-prev-item",
        "Highlight the previous item of the open menu (C-p, <up>).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-prev-menu",
        "Open the previous menu in the bar (C-b, <left> in a menu).",
    ),
    forge_docs::CommandDoc::line(
        "menu-bar-menu-quit",
        "Close the open menu and say Quit, the way keyboard-quit does (C-g in a menu).",
    ),
    forge_docs::CommandDoc::new("menu-bar-mode", "Toggle the menu bar at the top of the frame.").body("F10 opens it; with a prefix argument, turn it on (F10 and the mouse then work)."),
    forge_docs::CommandDoc::new("menu-bar-open", "Open the menu bar's first menu and put the keyboard in it (F10, M-`).").body("While a menu is already open, move to the next one."),
    forge_docs::CommandDoc::line("mkdir", "Make a directory, and any parents it needs (make-directory)."),
    forge_docs::CommandDoc::new("modifier-bar-mode", "Toggle the modifier bar: buttons for C-, M-, S-, H-, s-.").body("A clicked modifier applies to the next key, and several combine.\n\nIt's part of the tool bar, so tool-bar-mode must be on. It's a global mode only graphical frames show, like tool bar."),
    forge_docs::CommandDoc::line(
        "mouse-yank-at-click",
        "Paste (yank) the latest kill at the place the context menu was opened.",
    ),
    forge_docs::CommandDoc::new("move-beginning-of-line", "Move point to the beginning of the line.").body("With an argument N, move N-1 lines down first."),
    forge_docs::CommandDoc::new("move-end-of-line", "Move point to the end of the line.").body("With an argument N, move N-1 lines down first."),
    forge_docs::CommandDoc::line(
        "negative-argument",
        "Make the prefix argument negative (M--).",
    ),
    forge_docs::CommandDoc::new("newline", "Insert a line break (with an argument, that many) and indent the new line.").body("forge.opt.electric_indent = false turns the indenting off."),
    forge_docs::CommandDoc::line(
        "newline-without-indent",
        "Insert a line break and nothing else (with an argument, that many).",
    ),
    forge_docs::CommandDoc::line(
        "next-buffer",
        "Switch to the next buffer in the buffer list.",
    ),
    forge_docs::CommandDoc::new("next-error", "Visit the next location in the last *compilation* or *grep* output (with an").body("argument, the Nth next)."),
    forge_docs::CommandDoc::new("next-line", "Move point down one line, keeping the column if it can.").body("With an argument, that many lines."),
    forge_docs::CommandDoc::line(
        "number-to-register",
        "Store the prefix argument (default 0) in a register.",
    ),
    forge_docs::CommandDoc::line(
        "occur",
        "Show every line matching a regexp in *Occur*; RET there goes to the line.",
    ),
    forge_docs::CommandDoc::line(
        "occur-mode-goto-occurrence",
        "Go to the line this *Occur* line came from.",
    ),
    forge_docs::CommandDoc::new("open-line", "Insert a line break after point, leaving point where it is (with an argument,").body("that many)."),
    forge_docs::CommandDoc::new("open-rectangle", "Insert blank space filling the rectangle between mark and point,").body("shifting text right."),
    forge_docs::CommandDoc::line(
        "org-backward-heading-same-level",
        "Org: go to the previous heading at this level.",
    ),
    forge_docs::CommandDoc::new("org-clone-subtree-with-time-shift", "Org: duplicate the subtree below itself, shifting YYYY-MM-DD dates by the prefix").body("argument (days)."),
    forge_docs::CommandDoc::line(
        "org-copy-subtree",
        "Org: copy the subtree to the kill ring.",
    ),
    forge_docs::CommandDoc::line("org-ctrl-c-ctrl-c", "Org: do the useful thing for the context at point."),
    forge_docs::CommandDoc::new("org-ctrl-c-star", "Org: in a table, recalculate the current line; elsewhere turn the").body("heading into a list item or the item into a heading."),
    forge_docs::CommandDoc::line("org-cut-subtree", "Org: cut the subtree to the kill ring."),
    forge_docs::CommandDoc::new("org-cycle", "Org: on a heading, cycle its subtree folded, children,").body("everything; elsewhere indent."),
    forge_docs::CommandDoc::line(
        "org-cycle-force-archived",
        "Org: like org-cycle, showing archived subtrees too.",
    ),
    forge_docs::CommandDoc::line(
        "org-cycle-set-startup-visibility",
        "Org: fold the buffer contents view (with an argument, show everything).",
    ),
    forge_docs::CommandDoc::line(
        "org-demote-subtree",
        "Org: demote the heading and its whole subtree.",
    ),
    forge_docs::CommandDoc::line("org-do-demote", "Org: demote this heading line only."),
    forge_docs::CommandDoc::line("org-do-promote", "Org: promote this heading line only."),
    forge_docs::CommandDoc::line("org-fold-show-all", "Org: show everything in the buffer."),
    forge_docs::CommandDoc::line(
        "org-forward-heading-same-level",
        "Org: go to the next heading at this level.",
    ),
    forge_docs::CommandDoc::line("org-goto", "Org: pick a heading to visit (C-c C-j)."),
    forge_docs::CommandDoc::line(
        "org-goto-sibling",
        "Org: go to the next sibling heading (with an argument, the previous).",
    ),
    forge_docs::CommandDoc::line("org-hide-subtree", "Org: hide the subtree at point."),
    forge_docs::CommandDoc::line(
        "org-indent-mode",
        "Org: show body text aligned under its headline, with leading stars hidden.",
    ),
    forge_docs::CommandDoc::line(
        "org-insert-drawer",
        "Org: insert a :NAME: ... :END: drawer after this line.",
    ),
    forge_docs::CommandDoc::line(
        "org-insert-heading",
        "Org: a new heading after the current subtree, at the same level.",
    ),
    forge_docs::CommandDoc::line(
        "org-insert-heading-respect-content",
        "Org: a new heading after this entry's text, before its children.",
    ),
    forge_docs::CommandDoc::line(
        "org-insert-todo-heading",
        "Org: a new TODO heading after the current subtree.",
    ),
    forge_docs::CommandDoc::line(
        "org-insert-todo-heading-respect-content",
        "Org: a new TODO heading after this entry's text, before its children.",
    ),
    forge_docs::CommandDoc::line(
        "org-list-indent-item-generic",
        "Org: indent the list item (and its children).",
    ),
    forge_docs::CommandDoc::line(
        "org-list-insert-item",
        "Org: a new list item after this one (same bullet, fresh checkbox).",
    ),
    forge_docs::CommandDoc::line(
        "org-list-item-to-subtree",
        "Org: turn the list item (and its children) into headings.",
    ),
    forge_docs::CommandDoc::line(
        "org-list-outdent-item",
        "Org: outdent the list item (and its children).",
    ),
    forge_docs::CommandDoc::line(
        "org-list-repair",
        "Org: renumber ordered lists and recount cookies.",
    ),
    forge_docs::CommandDoc::line(
        "org-mark-subtree",
        "Org: mark the whole subtree (point at start, mark at end).",
    ),
    forge_docs::CommandDoc::line(
        "org-match-sparse-tree",
        "Org: show only subtrees matching the regexp (sparse tree).",
    ),
    forge_docs::CommandDoc::line(
        "org-meta-return",
        "Org: a new list item after this one, or a new heading after this subtree.",
    ),
    forge_docs::CommandDoc::line(
        "org-metadown",
        "Org: move the subtree below its next sibling.",
    ),
    forge_docs::CommandDoc::line(
        "org-metaleft",
        "Org: promote the heading (or outdent the list item).",
    ),
    forge_docs::CommandDoc::line(
        "org-metaright",
        "Org: demote the heading (or indent the list item).",
    ),
    forge_docs::CommandDoc::line(
        "org-metaup",
        "Org: move the subtree above its previous sibling.",
    ),
    forge_docs::CommandDoc::line(
        "org-move-subtree",
        "Org: move the subtree past its next sibling (with an argument, the previous).",
    ),
    forge_docs::CommandDoc::line(
        "org-move-subtree-down",
        "Org: move the subtree below its next sibling.",
    ),
    forge_docs::CommandDoc::line(
        "org-move-subtree-up",
        "Org: move the subtree above its previous sibling.",
    ),
    forge_docs::CommandDoc::line(
        "org-next-visible-heading",
        "Org: go to the next visible heading.",
    ),
    forge_docs::CommandDoc::line(
        "org-num-mode",
        "Org: show the section number of each headline after its stars.",
    ),
    forge_docs::CommandDoc::line("org-occur", "Org: list matching lines in *Occur*."),
    forge_docs::CommandDoc::line(
        "org-occur-in-agenda-files",
        "Org: list matching lines (agenda files: this buffer).",
    ),
    forge_docs::CommandDoc::line(
        "org-paste-subtree",
        "Org: paste the kill-ring top as a subtree after the current one.",
    ),
    forge_docs::CommandDoc::line(
        "org-previous-visible-heading",
        "Org: go to the previous visible heading.",
    ),
    forge_docs::CommandDoc::line(
        "org-promote-subtree",
        "Org: promote the heading and its whole subtree.",
    ),
    forge_docs::CommandDoc::line("org-reveal", "Org: open the folds hiding point."),
    forge_docs::CommandDoc::line(
        "org-shiftmetaleft",
        "Org: promote the heading and its subtree.",
    ),
    forge_docs::CommandDoc::line(
        "org-shiftmetaright",
        "Org: demote the heading and its subtree.",
    ),
    forge_docs::CommandDoc::line(
        "org-shifttab",
        "Org: cycle the whole buffer: overview, contents, everything.",
    ),
    forge_docs::CommandDoc::line(
        "org-show-children",
        "Org: show the heading's direct children.",
    ),
    forge_docs::CommandDoc::line("org-show-entry", "Org: show the entry's own text."),
    forge_docs::CommandDoc::line("org-show-subtree", "Org: show the whole subtree."),
    forge_docs::CommandDoc::line(
        "org-sort",
        "Org: sort the child subtrees alphabetically (with an argument, reversed).",
    ),
    forge_docs::CommandDoc::line(
        "org-sort-list",
        "Org: sort sibling list items alphabetically (with an argument, reversed).",
    ),
    forge_docs::CommandDoc::line(
        "org-sparse-tree",
        "Org: fold away subtrees that don't match the regexp.",
    ),
    forge_docs::CommandDoc::new("org-todo", "Org: cycle the heading's TODO keyword (none, TODO, DONE,").body("or the #+TODO sequence). With an argument, backwards."),
    forge_docs::CommandDoc::line(
        "org-toggle-heading",
        "Org: turn a heading into a list item, or an item into a heading.",
    ),
    forge_docs::CommandDoc::line(
        "org-tree-to-indirect-buffer",
        "Org: open the subtree in a new (indirect-like) buffer.",
    ),
    forge_docs::CommandDoc::line(
        "org-up-element",
        "Org: go to the parent element (enclosing item or heading).",
    ),
    forge_docs::CommandDoc::line("org-yank", "Org: same as org-paste-subtree."),
    forge_docs::CommandDoc::new("other-frame", "Select the next frame (with an argument N,").body("N frames on; negative goes back) (C-x 5 o)."),
    forge_docs::CommandDoc::line(
        "other-window",
        "Select the next window (with an argument N, N windows on; negative goes back).",
    ),
    forge_docs::CommandDoc::line(
        "outline-level",
        "Show the level of the heading at point.",
    ),
    forge_docs::CommandDoc::line(
        "outline-next-heading",
        "Move to the start of the next outline heading.",
    ),
    forge_docs::CommandDoc::line(
        "outline-previous-heading",
        "Move to the start of the previous outline heading.",
    ),
    forge_docs::CommandDoc::line("outline-up-heading", "Org: go to the parent heading."),
    forge_docs::CommandDoc::line(
        "package-delete",
        "Delete a plugin installed from a package index.",
    ),
    forge_docs::CommandDoc::new("package-install", "Install a plugin from the package index (checked against its sha256),").body("then load it."),
    forge_docs::CommandDoc::line("package-list-packages", "Same as list-packages."),
    forge_docs::CommandDoc::line(
        "package-menu-delete",
        "In *Packages*: delete the package on this line.",
    ),
    forge_docs::CommandDoc::line(
        "package-menu-install",
        "In *Packages*: install or upgrade the package on this line.",
    ),
    forge_docs::CommandDoc::line(
        "package-menu-refresh",
        "In *Packages*: read the indexes again and redraw.",
    ),
    forge_docs::CommandDoc::line(
        "package-refresh-contents",
        "Read the package indexes again.",
    ),
    forge_docs::CommandDoc::line(
        "package-upgrade-all",
        "Install newer versions of every package installed from an index.",
    ),
    forge_docs::CommandDoc::new("package-vc-install", "Install a plugin from a Git repository (URL, or URL@tag/branch/commit): clone,").body("build (asking first) and load it."),
    forge_docs::CommandDoc::line(
        "package-vc-upgrade",
        "Fetch a package installed from version control and rebuild it if it changed.",
    ),
    forge_docs::CommandDoc::line(
        "package-vc-upgrade-all",
        "Fetch and rebuild every package installed from version control.",
    ),
    forge_docs::CommandDoc::line(
        "point-to-register",
        "Save point in a register, for C-x r j. It follows later edits.",
    ),
    forge_docs::CommandDoc::line(
        "previous-buffer",
        "Show the buffer before this one in the buffer list.",
    ),
    forge_docs::CommandDoc::line(
        "previous-error",
        "Visit the previous location in the last *compilation* or *grep* output.",
    ),
    forge_docs::CommandDoc::new("previous-line", "Move point up one line, keeping the column if it can.").body("With an argument, that many lines."),
    forge_docs::CommandDoc::line(
        "project-compile",
        "Like compile, in the current project's root.",
    ),
    forge_docs::CommandDoc::new("project-find-file", "Visit a file of the current project (git ls-files,").body("or a walk of the directory); TAB completes by prefix or any part of the path."),
    forge_docs::CommandDoc::line(
        "project-find-regexp",
        "Search the project for a regexp (ripgrep, git grep or grep), matches in *grep*.",
    ),
    forge_docs::CommandDoc::new("query-replace", "Replace text, asking about each match: y replaces, n skips, !").body("does the rest, q stops.\n\nWith C-u, only whole words match. With an active region, only the region.\nLowercase text matches any case, and the replacement follows the match's case."),
    forge_docs::CommandDoc::new("query-replace-regexp", "Like query-replace, but the pattern is a regular expression (Rust regex syntax).").body("In the replacement, \\& is the whole match and \\1 .. \\9 are groups."),
    forge_docs::CommandDoc::line(
        "quit-window",
        "Leave this buffer and go back to the previous one.",
    ),
    forge_docs::CommandDoc::new("quoted-insert", "Insert the next key as a character, even a control key (C-q C-l inserts a form").body("feed).\n\nThree octal digits give a character code: C-q 1 7 7. With an argument, insert that many."),
    forge_docs::CommandDoc::new("recenter-top-bottom", "Scroll so the current line is in the middle of the window.").body("Repeat to move it to the top, then the bottom. With an argument N, put it on row N (negative: from the bottom)."),
    forge_docs::CommandDoc::line(
        "recentf-open",
        "Open a recently opened file (TAB completes).",
    ),
    forge_docs::CommandDoc::line(
        "recompile",
        "Run the last compile (or, in *grep*, grep) command again.",
    ),
    forge_docs::CommandDoc::line(
        "recover-this-file",
        "Replace the buffer's text with its auto-save file (#name#), after a crash.",
    ),
    forge_docs::CommandDoc::line(
        "rectangle-number-lines",
        "Number the lines of the rectangle, starting at 1 (or the prefix argument).",
    ),
    forge_docs::CommandDoc::new("reload-init-file", "Reset key bindings, settings and init.rhai's commands to the defaults,").body("then run init.rhai again."),
    forge_docs::CommandDoc::new("reload-plugin", "Load a plugin again from its file (after rebuilding it).").body("Plugins also reload by themselves when their file changes, unless forge.opt.plugin_auto_reload is false."),
    forge_docs::CommandDoc::new("replace-regexp", "Replace every match of a regular expression after point (or in the region)").body("without asking."),
    forge_docs::CommandDoc::new("replace-string", "Replace every occurrence of a string after point (or in the region) without").body("asking."),
    forge_docs::CommandDoc::new("revert-buffer", "Reload the buffer from its file, discarding unsaved changes (undo brings them").body("back)."),
    forge_docs::CommandDoc::new("revoke-plugin-permissions", "Forget what was granted: a plugin's files, network and memory,").body("or a project's permission to run its .forge/init.rhai. It asks again next time."),
    forge_docs::CommandDoc::new("rgrep", "Search a directory tree for a pattern.").body("The pattern and the directory are asked for separately; the command is built from forge.opt.grep_find_template. The directories in forge.opt.grep_find_ignored_directories are skipped."),
    forge_docs::CommandDoc::new("save-buffer", "Save the buffer to its file, asking for a file name if it has none.").body("The first save makes a backup (file~), and asks first if the file changed on disk."),
    forge_docs::CommandDoc::line(
        "save-buffers-kill-terminal",
        "Exit Forge, asking first if file buffers have unsaved changes.",
    ),
    forge_docs::CommandDoc::line(
        "scratch-buffer",
        "Switch to the *scratch* buffer, creating it if needed.",
    ),
    forge_docs::CommandDoc::new("scroll-bar-mode", "Toggle a vertical scroll bar on the right of every window.").body("Clicking above or below the thumb scrolls by a screen; dragging the thumb scrolls continuously. With a prefix argument, turn it on."),
    forge_docs::CommandDoc::line(
        "scroll-down-command",
        "Scroll back one screen. With an argument, scroll that many lines.",
    ),
    forge_docs::CommandDoc::line(
        "scroll-other-window",
        "Scroll the next window forward one screen, without selecting it.",
    ),
    forge_docs::CommandDoc::line(
        "scroll-other-window-down",
        "Scroll the next window back one screen, without selecting it.",
    ),
    forge_docs::CommandDoc::line(
        "scroll-up-command",
        "Scroll forward one screen. With an argument, scroll that many lines.",
    ),
    forge_docs::CommandDoc::line(
        "select-and-paste",
        "Choose an older kill (TAB completes) and insert it, like C-y with a choice.",
    ),
    forge_docs::CommandDoc::line(
        "select-frame-by-name",
        "Ask for a frame's name (TAB completes) and make it the selected frame.",
    ),
    forge_docs::CommandDoc::line(
        "self-insert-command",
        "Insert the character that was typed (with an argument, that many times).",
    ),
    forge_docs::CommandDoc::new(
        "set-fill-column",
        "Ask for the column M-q fills to and set it.",
    )
    .body(
        "RET keeps point's current column. With a numeric argument, use that number instead of asking.",
    ),
    forge_docs::CommandDoc::line(
        "set-frame-name",
        "Ask for a name and give it to the selected frame.",
    ),
    forge_docs::CommandDoc::new("set-mark-command", "Set the mark at point and start a region. Repeat to deactivate it.").body("With C-u, jump to the mark and pop the next entry off the mark ring into the mark, so that each following C-u C-SPC goes further back. The positions you move through stay on the ring. With forge.opt.set_mark_command_repeat_pop on, a plain C-SPC right after pops again."),
    forge_docs::CommandDoc::new("shell-command", "Run a shell command in the buffer's directory:").body("one line of output goes to the echo area, more to *Shell Command Output*. With C-u, insert the output at point. Ending with & runs it asynchronously."),
    forge_docs::CommandDoc::line(
        "show-paren-mode",
        "Toggle highlighting the bracket at point and its match.",
    ),
    forge_docs::CommandDoc::line(
        "shrink-window",
        "Make the selected window shorter by one line (with an argument, that many).",
    ),
    forge_docs::CommandDoc::line(
        "shrink-window-horizontally",
        "Make the selected window narrower by one column (with an argument, that many).",
    ),
    forge_docs::CommandDoc::line(
        "split-window-below",
        "Divide the selected window in two, stacked vertically.",
    ),
    forge_docs::CommandDoc::line(
        "split-window-right",
        "Divide the selected window in two, left and right.",
    ),
    forge_docs::CommandDoc::line(
        "start-kbd-macro",
        "Start recording keys as a keyboard macro. C-x ) ends it, C-x e runs it.",
    ),
    forge_docs::CommandDoc::line(
        "steal-file-lock",
        "Take the lock on the current file after confirming another editor owns it.",
    ),
    forge_docs::CommandDoc::line(
        "string-rectangle",
        "Replace each line of the rectangle between mark and point with a string.",
    ),
    forge_docs::CommandDoc::line(
        "switch-to-buffer",
        "Ask for a buffer name and switch to it, creating it if needed.",
    ),
    forge_docs::CommandDoc::line(
        "switch-to-buffer-other-frame",
        "Switch to a buffer in another frame, making one if needed (C-x 5 b).",
    ),
    forge_docs::CommandDoc::line(
        "switch-to-buffer-other-tab",
        "Switch to a buffer in another tab (C-x t b).",
    ),
    forge_docs::CommandDoc::line(
        "switch-to-buffer-other-window",
        "Switch to a buffer in another window (splitting if there is only one).",
    ),
    forge_docs::CommandDoc::line(
        "tab-bar-close-other-tabs",
        "Close every tab of the current frame except the current one.",
    ),
    forge_docs::CommandDoc::line(
        "tab-bar-close-tab",
        "Close the current tab (C-x t 0), or the tab the prefix argument numbers.",
    ),
    forge_docs::CommandDoc::new("tab-bar-mode", "Toggle the tab bar: a row of tabs, each a saved window configuration,").body("at the top of each frame.\n\nThe tab bar holds several window layouts per frame; click a tab to switch, or use C-x t. With `tab-bar-show` a frame with a single tab keeps the row hidden."),
    forge_docs::CommandDoc::line(
        "tab-bar-move-tab",
        "Move the current tab that many places (C-x t m; negative moves left).",
    ),
    forge_docs::CommandDoc::line(
        "tab-bar-new-tab",
        "Make a new tab showing a copy of the current window configuration (C-x t 2).",
    ),
    forge_docs::CommandDoc::line("tab-bar-next-tab", "Switch to the next tab (C-x t o)."),
    forge_docs::CommandDoc::line("tab-bar-previous-tab", "Switch to the previous tab."),
    forge_docs::CommandDoc::line(
        "tab-bar-rename-tab",
        "Rename the current tab (C-x t r); an empty name restores the default.",
    ),
    forge_docs::CommandDoc::line(
        "tab-bar-select-tab",
        "Switch to the tab the prefix argument numbers: 1 the first, 9 the last.",
    ),
    forge_docs::CommandDoc::line(
        "tab-bar-undo-close-tab",
        "Bring back the last closed tab (M-x tab-undo).",
    ),
    forge_docs::CommandDoc::line(
        "tab-close",
        "Close the current tab (C-x t 0), or the tab the prefix argument numbers.",
    ),
    forge_docs::CommandDoc::line(
        "tab-close-other",
        "Close every tab of the current frame except the current one.",
    ),
    forge_docs::CommandDoc::line(
        "tab-list-tabs",
        "Show the tabs of the current frame in the echo area.",
    ),
    forge_docs::CommandDoc::line(
        "tab-move",
        "Move the current tab that many places (C-x t m; negative moves left).",
    ),
    forge_docs::CommandDoc::line(
        "tab-new",
        "Make a new tab showing a copy of the current window configuration (C-x t 2).",
    ),
    forge_docs::CommandDoc::line("tab-next", "Switch to the next tab (C-x t o)."),
    forge_docs::CommandDoc::line("tab-previous", "Switch to the previous tab."),
    forge_docs::CommandDoc::line(
        "tab-rename",
        "Rename the current tab (C-x t r); an empty name restores the default.",
    ),
    forge_docs::CommandDoc::line(
        "tab-select",
        "Switch to the tab the prefix argument numbers: 1 the first, 9 the last.",
    ),
    forge_docs::CommandDoc::line(
        "tab-switch",
        "Switch to a tab by name, with completion (C-x t RET).",
    ),
    forge_docs::CommandDoc::line("tab-undo", "Bring back the last closed tab (M-x tab-undo)."),
    forge_docs::CommandDoc::line(
        "text-scale-decrease",
        "Make this buffer's text smaller by ARG steps (1.2 times each). GUI only.",
    ),
    forge_docs::CommandDoc::line(
        "text-scale-increase",
        "Make this buffer's text larger by ARG steps (1.2 times each). GUI only.",
    ),
    forge_docs::CommandDoc::line("text-scale-reset", "Show this buffer's text at its normal size."),
    forge_docs::CommandDoc::line(
        "thing-at-point",
        "Ask for a thing and show the text of the one at point.",
    ),
    forge_docs::CommandDoc::line(
        "toggle-frame-tab-bar",
        "Show or hide the tab bar on this frame only, whatever `tab-bar-mode` says.",
    ),
    forge_docs::CommandDoc::line(
        "toggle-truncate-lines",
        "Switch between cutting off long lines and wrapping them.",
    ),
    forge_docs::CommandDoc::new("tool-bar-mode", "Toggle the tool bar in graphical frames: a row of buttons for common commands.").body("A major mode's own tool bar replaces the default one unless `tool-bar-always-show-default` is set.\n\nThe terminal has no tool bar, as with Forge -nw. With a prefix argument, turn it on."),
    forge_docs::CommandDoc::new("transpose-chars", "Swap the characters before and after point (with an argument,").body("move the character that far)."),
    forge_docs::CommandDoc::new("transpose-lines", "Swap this line with the one above and move down (with an argument N,").body("move that line N lines down).\n\nWith 0, swap the lines of point and mark."),
    forge_docs::CommandDoc::line(
        "transpose-sexps",
        "Swap the sexp before point with the sexp after it and move past both (C-M-t).",
    ),
    forge_docs::CommandDoc::new("transpose-words", "Exchange the word at point with the next one, or with the previous one").body("when there is no next, leaving point after both (M-t)."),
    forge_docs::CommandDoc::new(
        "treesit-install-language-grammar",
        "Install a tree-sitter grammar from a built-in recipe and load it now",
    ).body("Forge clones and builds it with Git and cc/c++; progress goes to *Grammar Build*. It is recorded and loaded again at startup."),
    forge_docs::CommandDoc::line(
        "tutorial-exit",
        "Leave the tutorial buffer (q in the tutorial).",
    ),
    forge_docs::CommandDoc::line(
        "tutorial-next-exercise",
        "Move to the next exercise in the tutorial (n).",
    ),
    forge_docs::CommandDoc::line(
        "tutorial-prev",
        "Move to the previous exercise in the tutorial (p).",
    ),
    forge_docs::CommandDoc::new("undefined", "Do nothing and report that the key is undefined.").body("Bind a key to this to switch it off, even if a lower-priority keymap binds it."),
    forge_docs::CommandDoc::new("undelete-frame", "Bring back a frame deleted with delete-frame (C-x 5 u).").body("With no argument, the most recently deleted; with a numeric argument from 1 to 16, the one that far back. The frame keeps its name, windows and point."),
    forge_docs::CommandDoc::line(
        "undo",
        "Undo the last change (with an argument, that many). Repeat to undo more.",
    ),
    forge_docs::CommandDoc::line(
        "undo-redo",
        "Redo the last change that was undone (with an argument, that many).",
    ),
    forge_docs::CommandDoc::new("unfill-paragraph", "Join the lines of the current paragraph (the inverse of M-q).").body("With an argument N, do N paragraphs. With an active region, act on it like M-q."),
    forge_docs::CommandDoc::new("universal-argument", "Start a prefix argument: C-u alone is 4 (C-u C-u is 16); C-u 12 is 12;").body("C-u - is -1."),
    forge_docs::CommandDoc::line(
        "unload-plugin",
        "Unload a plugin, removing the commands, modes and key bindings it added.",
    ),
    forge_docs::CommandDoc::line(
        "up-list",
        "Move forward out of ARG enclosing lists (C-M-u).",
    ),
    forge_docs::CommandDoc::new("upcase-word", "Convert the word after point to upper case and move past it.").body("With an argument N, do N words; a negative N does the words before point without moving."),
    forge_docs::CommandDoc::line(
        "view-echo-area-messages",
        "Show the *Messages* buffer: everything that has appeared in the echo area.",
    ),
    forge_docs::CommandDoc::line("view-lossage", "Show the last keys you typed (C-h l)."),
    forge_docs::CommandDoc::line(
        "view-news",
        "Show the newest release notes from NEWS.org (C-h n).",
    ),
    forge_docs::CommandDoc::new("visual-line-mode", "Toggle wrapping long lines at word boundaries in this buffer;").body("C-n and C-p then move by screen line."),
    forge_docs::CommandDoc::line("wdired-abort-changes", "In wdired: forget the edits and return to Dired."),
    forge_docs::CommandDoc::line("wdired-change-to-wdired-mode", "In Dired: make the names editable; C-c C-c renames, C-c C-k aborts."),
    forge_docs::CommandDoc::line("wdired-finish-edit", "In wdired: rename the files whose names were edited."),
    forge_docs::CommandDoc::line("what-page", "Report the page number point is on and the line that page starts at."),
    forge_docs::CommandDoc::line(
        "where-is",
        "Ask for a command name and show the keys that run it.",
    ),
    forge_docs::CommandDoc::line(
        "which-function",
        "Say which defun point is in, from the grammar's tags query.",
    ),
    forge_docs::CommandDoc::line(
        "whitespace-mode",
        "Toggle showing spaces and tabs in this buffer.",
    ),
    forge_docs::CommandDoc::line(
        "write-file",
        "Write the buffer to a new file name and visit that file.",
    ),
    forge_docs::CommandDoc::new("xterm-mouse-mode", "Toggle mouse support: the wheel scrolls the window under the pointer,").body("a click moves point and\n\nis then dispatched as a key (<mouse-1>, <mouse-2>), and a drag selects. While on, most terminals\nstill select text with Shift+drag. With an argument, on if positive."),
    forge_docs::CommandDoc::new("yank", "Insert the most recent kill.").body("C-u C-y leaves point before it; C-y with a number N inserts the Nth most recent."),
    forge_docs::CommandDoc::new("yank-pop", "Right after a yank, replace the yanked text with the previous kill (with an").body("argument, that many back)."),
    forge_docs::CommandDoc::line(
        "yank-rectangle",
        "Insert the last killed rectangle with its top-left corner at point.",
    ),
    forge_docs::CommandDoc::new("zrgrep", "Like rgrep, but the files are gzipped.").body("The command is built from forge.opt.grep_find_template, with zgrep in place of grep."),
];

/// The commands worth offering in the selected buffer: those bound in the
/// active keymaps, and built-ins no keymap binds (their context is unknown).
/// A command that only another mode's keymap binds stays out of completion
/// until that mode is active, which keeps M-x to what is useful here.
pub fn visible_names(app: &App) -> Vec<String> {
    let bound_anywhere: std::collections::HashSet<&str> =
        app.editor.keymaps.bound_commands().collect();
    let active = app.editor.active_keymaps_now();
    let bound_here: std::collections::HashSet<&str> = active.commands().collect();
    app.editor
        .commands
        .names()
        .filter(|name| {
            crate::webkit::command_visible(app, name)
                .unwrap_or_else(|| !bound_anywhere.contains(name) || bound_here.contains(name))
        })
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
pub fn is_command(name: &str) -> bool {
    COMMANDS.binary_search_by(|cd| cd.name().cmp(name)).is_ok()
        // Commands of their own modules that have a key in the main map.
        || crate::server::COMMANDS.iter().any(|cd| cd.name() == name)
}

#[cfg(test)]
pub fn command_names() -> impl Iterator<Item = &'static str> {
    COMMANDS.iter().map(|cd| cd.name())
}

const TAB_STOP: usize = 4;

/// How many marks the global mark ring keeps.
const GLOBAL_MARK_RING_LIMIT: usize = 128;

/// Set the mark at point and start a region: the mark goes on this
/// buffer's mark ring and on the global one.
fn set_mark_here(app: &mut App) {
    let id = app.active_id();
    let b = app.buf();
    let p = b.point();
    b.push_mark(p, true);
    let offset = b.point_offset();
    app.editor.global_mark_ring.retain(|m| *m != (id, offset));
    app.editor.global_mark_ring.insert(0, (id, offset));
    app.editor.global_mark_ring.truncate(GLOBAL_MARK_RING_LIMIT);
    app.msg("Mark set");
}

/// Run a motion `n` times (negative: the other way), stopping with an
/// error at the edge of the buffer.
/// After RET: indent the new line (if the mode has rules), and leave the
/// line above without trailing blanks if it was only whitespace.
fn electric_newline(app: &mut App) {
    let line = app.buf_ref().point().line;
    app.editor.sync_language(
        app.editor
            .state
            .active_buffer
            .expect("there is always an active buffer"),
    );
    if app.buf_ref().indent_style().is_none() {
        return;
    }
    let above = app.buf_ref().line(line - 1);
    if !above.is_empty() && above.trim().is_empty() {
        let s = app.buf_ref().pos_to_offset(Pos::new(line - 1, 0));
        app.buf().replace_offsets(s, s + above.chars().count(), "");
        app.buf().set_point(Pos::new(line, 0));
    }
    app.editor.indent_line(line);
}

/// After typing a character: a closing bracket first on its line goes
/// to its opener's level; in Python, `else:` and friends dedent.
fn electric_char(app: &mut App, c: char) {
    let p = app.buf_ref().point();
    let text = app.buf_ref().line(p.line);
    let before: String = text.chars().take(p.col.saturating_sub(1)).collect();
    let reindent = match c {
        '}' | ')' | ']' => before.trim().is_empty(),
        ':' => {
            let first = before.split_whitespace().next().unwrap_or("");
            ["else", "elif", "except", "finally"].contains(&first.trim_end_matches(':'))
                && app.buf_ref().language().is_some_and(|l| l.name == "python")
        }
        _ => false,
    };
    if reindent {
        app.editor.indent_line(p.line);
    }
}

/// Auto Fill mode, after a command: break the line when the character just
/// typed pushed it past the fill column. Only a self-inserting character
/// (or a run of them) counts; `C-q` and `C-o` do not (they go through
/// their own commands) and RET does not break the line. `fill-column` is
/// the buffer's, so C-u does not change it here.
pub fn auto_fill(app: &mut App) {
    if !app.buf_ref().minor_mode_enabled("auto-fill-mode") {
        return;
    }
    if app.last_command != "self-insert-command" {
        return;
    }
    // The commit buffer has its own rule about the summary line.
    if app.buf_ref().major_mode() == crate::vc::log_edit::MODE {
        return;
    }
    let key = app.this_command_keys.last().cloned().unwrap_or_default();
    let space = key == "SPC" || key == " ";
    forge_core::auto_fill::maybe_break(app.buf(), space);
}

/// C-n / C-p move by screen rows (visual-line-mode).
fn visual_lines(app: &App) -> bool {
    app.buf_ref().options.wrap.unwrap_or(app.wrap) == forge_core::decoration::Wrap::Word
}

/// The pairs electric-pair-mode uses in the current buffer.
fn pairs(app: &App) -> Vec<(char, char)> {
    let s = app
        .buf_ref()
        .language()
        .map_or(forge_core::syntax::DEFAULT_PAIRS, |l| l.pairs);
    let c: Vec<char> = s.chars().collect();
    c.chunks(2).map(|p| (p[0], p[1])).collect()
}

/// electric-pair-mode: typing an opener inserts its closer too; typing a
/// closer that's already next to point just moves over it. True if it
/// handled the character.
fn electric_pair(app: &mut App, c: char) -> bool {
    let pairs = pairs(app);
    let b = app.buf_ref();
    let p = b.point();
    let next = b.char_at(p);
    let prev = b.prev_pos(p).and_then(|q| b.char_at(q));
    if pairs.iter().any(|&(_, close)| close == c) && next == Some(c) {
        // Skip over a closer (or closing quote) that's already there.
        app.buf().move_right();
        return true;
    }
    let Some(&(open, close)) = pairs.iter().find(|&&(open, _)| open == c) else {
        return false;
    };
    let next_ok = next.is_none_or(|n| n.is_whitespace() || pairs.iter().any(|&(_, cl)| cl == n));
    // Quotes: not right after a word character (don't, it's).
    let quote_ok = open != close || !prev.is_some_and(|p| p.is_alphanumeric() || p == '_');
    if !next_ok || !quote_ok {
        return false;
    }
    if app.buf_ref().active_region().is_some() {
        let (a, z) = app.buf_ref().active_region().expect("checked");
        let inner = app.buf_ref().text_range(a, z);
        app.buf()
            .replace_range(a, z, &format!("{open}{inner}{close}"));
        return true;
    }
    app.buf().insert_str(&format!("{open}{close}"));
    app.buf().move_left();
    true
}

/// Where Electric Quote mode may convert a character at `at`: the option
/// for the context point is in. A text paragraph is any context that is not
/// a comment or string, so a buffer without a grammar is always a
/// paragraph (and Electric Quote's default settings convert there).
fn electric_quote_active(app: &mut App, at: usize) -> bool {
    let face = if at == 0 {
        None
    } else {
        app.buf()
            .syntax_faces(at - 1..at)
            .into_iter()
            .next()
            .flatten()
    };
    match face {
        Some(forge_core::syntax::SyntaxFace::Comment) => app.electric_quote_comment,
        Some(forge_core::syntax::SyntaxFace::String | forge_core::syntax::SyntaxFace::Escape) => {
            app.electric_quote_string
        }
        _ => app.electric_quote_paragraph,
    }
}

/// Electric Quote mode: type ` and ' as curved quotes, and with
/// electric-quote-replace-double, `"` as a curved double quote. `` `` ``
/// becomes a left double quote and `''` a right double quote. True if the
/// character was handled, so self-insert-command should not insert it.
fn electric_quote(app: &mut App, c: char) -> bool {
    if c == '"' {
        if !app.electric_quote_replace_double {
            return false;
        }
    } else if c != '`' && c != '\'' {
        return false;
    }
    let at = app.buf_ref().point_offset();
    if !electric_quote_active(app, at) {
        return false;
    }
    let [left_single, right_single, left_double, right_double] = app.electric_quote_chars;
    let prev = (at > 0)
        .then(|| app.buf_ref().offset_to_pos(at - 1))
        .and_then(|p| app.buf_ref().char_at(p));
    match c {
        // A second ` or ' turns the curved single quote just typed into
        // the matching curved double quote.
        '`' => {
            if prev == Some(left_single) {
                app.buf()
                    .replace_offsets(at - 1, at, &left_double.to_string());
            } else {
                app.buf().insert_char(left_single);
            }
            true
        }
        '\'' => {
            if prev == Some(right_single) {
                app.buf()
                    .replace_offsets(at - 1, at, &right_double.to_string());
            } else {
                app.buf().insert_char(right_single);
            }
            true
        }
        '"' => {
            let open = at == 0
                || prev.is_none_or(|c| c.is_whitespace() || "([{\"\u{2018}\u{201c}".contains(c));
            app.buf()
                .insert_char(if open { left_double } else { right_double });
            true
        }
        _ => false,
    }
}

/// DEL right between an empty pair deletes both.
fn delete_empty_pair(app: &mut App) -> bool {
    let pairs = pairs(app);
    let b = app.buf_ref();
    let p = b.point();
    let (Some(prev), Some(next)) = (b.prev_pos(p).and_then(|q| b.char_at(q)), b.char_at(p)) else {
        return false;
    };
    if !pairs.contains(&(prev, next)) {
        return false;
    }
    let o = b.point_offset();
    app.buf().replace_offsets(o - 1, o + 1, "");
    true
}

fn repeat_motion(
    app: &mut App,
    n: i64,
    fwd: fn(&mut Buffer) -> bool,
    back: fn(&mut Buffer) -> bool,
) {
    let (step, msg): (fn(&mut Buffer) -> bool, _) = if n >= 0 {
        (fwd, "End of buffer")
    } else {
        (back, "Beginning of buffer")
    };
    for _ in 0..n.unsigned_abs() {
        if !step(app.buf()) {
            app.error(msg);
            break;
        }
    }
}

/// Repeat a motion up to `n` times, stopping as soon as it stops making
/// progress: a motion that cannot go on (at the end of the buffer, say)
/// would otherwise spin for the whole count.  The step says whether it
/// moved.
fn repeat_until_stuck(n: i64, mut step: impl FnMut() -> bool) {
    for _ in 0..n.unsigned_abs() {
        if !step() {
            break;
        }
    }
}

/// Run a command. Returns false if the name is unknown.
pub fn execute(app: &mut App, cmd: &str) -> bool {
    let n = app.count();
    match cmd {
        // ---------------------------------------------- prefix argument
        "universal-argument" => app.universal_argument(),
        "digit-argument" => {
            let d = app
                .this_command_keys
                .last()
                .and_then(|k| k.chars().last())
                .and_then(|c| c.to_digit(10));
            app.digit_argument(d.unwrap_or(0) as i64);
        }
        "negative-argument" => app.negative_argument(),

        // ------------------------------------------------------ motion
        "forward-char" => repeat_motion(app, n, Buffer::move_right, Buffer::move_left),
        "backward-char" => repeat_motion(app, -n, Buffer::move_right, Buffer::move_left),
        "next-line" | "previous-line" => {
            let n = if cmd == "next-line" { n } else { -n };
            if visual_lines(app) {
                // visual-line-mode: by screen rows.
                let (off, width) = (app.buf_ref().point_offset(), app.win_mut().width);
                let to = forge_core::display::move_visual_rows(
                    app.buf_ref(),
                    off,
                    n as isize,
                    width,
                    true,
                );
                if to == off {
                    app.error(if n > 0 {
                        "End of buffer"
                    } else {
                        "Beginning of buffer"
                    });
                }
                app.buf().set_offset(to);
            } else {
                repeat_motion(app, n, Buffer::move_down, Buffer::move_up);
            }
        }
        "move-beginning-of-line" | "move-end-of-line" => {
            if n != 1 {
                app.buf().move_lines((n - 1) as isize);
            }
            if cmd == "move-beginning-of-line" {
                app.buf().move_line_start();
            } else {
                app.buf().move_line_end();
            }
        }
        "forward-word" | "backward-word" => {
            let n = if cmd == "forward-word" { n } else { -n };
            repeat_until_stuck(n, || {
                if n > 0 {
                    app.buf().forward_word()
                } else {
                    app.buf().backward_word()
                }
            });
        }
        "mark-word" => {
            // M-@: put the mark at the end of the next N words, point
            // stays put.
            let start = app.buf_ref().point();
            repeat_until_stuck(n, || {
                if n > 0 {
                    app.buf().forward_word()
                } else {
                    app.buf().backward_word()
                }
            });
            let end = app.buf_ref().point();
            app.buf().push_mark(end, true);
            app.buf().set_point(start);
            app.msg("Mark set");
        }
        "transpose-words" => {
            let mut chars: Vec<char> = app.buf_ref().get_contents().chars().collect();
            let from = app.buf_ref().point_offset();
            let to = text_nav::transpose_words(&mut chars, from);
            if to != from {
                let text: String = chars.into_iter().collect();
                let b = app.buf();
                let end = b.char_count();
                b.replace_offsets(0, end, &text);
                b.set_offset(to);
            }
        }
        "forward-sentence" | "backward-sentence" => {
            let re = match app.sentence_end_re() {
                Ok(re) => re,
                Err(e) => {
                    app.error(e);
                    return true;
                }
            };
            let forward = cmd == "forward-sentence";
            repeat_until_stuck(n, || {
                let chars: Vec<char> = app.buf_ref().get_contents().chars().collect();
                let from = app.buf_ref().point_offset();
                let to = if forward {
                    text_nav::forward_sentence(&chars, from, app.sentence_end, re.as_ref())
                } else {
                    text_nav::backward_sentence(&chars, from, app.sentence_end, re.as_ref())
                };
                app.buf().set_offset(to);
                to != from
            });
        }
        "forward-paragraph" | "backward-paragraph" => {
            let (start_re, separate_re) = match app.paragraph_rules() {
                Ok(r) => r,
                Err(e) => {
                    app.error(e);
                    return true;
                }
            };
            let rules = text_nav::ParagraphRules {
                start: &start_re,
                separate: &separate_re,
            };
            let forward = cmd == "forward-paragraph";
            repeat_until_stuck(n, || {
                let lines: Vec<String> = (0..app.buf_ref().line_count())
                    .map(|l| app.buf_ref().line(l))
                    .collect();
                let line = app.buf_ref().point().line;
                let target = if forward {
                    text_nav::forward_paragraph(&lines, line, rules)
                } else {
                    text_nav::backward_paragraph(&lines, line, rules)
                };
                app.buf().set_point(Pos::new(target, 0));
                target != line
            });
        }
        "mark-paragraph" => {
            // with no argument, mark the whole paragraph; with an
            // argument N, mark N paragraphs starting here.
            let (start_re, separate_re) = match app.paragraph_rules() {
                Ok(r) => r,
                Err(e) => {
                    app.error(e);
                    return true;
                }
            };
            let rules = text_nav::ParagraphRules {
                start: &start_re,
                separate: &separate_re,
            };
            let lines: Vec<String> = (0..app.buf_ref().line_count())
                .map(|l| app.buf_ref().line(l))
                .collect();
            let line = app.buf_ref().point().line;
            let start = text_nav::paragraph_bounds(&lines, line, rules)
                .map(|(s, _)| s)
                .unwrap_or(line);
            let count = if app.has_arg() && !app.raw_arg() {
                n.unsigned_abs().max(1) as usize
            } else {
                1
            };
            let mut end = start;
            for _ in 0..count {
                end = text_nav::forward_paragraph(&lines, end, rules);
                if end + 1 >= lines.len() {
                    end = lines.len().saturating_sub(1);
                    break;
                }
            }
            app.buf().push_mark(Pos::new(end, 0), true);
            app.buf().set_point(Pos::new(start, 0));
            app.msg("Mark set");
        }
        "forward-page" | "backward-page" => {
            let re = match app.page_delimiter_re() {
                Ok(re) => re,
                Err(e) => {
                    app.error(e);
                    return true;
                }
            };
            let forward = cmd == "forward-page";
            repeat_until_stuck(n, || {
                let chars: Vec<char> = app.buf_ref().get_contents().chars().collect();
                let from = app.buf_ref().point_offset();
                let to = if forward {
                    text_nav::forward_page(&chars, from, &re)
                } else {
                    text_nav::backward_page(&chars, from, &re)
                };
                app.buf().set_offset(to);
                to != from
            });
        }
        "mark-page" => {
            let re = match app.page_delimiter_re() {
                Ok(re) => re,
                Err(e) => {
                    app.error(e);
                    return true;
                }
            };
            let chars: Vec<char> = app.buf_ref().get_contents().chars().collect();
            let from = app.buf_ref().point_offset();
            let (_, start) = text_nav::page_number(&chars, from, &re);
            let end = text_nav::forward_page(&chars, start, &re);
            let mark = app.buf_ref().offset_to_pos(end);
            app.buf().push_mark(mark, false);
            app.buf().set_offset(start);
            app.msg("Mark set");
        }
        "what-page" => {
            let re = match app.page_delimiter_re() {
                Ok(re) => re,
                Err(e) => {
                    app.error(e);
                    return true;
                }
            };
            let chars: Vec<char> = app.buf_ref().get_contents().chars().collect();
            let from = app.buf_ref().point_offset();
            let (page, start) = text_nav::page_number(&chars, from, &re);
            let line = text_nav::offset_line(&chars, start) + 1;
            let point_line = text_nav::offset_line(&chars, from) + 1;
            app.msg(format!(
                "Page {page}, line {point_line} (page starts at line {line})"
            ));
        }
        "count-lines-page" => {
            let re = match app.page_delimiter_re() {
                Ok(re) => re,
                Err(e) => {
                    app.error(e);
                    return true;
                }
            };
            let chars: Vec<char> = app.buf_ref().get_contents().chars().collect();
            let from = app.buf_ref().point_offset();
            let (_, start) = text_nav::page_number(&chars, from, &re);
            let before = text_nav::page_line_count(&chars, start, &re);
            let count = text_nav::page_line_count(&chars, from, &re);
            let buf_lines = app.buf_ref().line_count();
            let after = buf_lines.saturating_sub(before + count);
            app.msg(format!(
                "Page has {count} lines ({before} + {count} + {after})"
            ));
        }
        "beginning-of-buffer" | "end-of-buffer" => {
            let b = app.buf();
            let p = b.point();
            b.push_mark(p, false);
            if cmd == "beginning-of-buffer" {
                b.beginning_of_buffer();
            } else {
                b.end_of_buffer();
            }
            app.msg("Mark set");
        }
        "scroll-up-command" | "scroll-down-command" => {
            let up = cmd == "scroll-up-command";
            if app.has_arg() && !app.raw_arg() {
                scroll_lines(app, if up { n } else { -n });
            } else {
                scroll_screen(app, up);
            }
        }
        "recenter-top-bottom" => {
            let line = app.buf_ref().point().line;
            let explicit = app.has_arg().then(|| (app.raw_arg(), n));
            if app.last_command != "recenter-top-bottom" || explicit.is_some() {
                app.win_mut().recenter_step = 0;
            }
            let w = app.win_mut();
            let h = w.height.max(1);
            w.scroll_y_px = 0;
            w.scroll_top = match explicit {
                Some((true, _)) => line.saturating_sub(h / 2),
                Some((false, row)) => {
                    let row = if row >= 0 {
                        row as usize
                    } else {
                        h.saturating_sub(row.unsigned_abs() as usize)
                    };
                    line.saturating_sub(row.min(h - 1))
                }
                None => {
                    // Repeated C-l cycles: center, top, bottom.
                    let top = match w.recenter_step {
                        0 => line.saturating_sub(h / 2),
                        1 => line,
                        _ => (line + 1).saturating_sub(h),
                    };
                    w.recenter_step = (w.recenter_step + 1) % 3;
                    top
                }
            };
        }
        "goto-line" => {
            if app.has_arg() && !app.raw_arg() {
                let b = app.buf();
                let p = b.point();
                b.push_mark(p, false);
                b.goto_line(n.max(1) as usize);
            } else {
                app.prompt("Goto line: ", Purpose::GotoLine);
            }
        }

        // ----------------------------------------------------- editing
        "self-insert-command" => {
            if app.buf_ref().major_mode() == crate::dired::MODE {
                app.error("Buffer is read-only (Dired)");
                return true;
            }
            let key = app.this_command_keys.last().cloned().unwrap_or_default();
            let mut chars = key.chars();
            let c = match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ if key == "SPC" => Some(' '),
                // Shift is written on the letter (Shift+a is "S-a", the way
                // Forge writes it) and typed as the letter it makes.
                _ => key
                    .strip_prefix("S-")
                    .filter(|c| c.chars().count() == 1)
                    .and_then(|c| c.chars().next())
                    .and_then(|c| c.to_uppercase().next()),
            };
            match c {
                None => app.error(format!(
                    "{} can't insert a character",
                    describe_key_sequence(&app.this_command_keys)
                )),
                Some(_) if n < 0 => app.error("Negative repetition argument"),
                Some(c) if n == 1 => {
                    let quote_on =
                        app.electric_quote || app.buf_ref().options.electric_quote == Some(true);
                    if !(quote_on && electric_quote(app, c))
                        && !(app.electric_pair && electric_pair(app, c))
                    {
                        app.buf().insert_char(c);
                    }
                    if app.electric_indent {
                        electric_char(app, c);
                    }
                }
                Some(c) => app.buf().insert_str(&c.to_string().repeat(n as usize)),
            }
        }
        "quoted-insert" => app.start_quoted_insert(),
        "newline" => {
            for _ in 0..n.max(0) {
                app.buf().insert_str("\n");
                if app.electric_indent {
                    electric_newline(app);
                }
            }
        }
        "newline-without-indent" => app.buf().insert_str(&"\n".repeat(n.max(0) as usize)),
        "back-to-indentation" => {
            let line = app.buf_ref().point().line;
            let ws = app
                .buf_ref()
                .line(line)
                .chars()
                .take_while(|c| c.is_whitespace())
                .count();
            app.buf().set_point(Pos::new(line, ws));
        }
        "indent-region" => match app.buf_ref().region() {
            Some((a, b)) => {
                let last = if b.col == 0 && b.line > a.line {
                    b.line - 1
                } else {
                    b.line
                };
                let p = app.buf_ref().point_offset();
                let mut any = false;
                for line in a.line..=last {
                    if !app.buf_ref().line(line).trim().is_empty() {
                        any |= app.editor.indent_line(line);
                    } else if app.buf_ref().line_len(line) > 0 {
                        // Blank lines lose their whitespace.
                        let s = app.buf_ref().pos_to_offset(Pos::new(line, 0));
                        let e = s + app.buf_ref().line_len(line);
                        app.buf().replace_offsets(s, e, "");
                    }
                }
                let len = app.buf_ref().char_count();
                app.buf().set_offset(p.min(len));
                app.buf().deactivate_mark();
                if any {
                    app.msg("Indenting region...done");
                } else {
                    app.error(format!(
                        "{} has no indentation rules",
                        app.buf_ref().major_mode()
                    ));
                }
            }
            None => app.error("The mark is not set now, so there is no region"),
        },
        "open-line" => {
            let p = app.buf_ref().point();
            app.buf().insert_str(&"\n".repeat(n.max(0) as usize));
            app.buf().set_point(p);
        }
        "indent-for-tab-command" => {
            let line = app.buf_ref().point().line;
            if app.buf_ref().active_region().is_some() {
                return execute(app, "indent-region");
            }
            if !app.editor.indent_line(line) {
                // No rules for this mode: to the next tab stop.
                let col = app.buf_ref().point().col;
                let spaces = TAB_STOP - col % TAB_STOP;
                app.buf().insert_str(&" ".repeat(spaces));
            }
        }
        "delete-char" | "delete-backward-char" => {
            let forward = cmd == "delete-char";
            let n = if forward { n } else { -n };
            if !app.has_arg() {
                // An active region that isn't empty goes (use-region-p).
                if let Some((a, b)) = app.buf_ref().active_region().filter(|(a, b)| a != b) {
                    app.buf().delete_range(a, b);
                } else if !forward && app.electric_pair && delete_empty_pair(app) {
                    // electric-pair-mode: DEL between () removes both.
                } else if forward && !app.buf().delete_char() {
                    app.error("End of buffer");
                } else if !forward && !app.buf().backspace() {
                    app.error("Beginning of buffer");
                }
            } else {
                // With an argument the characters are killed.
                let start = app.buf_ref().point();
                repeat_motion(app, n, Buffer::move_right, Buffer::move_left);
                let end = app.buf_ref().point();
                let text = app.buf().delete_range(start, end);
                app.kill(text, n < 0);
            }
        }
        "transpose-chars" => {
            for _ in 0..n.max(1) {
                if !app.buf().transpose_chars() {
                    app.error("Beginning of line");
                    break;
                }
            }
        }
        "transpose-lines" => transpose_lines(app, n),
        "delete-indentation" | "join-line" => {
            let b = app.buf_ref();
            if let (false, Some((a, z))) = (app.has_arg(), b.active_region()) {
                for l in (a.line + 1..=z.line).rev() {
                    fill::join_line(app.buf(), l);
                }
            } else {
                let line = b.point().line + usize::from(app.has_arg());
                if !fill::join_line(app.buf(), line) && line == 0 {
                    app.error("Beginning of buffer");
                }
            }
        }
        "fill-paragraph" => {
            let justify = app.has_arg() && !app.raw_arg();
            if let Some((a, b)) = app.buf_ref().active_region() {
                fill_region(app, a.line, b.line, justify);
            } else {
                let line = app.buf_ref().point().line;
                if let Some((s, e)) = fill::paragraph_at(app.buf_ref(), line) {
                    fill::fill_line_range_arg(app.buf(), s, e, justify);
                }
            }
        }
        "fill-region" => {
            let justify = app.has_arg() && !app.raw_arg();
            let Some((a, b)) = app.buf_ref().region() else {
                app.error("The mark is not set now, or not a region");
                return true;
            };
            fill_region(app, a.line, b.line, justify);
        }
        "fill-region-as-paragraph" => {
            let justify = app.has_arg() && !app.raw_arg();
            let Some((a, b)) = app.buf_ref().region() else {
                app.error("The mark is not set now, or not a region");
                return true;
            };
            // The whole region becomes one paragraph: blank lines inside
            // it are removed first, then it is filled.
            let (start, end) = (a.line, b.line);
            let text: Vec<String> = (start..=end)
                .map(|l| app.buf_ref().line(l))
                .filter(|l| !l.trim().is_empty())
                .collect();
            if text.is_empty() {
                return true;
            }
            let s = app.buf_ref().pos_to_offset(Pos::new(start, 0));
            let e = app
                .buf_ref()
                .pos_to_offset(Pos::new(end, app.buf_ref().line_len(end)));
            app.buf().replace_offsets(s, e, &text.join("\n"));
            fill::fill_line_range_arg(app.buf(), start, start + text.len() - 1, justify);
        }
        "unfill-paragraph" => {
            if let Some((a, b)) = app.buf_ref().active_region() {
                // Join every paragraph in the region, bottom up.
                for (s, e) in paragraphs_in(app.buf_ref(), a.line, b.line)
                    .into_iter()
                    .rev()
                {
                    fill::unfill_line_range(app.buf(), s, e);
                }
            } else {
                let count = n.unsigned_abs().max(1) as usize;
                let mut line = app.buf_ref().point().line;
                for _ in 0..count {
                    if !fill::unfill_paragraph(app.buf(), line) {
                        break;
                    }
                    line = app.buf_ref().point().line;
                    let lines = app.buf_ref().line_count();
                    while line < lines && !app.buf_ref().line(line).trim().is_empty() {
                        line += 1;
                    }
                }
            }
        }
        "center-line" => {
            let count = n.unsigned_abs().max(1) as usize;
            let start = app.buf_ref().point().line;
            for i in 0..count {
                let line = start + i;
                if line >= app.buf_ref().line_count() {
                    break;
                }
                fill::center_line_at(app.buf(), line);
            }
            let line = (start + count).min(app.buf_ref().line_count().saturating_sub(1));
            app.buf().set_point(Pos::new(line, 0));
        }
        "set-fill-column" => {
            let b = app.buf_ref();
            let old = b.fill_column;
            let at_point = {
                let p = b.point();
                forge_core::width::display_col(&b.line(p.line), p.col)
            };
            // a numeric argument is the column; a plain C-u means
            // point's column; with no argument, ask (RET keeps point's).
            if app.has_arg() {
                let new = if app.raw_arg() {
                    at_point
                } else {
                    n.max(1) as usize
                };
                app.buf().fill_column = new;
                app.msg(format!("Fill column set to {new} (was {old})"));
            } else {
                app.prompt(
                    format!("Change fill-column from {old} to: "),
                    Purpose::SetFillColumn { default: at_point },
                );
            }
        }
        "upcase-word" => transform_word(app, n, |w| w.to_uppercase()),
        "downcase-word" => transform_word(app, n, |w| w.to_lowercase()),
        "capitalize-word" => transform_word(app, n, capitalize),

        // ------------------------------------------------ mark & region
        "set-mark-command" => {
            let repeated = app.last_command == "set-mark-command";
            // set-mark-command: a prefix argument jumps to the
            // mark and pops the next entry off the mark ring into the
            // mark, the way pop-to-mark-command does; a big one
            // (C-u C-u, or any argument past 4) sets the mark here
            // instead. With set-mark-command-repeat-pop on, a plain
            // C-SPC right after a pop pops again.
            let arg = app.has_arg();
            let big = arg && app.count() > 4;
            let pops = if big {
                false
            } else if arg {
                true
            } else {
                app.set_mark_command_repeat_pop && app.last_command == "pop-to-mark-command"
            };
            if pops {
                if app.buf().pop_to_mark() {
                    app.rename_command("pop-to-mark-command");
                } else {
                    app.error("No mark set in this buffer");
                }
            } else if big {
                // C-u C-u C-SPC sets the mark here, whatever came before.
                set_mark_here(app);
            } else if repeated && app.buf_ref().mark_active() {
                // C-SPC C-SPC turns the highlight off again.
                app.buf().deactivate_mark();
                app.msg("Mark deactivated");
            } else {
                set_mark_here(app);
            }
        }
        "exchange-point-and-mark" => {
            if !app.buf().exchange_point_and_mark() {
                app.error("No mark set in this buffer");
            }
        }
        "mark-whole-buffer" => {
            let b = app.buf();
            let end = b.end();
            b.push_mark(end, true);
            b.beginning_of_buffer();
            app.msg("Mark set");
        }
        "keyboard-quit" => {
            app.buf().deactivate_mark();
            app.msg("Quit");
        }

        // ---------------------------------------------- kill & yank
        "clipboard-kill-region" => match app.buf_ref().region() {
            Some((a, b)) => {
                let text = app.buf().delete_range(a, b);
                app.kill_with_clipboard_override(text, false, true);
            }
            None => app.error("The mark is not set now, so there is no region"),
        },
        "clipboard-kill-ring-save" => match app.buf_ref().region() {
            Some((a, b)) => {
                let text = app.buf_ref().text_range(a, b);
                app.kill_ring_save_with_clipboard_override(text, true);
                app.buf().deactivate_mark();
            }
            None => app.error("The mark is not set now, so there is no region"),
        },
        "clipboard-yank" => {
            let text = app
                .clipboard_provider
                .get_text(forge_core::clipboard::SelectionTarget::Clipboard);
            match text {
                Some(text) if !text.is_empty() => {
                    app.editor.kill_ring.push(text.clone());
                    app.last_clipboard_out = Some(text.replace('\0', "\\0"));
                    let b = app.buf();
                    let start = b.point();
                    b.insert_str(&text);
                    let end = b.point();
                    b.push_mark(start, false);
                    app.yank_range = Some((start, end));
                }
                _ => {
                    if app.editor.kill_ring.is_empty() {
                        app.error("Kill ring is empty");
                    } else {
                        execute(app, "yank");
                    }
                }
            }
        }
        "kill-line" => kill_line(app),
        "kill-region" => match app.buf_ref().region() {
            Some((a, b)) => {
                let text = app.buf().delete_range(a, b);
                app.kill(text, false);
            }
            None => app.error("The mark is not set now, so there is no region"),
        },
        "kill-ring-save" => match app.buf_ref().region() {
            Some((a, b)) => {
                let text = app.buf_ref().text_range(a, b);
                app.kill_ring_save(text);
                app.buf().deactivate_mark();
            }
            None => app.error("The mark is not set now, so there is no region"),
        },
        "delete-region" => match app.buf_ref().region() {
            Some((a, b)) => {
                let _ = app.buf().delete_range(a, b);
            }
            None => app.error("The mark is not set now, so there is no region"),
        },
        "select-and-paste" => {
            if app.editor.kill_ring.is_empty() {
                app.error("Kill ring is empty");
            } else {
                let candidates: Vec<String> = (0..app.editor.kill_ring.len())
                    .filter_map(|i| app.editor.kill_ring.get(i))
                    .enumerate()
                    .map(|(i, text)| format!("{}: {}", i + 1, kill_preview(text)))
                    .collect();
                app.prompt(
                    "Select and paste: ",
                    Purpose::Choose {
                        what: Choice::KillEntry,
                        candidates,
                    },
                );
            }
        }
        "kill-word" | "backward-kill-word" => {
            let n = if cmd == "kill-word" { n } else { -n };
            let start = app.buf_ref().point();
            repeat_until_stuck(n, || {
                if n > 0 {
                    app.buf().forward_word()
                } else {
                    app.buf().backward_word()
                }
            });
            let end = app.buf_ref().point();
            let text = app.buf().delete_range(start, end);
            app.kill(text, n < 0);
        }
        "kill-sentence" | "backward-kill-sentence" => {
            let re = match app.sentence_end_re() {
                Ok(re) => re,
                Err(e) => {
                    app.error(e);
                    return true;
                }
            };
            let backward = cmd == "backward-kill-sentence";
            let mut text = String::new();
            for _ in 0..n.unsigned_abs() {
                let chars: Vec<char> = app.buf_ref().get_contents().chars().collect();
                let from = app.buf_ref().point_offset();
                let (s, e) = if backward {
                    text_nav::sentence_at(&chars, from, app.sentence_end, re.as_ref())
                } else {
                    let start = from;
                    let end =
                        text_nav::forward_sentence(&chars, from, app.sentence_end, re.as_ref());
                    (start, end)
                };
                if s == e {
                    break;
                }
                let b = app.buf();
                let a_pos = b.offset_to_pos(s);
                let z_pos = b.offset_to_pos(e);
                let piece = b.delete_range(a_pos, z_pos);
                if backward {
                    text.insert_str(0, &piece);
                } else {
                    text.push_str(&piece);
                }
            }
            if !text.is_empty() {
                app.kill(text, backward);
            }
        }
        "yank" => {
            app.prepare_yank();
            if app.has_arg() && !app.raw_arg() {
                for _ in 1..n.max(1) {
                    app.editor.kill_ring.rotate();
                }
            }
            match app.editor.kill_ring.current().map(str::to_owned) {
                Some(text) => {
                    let b = app.buf();
                    let start = b.point();
                    b.insert_str(&text);
                    let end = b.point();
                    if app.raw_arg() {
                        // C-u C-y: point before the text, mark after.
                        app.buf().push_mark(end, false);
                        app.buf().set_point(start);
                    } else {
                        app.buf().push_mark(start, false);
                    }
                    app.yank_range = Some((start, end));
                }
                None => app.error("Kill ring is empty"),
            }
        }
        "yank-pop" => {
            let after_yank = matches!(
                app.last_command.as_str(),
                "yank" | "yank-pop" | "clipboard-yank"
            );
            match (after_yank, app.yank_range) {
                (true, Some((start, end))) => {
                    for _ in 1..n.max(1) {
                        app.editor.kill_ring.rotate();
                    }
                    let text = app.editor.kill_ring.rotate().unwrap_or_default().to_owned();
                    if app.yank_pop_change_selection {
                        app.interprogram_cut(&text);
                    }
                    let b = app.buf();
                    b.replace_range(start, end, &text);
                    let new_end = b.point();
                    b.push_mark(start, false);
                    app.yank_range = Some((start, new_end));
                }
                _ => app.error("Previous command was not a yank"),
            }
        }

        // -------------------------------------------------------- undo
        "undo" | "undo-redo" => {
            let redo = cmd == "undo-redo";
            let mut done = 0;
            for _ in 0..n.max(1) {
                let ok = if redo {
                    app.buf().redo()
                } else {
                    app.buf().undo()
                };
                if !ok {
                    break;
                }
                done += 1;
            }
            match (done, redo) {
                (0, false) => app.error("No further undo information"),
                (0, true) => app.error("No further redo information"),
                (_, false) => app.msg("Undo"),
                (_, true) => app.msg("Redo"),
            }
        }

        // ------------------------------------------------------ search
        "isearch-forward" => minibuffer::start_isearch(app, true),
        "isearch-backward" => minibuffer::start_isearch(app, false),
        "query-replace" | "query-replace-regexp" | "replace-string" | "replace-regexp" => {
            let b = app.buf_ref();
            let region = b
                .active_region()
                .map(|(a, z)| (b.pos_to_offset(a), b.pos_to_offset(z)));
            let req = ReplaceRequest {
                regexp: cmd.ends_with("regexp"),
                delimited: app.has_arg(),
                query: cmd.starts_with("query"),
                region,
            };
            let prompt = match &app.last_replace {
                Some((f, t)) => format!("{} (default {f} → {t}): ", req.name()),
                None => format!("{}: ", req.name()),
            };
            app.prompt(prompt, Purpose::ReplaceFrom(req));
        }

        // ------------------------------------------- files & buffers
        "find-file" => {
            let input = crate::minibuffer::initial_find_file_input(app);
            app.prompt(
                "Find file: ",
                Purpose::FindFile {
                    other_window: false,
                    other_frame: false,
                    other_tab: false,
                },
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = input;
            }
        }
        "find-file-other-window" => {
            let input = crate::minibuffer::initial_find_file_input(app);
            app.prompt(
                "Find file in other window: ",
                Purpose::FindFile {
                    other_window: true,
                    other_frame: false,
                    other_tab: false,
                },
            );
            if let Some(mb) = app.minibuffer.as_mut() {
                mb.input = input;
            }
        }
        "write-file" => app.prompt("Write file: ", Purpose::WriteFile),
        "save-buffer" => {
            let b = app.buf_ref();
            if b.path().is_none() {
                app.prompt("File to save in: ", Purpose::WriteFile);
            } else if !b.is_modified() {
                app.msg("(No changes need to be saved)");
            } else if b.changed_on_disk() {
                let (id, name) = (app.active_id(), b.name().to_string());
                app.prompt(
                    format!("{name} has changed on disk since it was read; save anyway? (y or n) "),
                    Purpose::Confirm(ConfirmAction::Save(id)),
                );
            } else {
                save(app);
            }
        }
        "revert-buffer" => crate::files::revert_buffer(app),
        "steal-file-lock" => match app.buf_ref().lock_conflict().map(|o| o.describe()) {
            None => app.msg("The current buffer has no conflicting file lock"),
            Some(_) => match app.buf().steal_file_lock() {
                Ok(()) => app.msg("Took the file lock"),
                Err(e) => app.error(format!("Couldn't take file lock: {e}")),
            },
        },
        "recover-this-file" => crate::files::recover_this_file(app),
        "switch-to-buffer" | "switch-to-buffer-other-window" => {
            let other_window = cmd.ends_with("other-window");
            let default = app
                .other_buffer()
                .and_then(|id| app.editor.state.get_buffer(id))
                .map(|b| b.name().to_string());
            let what = if other_window {
                "Switch to buffer in other window"
            } else {
                "Switch to buffer"
            };
            let prompt = match &default {
                Some(d) => format!("{what} (default {d}): "),
                None => format!("{what}: "),
            };
            app.prompt(
                prompt,
                Purpose::SwitchBuffer {
                    default,
                    other_window,
                    other_frame: false,
                    other_tab: false,
                },
            );
        }
        "kill-buffer" => {
            let default = app.buf_ref().name().to_string();
            app.prompt(
                format!("Kill buffer (default {default}): "),
                Purpose::KillBuffer { default },
            );
        }
        "list-buffers" => {
            let mut text =
                String::from(" MR Buffer                File\n -- ------                ----\n");
            for id in app.editor.buffer_ids() {
                let b = app.editor.state.get_buffer(id).expect("listed");
                if b.name() == "*Buffer List*" {
                    continue;
                }
                let flag = if b.is_modified() { '*' } else { ' ' };
                let path = b
                    .path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                text.push_str(&format!("  {flag} {:<21} {path}\n", b.name()));
            }
            text.push_str("\nSwitch buffers with C-x b.");
            app.show_special_buffer("*Buffer List*", "Buffer-menu-mode", &text);
        }
        "scratch-buffer" => {
            let id = match app.editor.find_buffer("*scratch*") {
                Some(id) => id,
                None => {
                    let id = app.editor.add_buffer(Buffer::new("*scratch*"));
                    if let Some(b) = app.editor.state.get_buffer_mut(id) {
                        b.set_major_mode(forge_core::rhai::INTERACTION_MODE);
                    }
                    id
                }
            };
            app.switch_to(id);
            app.msg("Switched to *scratch*");
        }
        "save-buffers-kill-terminal" => {
            let unsaved = app
                .editor
                .state
                .buffers
                .values()
                .any(|b| b.is_modified() && b.path().is_some());
            if unsaved {
                app.prompt(
                    "Modified buffers exist; exit anyway? (y or n) ",
                    Purpose::Confirm(ConfirmAction::Quit),
                );
            } else {
                app.quit = true;
            }
        }

        // -------------------------------------------------------- misc
        "execute-extended-command" => {
            let arg = app.prefix_arg;
            let shown = match arg {
                Some(a) if app.raw_arg() && a.value() == 4 => "C-u M-x ".to_string(),
                Some(a) => format!("{} M-x ", a.value()),
                None => "M-x ".to_string(),
            };
            app.prompt(shown, Purpose::ExtendedCommand(arg));
        }
        "undefined" => {
            let keys = describe_key_sequence(&app.this_command_keys);
            app.error(format!("{keys} is undefined"));
        }
        "xterm-mouse-mode" => {
            app.mouse = if app.has_arg() {
                app.count() > 0
            } else {
                !app.mouse
            };
            app.msg(if app.mouse {
                "Mouse enabled: the wheel scrolls Forge (Shift+drag selects text in most terminals)"
            } else {
                "Mouse disabled: the terminal handles the mouse"
            });
        }
        "quit-window" => match app.other_buffer() {
            Some(id) => app.switch_to(id),
            None => app.msg("No other buffer"),
        },

        // -------------------------------------------------------- keys
        "global-set-key" => app.read_key_sequence(ReadKeyFor::SetKey { local: false }),
        "local-set-key" => app.read_key_sequence(ReadKeyFor::SetKey { local: true }),
        "reload-init-file" => {
            app.load_init_file();
            crate::declare::resolve(app);
            app.report_unknown_bindings();
            if app.message.is_none() {
                match &app.init_path {
                    Some(p) if p.exists() => app.msg(format!("Loaded {}", p.display())),
                    Some(p) => app.msg(format!("Key bindings reset; no file at {}", p.display())),
                    None => app.msg("Key bindings reset to the defaults"),
                }
            }
        }

        // -------------------------------------------------------- help
        "describe-bindings" => help::describe_bindings(app),
        "describe-mode" => help::describe_mode(app),
        "help-follow-link" => crate::help::follow_link(app),
        "help-next-link" => crate::help::next_link(app),
        "help-prev-link" => crate::help::previous_link(app),
        "help-with-tutorial" => crate::tutorial::open(app),
        "tutorial-next-exercise" => crate::tutorial::next(app),
        "tutorial-prev" => crate::tutorial::prev(app),
        "tutorial-exit" => crate::tutorial::exit(app),
        // ------------------------------------------------ manual (info)
        "info" | "manual" => crate::info::open(app),
        "info-next" => crate::info::next(app),
        "info-prev" => crate::info::prev(app),
        "info-up" => crate::info::up(app),
        "info-top" => crate::info::top(app),
        "info-forward-node" => crate::info::forward_node(app),
        "info-backward-node" => crate::info::backward_node(app),
        "info-last" => crate::info::last(app),
        "info-history-forward" => crate::info::history_forward(app),
        "info-follow-reference" => crate::info::follow(app),
        "info-next-reference" => crate::info::next_ref(app),
        "info-prev-reference" => crate::info::prev_ref(app),
        "info-quit" => crate::info::quit(app),
        "info-toc" => crate::info::toc(app),
        "info-goto-node" => app.prompt(
            "Go to node: ",
            Purpose::Choose {
                what: Choice::GotoNode,
                candidates: crate::info::node_titles(),
            },
        ),
        "info-menu" => {
            let candidates = crate::info::menu_candidates(app);
            if candidates.is_empty() {
                app.error("This node has no menu");
            } else {
                app.prompt(
                    "Menu item: ",
                    Purpose::Choose {
                        what: Choice::InfoMenu,
                        candidates,
                    },
                );
            }
        }
        "info-search" => app.prompt(
            "Search manual (regexp): ",
            Purpose::Choose {
                what: Choice::InfoSearch,
                candidates: Vec::new(),
            },
        ),
        "info-index" => app.prompt(
            "Index entry: ",
            Purpose::Choose {
                what: Choice::InfoIndex,
                candidates: crate::info::index_terms(),
            },
        ),
        "info-lookup-command" => app.prompt(
            "Look up command: ",
            Purpose::Choose {
                what: Choice::InfoLookup,
                candidates: help::symbol_candidates(app),
            },
        ),
        // --------------------------------------------- prompt keys
        "minibuffer-abort" => crate::minibuffer::cmd_abort(app),
        "minibuffer-exit" => crate::minibuffer::cmd_exit(app),
        "minibuffer-exit-candidate" => crate::minibuffer::cmd_exit_completion(app),
        "minibuffer-exit-with-input" => crate::minibuffer::cmd_exit_with_input(app),
        "minibuffer-complete" => crate::minibuffer::cmd_complete(app),
        "minibuffer-insert-candidate" => crate::minibuffer::cmd_insert_candidate(app),
        "minibuffer-delete-backward" => crate::minibuffer::cmd_delete_backward(app),
        "minibuffer-beginning-of-line" => crate::minibuffer::cmd_edit(app, "beginning-of-line"),
        "minibuffer-end-of-line" => crate::minibuffer::cmd_edit(app, "end-of-line"),
        "minibuffer-forward-char" => crate::minibuffer::cmd_edit(app, "forward-char"),
        "minibuffer-backward-char" => crate::minibuffer::cmd_edit(app, "backward-char"),
        "minibuffer-forward-word" => crate::minibuffer::cmd_edit(app, "forward-word"),
        "minibuffer-backward-word" => crate::minibuffer::cmd_edit(app, "backward-word"),
        "minibuffer-delete-char" => crate::minibuffer::cmd_edit(app, "delete-char"),
        "minibuffer-kill-word" => crate::minibuffer::cmd_edit(app, "kill-word"),
        "minibuffer-backward-kill-word" => crate::minibuffer::cmd_edit(app, "backward-kill-word"),
        "minibuffer-kill-line" => crate::minibuffer::cmd_edit(app, "kill-line"),
        "minibuffer-kill-whole-line" => crate::minibuffer::cmd_edit(app, "kill-whole-line"),
        "minibuffer-yank" => crate::minibuffer::cmd_edit(app, "yank"),
        "minibuffer-transpose-chars" => crate::minibuffer::cmd_edit(app, "transpose-chars"),
        "minibuffer-upcase-word" => crate::minibuffer::cmd_edit(app, "upcase-word"),
        "minibuffer-downcase-word" => crate::minibuffer::cmd_edit(app, "downcase-word"),
        "minibuffer-capitalize-word" => crate::minibuffer::cmd_edit(app, "capitalize-word"),
        "minibuffer-completion-help" => crate::minibuffer::cmd_completion_help(app),
        "minibuffer-history-prev" => crate::minibuffer::cmd_history_prev(app),
        "minibuffer-history-next" => crate::minibuffer::cmd_history_next(app),
        "minibuffer-list-next" => crate::minibuffer::cmd_list_next(app),
        "minibuffer-list-prev" => crate::minibuffer::cmd_list_prev(app),
        "minibuffer-list-next-page" => crate::minibuffer::cmd_list_next_page(app),
        "minibuffer-list-prev-page" => crate::minibuffer::cmd_list_prev_page(app),
        "minibuffer-list-first" => crate::minibuffer::cmd_list_first(app),
        "minibuffer-list-last" => crate::minibuffer::cmd_list_last(app),
        "minibuffer-yes" => crate::minibuffer::cmd_yes(app),
        "completion-popup-next" => crate::capf::cmd_next(app),
        "completion-popup-prev" => crate::capf::cmd_prev(app),
        "completion-popup-expand" => crate::capf::cmd_expand(app),
        "completion-popup-insert" => crate::capf::cmd_insert(app),
        "completion-popup-abort" => crate::capf::cmd_abort(app),
        "completion-popup-move-to-minibuffer" => crate::capf::cmd_move_to_minibuffer(app),
        "minibuffer-no" => crate::minibuffer::cmd_no(app),

        // ------------------------------------------------------ menu keys
        "menu-bar-open" => crate::menu::open_next(app),
        "menu-bar-menu-close" => crate::menu::cmd_close(app),
        "menu-bar-menu-quit" => crate::menu::cmd_quit(app),
        "menu-bar-menu-next-item" => crate::menu::cmd_next_item(app),
        "menu-bar-menu-prev-item" => crate::menu::cmd_prev_item(app),
        "menu-bar-menu-next-menu" => crate::menu::cmd_next_menu(app),
        "menu-bar-menu-prev-menu" => crate::menu::cmd_prev_menu(app),
        "menu-bar-menu-first-item" => crate::menu::cmd_first_item(app),
        "menu-bar-menu-last-item" => crate::menu::cmd_last_item(app),
        "menu-bar-menu-choose" => crate::menu::cmd_choose(app),

        "describe-key" => app.read_key_sequence(ReadKeyFor::DescribeKey),
        "describe-key-briefly" => app.read_key_sequence(ReadKeyFor::DescribeKeyBriefly),
        "describe-function" | "describe-command" => {
            app.prompt("Describe function: ", Purpose::DescribeFunction)
        }
        "goto-char" => app.prompt("Goto char: ", Purpose::GotoChar),
        "insert-file" => app.prompt("Insert file: ", Purpose::InsertFile),
        "make-directory" | "mkdir" => app.prompt("Make directory: ", Purpose::FileOp(FileOp::MakeDirectory)),
        "make-empty-file" => app.prompt("Create empty file: ", Purpose::FileOp(FileOp::MakeEmptyFile)),
        "copy-directory" => app.prompt("Copy directory: ", Purpose::FileOp(FileOp::CopyFrom)),
        "delete-directory" => app.prompt("Delete directory: ", Purpose::FileOp(FileOp::DeleteDirectory)),
        "insert-directory" => app.prompt("Insert directory listing of: ", Purpose::FileOp(FileOp::InsertDirectory)),
        "apropos-command" => app.prompt(
            "Apropos command (word list or regexp): ",
            Purpose::Apropos { docs: false },
        ),
        "apropos-documentation" => app.prompt(
            "Apropos documentation (regexp): ",
            Purpose::Apropos { docs: true },
        ),
        "help-for-help" => crate::help::help_for_help(app),
        "view-echo-area-messages" => {
            let id = crate::messages::messages_buffer(app);
            app.switch_to(id);
        }
        "view-lossage" => crate::help::view_lossage(app),
        "view-news" => crate::help::view_news(app),
        "describe-symbol" | "describe-variable" | "describe-face" => {
            let default = help::symbol_at_point(app);
            let prompt = match &default {
                Some(d) => format!("Describe symbol (default {d}): "),
                None => "Describe symbol: ".to_string(),
            };
            app.prompt(
                prompt,
                Purpose::Choose {
                    what: Choice::DescribeSymbol { default },
                    candidates: help::symbol_candidates(app),
                },
            );
        }
        "where-is" => app.prompt("Where is command: ", Purpose::WhereIs),

        // ------------------------------------------------------- themes
        "load-theme" => {
            let candidates = app.editor.themes.names();
            if candidates.is_empty() {
                app.error("No themes are defined (see forge.theme.define)");
            } else {
                app.prompt(
                    "Load theme: ",
                    Purpose::Choose {
                        what: Choice::LoadTheme,
                        candidates,
                    },
                );
            }
        }
        "disable-theme" => match app.editor.themes.current().map(str::to_string) {
            Some(name) => {
                app.editor.disable_theme();
                app.msg(format!("Disabled theme: {name}"));
            }
            None => app.error("No theme is enabled"),
        },
        "list-themes" => {
            let names = app.editor.themes.names();
            if names.is_empty() {
                app.error("No themes are defined (see forge.theme.define)");
            } else {
                let current = app.editor.themes.current().map(str::to_string);
                let mut t = String::from("Themes\n\n");
                for name in &names {
                    let mark = if current.as_deref() == Some(name.as_str()) {
                        "*"
                    } else {
                        " "
                    };
                    let desc = app.editor.themes.description(name).unwrap_or_default();
                    if desc.is_empty() {
                        t.push_str(&format!("{mark} {name}\n"));
                    } else {
                        t.push_str(&format!("{mark} {name} - {desc}\n"));
                    }
                }
                app.show_special_buffer("*Themes*", "special-mode", &t);
            }
        }

        // ------------------------------------------------------ plugins
        "load-plugin" => app.prompt("Load plugin: ", Purpose::LoadPlugin),
        "describe-plugin" => {
            let names = plugins::loaded_names(app);
            if names.is_empty() {
                app.error("No plugins are loaded");
            } else {
                app.prompt(
                    "Describe plugin: ",
                    Purpose::Choose {
                        what: Choice::DescribePlugin,
                        candidates: names,
                    },
                );
            }
        }
        "list-plugins" => plugins::list(app),
        "list-projects" => crate::project::list(app),
        "eval-expression" => app.prompt("Eval: ", Purpose::EvalRhai),
        "eval-rhai-last-sexp" => plugins::eval_rhai_sexp_before(app, false),
        "eval-rhai-print-last-sexp" => plugins::eval_rhai_sexp_before(app, true),
        "revoke-plugin-permissions" => {
            if plugins::granted_names(app).is_empty() {
                app.error("Nothing has been granted: no plugin, no project");
            } else {
                app.prompt(
                    "Forget the grant of (a plugin or project:...): ",
                    Purpose::RevokePluginPermissions,
                );
            }
        }
        "reload-plugin" => {
            if plugins::reloadable_names(app).is_empty() {
                app.error("No plugins were loaded from a file");
            } else {
                app.prompt("Reload plugin: ", Purpose::ReloadPlugin);
            }
        }
        "unload-plugin" => {
            if plugins::loaded_names(app).is_empty() {
                app.error("No plugins are loaded");
            } else {
                app.prompt("Unload plugin: ", Purpose::UnloadPlugin);
            }
        }

        _ => {
            return windows::execute(app, cmd)
                || crate::window_extras::execute(app, cmd)
                || crate::frames::execute(app, cmd)
                || crate::tabs::execute(app, cmd)
                || crate::toolbar::execute(app, cmd)
                || crate::builtins::execute(app, cmd)
                || crate::grep::execute(app, cmd)
                || crate::compile::execute(app, cmd)
                || crate::dired::execute(app, cmd)
                || crate::tether::execute(app, cmd)
                || crate::capf::execute(app, cmd)
                || crate::packages::execute(app, cmd)
                || kmacro::execute(app, cmd)
                || registers::execute(app, cmd)
                || bookmarks::execute(app, cmd)
                || crate::vc::execute(app, cmd)
                || crate::iso_transl::execute(app, cmd)
                || crate::input_method::execute(app, cmd)
                || crate::few::execute(app, cmd)
                || crate::server::execute(app, cmd)
                || crate::term::execute(app, cmd)
                || crate::icon::execute(app, cmd)
                || crate::canvas::execute(app, cmd)
                || crate::dialog::execute(app, cmd)
                || crate::mode_line::execute(app, cmd)
                || crate::hover::execute(app, cmd)
                || crate::image_mode::execute(app, cmd)
                || crate::webkit::execute(app, cmd)
                || plugins::run_command(app, cmd)
                || plugins::run_rhai_command(app, cmd);
        }
    }
    true
}

/// Insert kill-ring entry `index` the way yank inserts the latest one,
/// so M-y cycles from there afterwards.
pub fn insert_kill(app: &mut App, index: usize) {
    match app.editor.kill_ring.get(index).map(str::to_owned) {
        Some(text) => {
            let b = app.buf();
            let start = b.point();
            b.insert_str(&text);
            let end = b.point();
            b.push_mark(start, false);
            app.yank_range = Some((start, end));
        }
        None => app.error("Kill ring is empty"),
    }
}

/// One line of a kill for the select-and-paste list.
fn kill_preview(text: &str) -> String {
    let one_line: String = text
        .chars()
        .take(60)
        .map(|c| if c == '\n' { '⏎' } else { c })
        .collect();
    format!(
        "{one_line}{}",
        if text.chars().count() > 60 { "…" } else { "" }
    )
}

/// Save the current buffer and report it.
pub fn save(app: &mut App) {
    crate::org_dblock::before_save(app);
    tidy_before_save(app);
    if crate::tether::save(app, false) {
        return;
    }
    if crate::org_crypt::save_with_crypt(app) {
        return;
    }
    let result = app.buf().save();
    let path = app
        .buf_ref()
        .path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    match result {
        Ok(()) => {
            make_script_executable(app);
            app.msg(format!("Wrote {path}"));
            crate::files::after_save(app);
        }
        Err(e) => app.error(format!("Error saving {path}: {e}")),
    }
}

/// `delete-trailing-whitespace` and `require-final-newline` as
/// `before-save-hook` functions, each behind its own option. Point stays
/// where it was.
fn tidy_before_save(app: &mut App) {
    use crate::org_flow::setting_flag;
    use forge_core::Pos;
    if app.buf_ref().path().is_none() {
        return;
    }
    let at = app.buf_ref().point();
    if setting_flag(app, "delete_trailing_whitespace_on_save") {
        for l in (0..app.buf_ref().line_count()).rev() {
            let text = app.buf_ref().line(l);
            let kept = text.trim_end_matches([' ', '\t']).chars().count();
            let len = text.chars().count();
            if kept < len {
                app.buf().delete_range(Pos::new(l, kept), Pos::new(l, len));
            }
        }
    }
    if setting_flag(app, "require_final_newline") {
        let last = app.buf_ref().line_count().saturating_sub(1);
        if app.buf_ref().line_len(last) > 0 {
            let end = Pos::new(last, app.buf_ref().line_len(last));
            app.buf().replace_range(end, end, "\n");
        }
    }
    let max = app.buf_ref().line_count().saturating_sub(1);
    app.buf().set_point(Pos::new(at.line.min(max), at.col));
}

/// `executable-make-buffer-file-executable-if-script-p`: after a save,
/// a file that starts with `#!` gets its execute bits.
fn make_script_executable(app: &App) {
    use std::os::unix::fs::PermissionsExt;
    if !crate::org_flow::setting_flag(app, "make_scripts_executable_on_save")
        || !app.buf_ref().line(0).starts_with("#!")
    {
        return;
    }
    if let Some(p) = app.buf_ref().path()
        && let Ok(meta) = std::fs::metadata(p)
    {
        let mode = meta.permissions().mode();
        if mode & 0o111 == 0 {
            let _ = std::fs::set_permissions(
                p,
                std::fs::Permissions::from_mode(mode | ((mode & 0o444) >> 2)),
            );
        }
    }
}

/// C-v / M-v: a screen at a time (two lines of overlap). Fails only
/// when the window already shows that end of the buffer; point is
/// dragged along if it would leave the window.
pub(crate) fn scroll_screen(app: &mut App, up: bool) {
    let h = app.win().height.max(1);
    let step = h.saturating_sub(2).max(1);
    if up {
        let lines = app.buf_ref().line_count();
        let top = app.win().scroll_top;
        // A screen (or less) of shown rows left: already there.
        let h = h + usize::from(app.win().scroll_y_px > 0);
        if app.buf_ref().visible_lines_between(top, lines) <= h {
            return app.error("End of buffer");
        }
        let next = app.buf_ref().forward_visible_line(top, step);
        app.set_scroll_target(next);
    } else {
        if app.win().scroll_top == 0 {
            return app.error("Beginning of buffer");
        }
        let top = app.win().scroll_top;
        let next = app.buf_ref().backward_visible_line(top, step);
        app.set_scroll_target(next);
    }
}

pub(crate) fn keep_point_in_window(app: &mut App) {
    let (top, h) = (app.win().scroll_top, app.win().height.max(1));
    let line = app.buf_ref().point().line;
    if line < top {
        app.buf().move_lines((top - line) as isize);
    } else if app.buf_ref().visible_lines_between(top, line) >= h {
        // Point is a screen or more below the top: bring it to the last
        // shown row, stepping through visible lines so folds don't count.
        let mut target = top;
        for _ in 0..h.saturating_sub(1) {
            match app.buf_ref().next_visible_line(target) {
                Some(t) if t <= line => target = t,
                _ => break,
            }
        }
        if target != line {
            app.buf().move_lines(target as isize - line as isize);
        }
    }
}

/// C-u N C-v: scroll N lines (negative: back).
fn scroll_lines(app: &mut App, n: i64) {
    let top = app.win().scroll_top;
    let next = if n >= 0 {
        app.buf_ref().forward_visible_line(top, n as usize)
    } else {
        app.buf_ref()
            .backward_visible_line(top, n.unsigned_abs() as usize)
    };
    app.set_scroll_target(next);
}

fn kill_line(app: &mut App) {
    let b = app.buf_ref();
    let p = b.point();
    let (start, end, backward) = if app.has_arg() {
        let n = app.count();
        match n {
            0 => (Pos::new(p.line, 0), p, true),
            n if n > 0 => {
                let target = p.line + n as usize;
                let end = if target < b.line_count() {
                    Pos::new(target, 0)
                } else {
                    b.end()
                };
                (p, end, false)
            }
            n => (
                Pos::new(p.line.saturating_sub(n.unsigned_abs() as usize), 0),
                p,
                true,
            ),
        }
    } else {
        let eol = Pos::new(p.line, b.line_len(p.line));
        let rest_is_blank = b.text_range(p, eol).trim().is_empty();
        let end = if !rest_is_blank {
            eol
        } else if p.line + 1 < b.line_count() {
            // At (or only whitespace before) end of line: kill through the newline.
            Pos::new(p.line + 1, 0)
        } else if p != eol {
            eol
        } else {
            return app.error("End of buffer");
        };
        (p, end, false)
    };
    let text = app.buf().delete_range(start, end);
    app.kill(text, backward);
}

/// C-x C-t.
fn transpose_lines(app: &mut App, n: i64) {
    let swap = |b: &mut Buffer, a: usize| {
        let (x, y) = (b.line(a), b.line(a + 1));
        let end = Pos::new(a + 1, b.line_len(a + 1));
        b.replace_range(Pos::new(a, 0), end, &format!("{y}\n{x}"));
    };
    if n == 0 {
        let b = app.buf_ref();
        let Some(m) = b.mark() else {
            return app.error("No mark set in this buffer");
        };
        let (a, z) = (b.point().line.min(m.line), b.point().line.max(m.line));
        if a != z {
            let (x, y) = (b.line(a), b.line(z));
            let b = app.buf();
            b.replace_range(Pos::new(z, 0), Pos::new(z, b.line_len(z)), &x);
            b.replace_range(Pos::new(a, 0), Pos::new(a, b.line_len(a)), &y);
        }
        return;
    }
    for _ in 0..n.unsigned_abs() {
        let l = app.buf_ref().point().line;
        if n > 0 {
            if l == 0 {
                return app.error("Don't have two things to transpose");
            }
            if l + 1 >= app.buf_ref().line_count() {
                // The last line: make a line for point to move onto.
                let end = app.buf_ref().end();
                app.buf().replace_range(end, end, "\n");
            }
            swap(app.buf(), l - 1);
            app.buf().set_point(Pos::new(l + 1, 0));
        } else {
            if l < 2 {
                return app.error("Don't have two things to transpose");
            }
            swap(app.buf(), l - 2);
            app.buf().set_point(Pos::new(l - 1, 0));
        }
    }
}

/// Paragraphs (runs of non-blank lines) within lines `a..=b`.
fn paragraphs_in(b: &Buffer, a: usize, z: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for l in a..=z {
        let blank = b.line(l).trim().is_empty();
        match (blank, start) {
            (false, None) => start = Some(l),
            (true, Some(s)) => {
                out.push((s, l - 1));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, z));
    }
    out
}

/// Fill every paragraph between lines `a..=z` (M-q with a region, and
/// fill-region), from the bottom up so earlier line numbers stay valid.
fn fill_region(app: &mut App, a: usize, z: usize, justify: bool) {
    for (s, e) in paragraphs_in(app.buf_ref(), a, z).into_iter().rev() {
        fill::fill_line_range_arg(app.buf(), s, e, justify);
    }
}

/// M-u / M-l / M-c: transform the next `n` words (negative: the words
/// before point, leaving point where it is).
fn transform_word(app: &mut App, n: i64, f: impl Fn(&str) -> String) {
    let origin = app.buf_ref().point();
    repeat_until_stuck(n, || {
        if n > 0 {
            app.buf().forward_word()
        } else {
            app.buf().backward_word()
        }
    });
    let other = app.buf_ref().point();
    let (start, end) = if n > 0 {
        (origin, other)
    } else {
        (other, origin)
    };
    let text = app.buf_ref().text_range(start, end);
    let new = f(&text);
    if new != text {
        app.buf().replace_range(start, end, &new);
    }
    if n < 0 {
        app.buf().set_point(origin);
    }
}

fn capitalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_word = false;
    for c in s.chars() {
        if forge_core::buffer::is_word_char(c) {
            if in_word {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            in_word = true;
        } else {
            out.push(c);
            in_word = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PrefixArg;
    use forge_core::{Buffer, Editor};

    fn app() -> App {
        let mut app = App::new(Editor::default());
        let id = app.editor.add_buffer(Buffer::new("*scratch*"));
        app.editor.show_buffer(id);
        app
    }

    #[test]
    fn scratch_evaluates_the_rhai_before_point() {
        let mut app = app();
        app.buf().set_major_mode(forge_core::rhai::INTERACTION_MODE);
        app.execute_keys("1 SPC + SPC 2");
        assert!(execute(&mut app, "eval-rhai-last-sexp"));
        assert_eq!(app.message.as_deref(), Some("3"));

        // C-j prints the value after the expression instead.
        app.execute_keys("C-j");
        assert_eq!(app.buf_ref().get_contents(), "1 + 2\n3");
        assert_eq!(app.buf_ref().point().line, 1);
    }

    #[test]
    fn command_table_is_sorted() {
        let names: Vec<String> = super::command_names().map(str::to_string).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "binary search needs COMMANDS sorted");
    }

    #[test]
    fn auto_fill_breaks_on_a_typed_space_past_the_column() {
        let mut app = app();
        app.execute_keys("a SPC b SPC c SPC d SPC e SPC f SPC g SPC h SPC i SPC j SPC k");
        assert_eq!(app.buf_ref().get_contents(), "a b c d e f g h i j k");
        app.buf().fill_column = 15;
        app.buf().enable_minor_mode("auto-fill-mode");
        // The line fills right up to the column, then breaks.
        app.execute_keys("SPC l");
        assert_eq!(app.buf_ref().get_contents(), "a b c d e f g h\ni j k l");
    }

    #[test]
    fn auto_fill_does_not_break_on_ret_or_a_quoted_space() {
        let mut app = app();
        app.execute_keys("a SPC b SPC c SPC d SPC e SPC f SPC g SPC h SPC i SPC j SPC k");
        app.buf().fill_column = 10;
        app.buf().enable_minor_mode("auto-fill-mode");
        // RET goes through `newline`, not `self-insert-command`.
        app.execute_keys("RET x");
        assert_eq!(app.buf_ref().get_contents(), "a b c d e f g h i j k\nx");
        // C-q SPC inserts a literal space without breaking.
        app.execute_keys("C-q SPC");
        assert_eq!(app.buf_ref().get_contents(), "a b c d e f g h i j k\nx ");
    }

    #[test]
    fn fill_region_fills_every_paragraph() {
        let mut app = app();
        app.buf()
            .set_contents("one two three\nfour five\n\nsix seven eight\nnine");
        app.buf().fill_column = 14;
        let a = Pos::new(0, 0);
        let b = Pos::new(4, 0);
        app.buf().push_mark(a, true);
        app.buf().set_point(b);
        let _ = app.buf_ref().active_region();
        assert!(execute(&mut app, "fill-region"));
        assert_eq!(
            app.buf_ref().get_contents(),
            "one two three\nfour five\n\nsix seven\neight nine"
        );
    }

    #[test]
    fn fill_region_as_paragraph_combines_blank_lines() {
        let mut app = app();
        app.buf().set_contents("one two\n\nthree four\n\nfive six");
        app.buf().fill_column = 16;
        app.buf().push_mark(Pos::new(0, 0), true);
        app.buf().set_point(Pos::new(4, 0));
        assert!(execute(&mut app, "fill-region-as-paragraph"));
        assert_eq!(app.buf_ref().get_contents(), "one two three\nfour five six");
    }

    #[test]
    fn unfill_paragraph_joins_lines() {
        let mut app = app();
        app.buf().set_contents("one two\nthree four\n\nnext");
        app.buf().set_point(Pos::new(1, 0));
        assert!(execute(&mut app, "unfill-paragraph"));
        assert_eq!(app.buf_ref().get_contents(), "one two three four\n\nnext");
    }

    #[test]
    fn center_line_centers_and_moves_past() {
        let mut app = app();
        app.buf().set_contents("hello\nworld\ntail");
        app.buf().fill_column = 11;
        app.buf().set_point(Pos::new(0, 0));
        assert!(execute(&mut app, "center-line"));
        assert_eq!(app.buf_ref().get_contents(), "   hello\nworld\ntail");
        // With an argument, center that many lines and move past them.
        app.buf().set_point(Pos::new(0, 0));
        app.prefix_arg = Some(PrefixArg::Raw(2));
        assert!(execute(&mut app, "center-line"));
        assert_eq!(app.buf_ref().get_contents(), "   hello\n   world\ntail");
        assert_eq!(app.buf_ref().point().line, 2);
    }

    #[test]
    fn set_fill_column_asks_and_takes_the_answer() {
        let mut app = app();
        app.buf().fill_column = 70;
        assert!(execute(&mut app, "set-fill-column"));
        let mb = app.minibuffer.as_ref().expect("asks for a column");
        assert_eq!(mb.prompt, "Change fill-column from 70 to: ");
        app.execute_keys("9 0 RET");
        assert!(app.minibuffer.is_none());
        assert_eq!(app.buf_ref().fill_column, 90);
    }

    #[test]
    fn set_fill_column_empty_answer_keeps_points_column() {
        let mut app = app();
        app.buf().set_contents("abc");
        app.buf().set_point(Pos::new(0, 2));
        app.buf().fill_column = 70;
        assert!(execute(&mut app, "set-fill-column"));
        app.execute_keys("RET");
        assert_eq!(app.buf_ref().fill_column, 2);
    }

    #[test]
    fn set_fill_column_prefix_argument_skips_the_prompt() {
        let mut app = app();
        app.prefix_arg = Some(PrefixArg::Number(100));
        assert!(execute(&mut app, "set-fill-column"));
        assert!(app.minibuffer.is_none());
        assert_eq!(app.buf_ref().fill_column, 100);

        // A plain C-u uses point's column.
        app.buf().set_contents("abcd");
        app.buf().set_point(Pos::new(0, 3));
        app.prefix_arg = Some(PrefixArg::Raw(4));
        assert!(execute(&mut app, "set-fill-column"));
        assert!(app.minibuffer.is_none());
        assert_eq!(app.buf_ref().fill_column, 3);
    }

    #[test]
    fn global_fill_column_indicator_mode_toggles_the_default() {
        let mut app = app();
        assert!(!app.fill_column_indicator);
        assert!(execute(
            &mut app,
            "global-display-fill-column-indicator-mode"
        ));
        assert!(app.fill_column_indicator);
        assert!(execute(
            &mut app,
            "global-display-fill-column-indicator-mode"
        ));
        assert!(!app.fill_column_indicator);
    }

    /// A buffer with three marks set: the two older ones are on the
    /// mark ring, the newest is the mark.
    fn app_with_marks() -> App {
        let mut app = app();
        app.buf().set_contents("one two three");
        for col in [0, 4, 8] {
            app.buf().set_point(Pos::new(0, col));
            app.buf().push_mark(Pos::new(0, col), false);
        }
        app.buf().set_point(Pos::new(0, 12));
        app
    }

    #[test]
    fn c_u_c_spc_walks_back_through_the_mark_ring() {
        let mut app = app_with_marks();
        // C-u C-SPC jumps to the mark and pops the next entry off the
        // ring into the mark.
        app.execute_keys("C-u C-SPC");
        assert_eq!(app.buf_ref().point(), Pos::new(0, 8));
        assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 4)));
        // The next one goes further back.
        app.execute_keys("C-u C-SPC");
        assert_eq!(app.buf_ref().point(), Pos::new(0, 4));
        assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 0)));
        // The positions we walked through are still on the ring.
        assert_eq!(
            app.buf_ref()
                .mark_ring()
                .iter()
                .map(|(_, p)| *p)
                .collect::<Vec<_>>(),
            vec![Pos::new(0, 8), Pos::new(0, 4)]
        );
    }

    #[test]
    fn a_plain_c_spc_after_c_u_c_spc_sets_a_mark_again() {
        let mut app = app_with_marks();
        app.execute_keys("C-u C-SPC");
        assert_eq!(app.buf_ref().point(), Pos::new(0, 8));
        // Without set-mark-command-repeat-pop, C-SPC sets the mark here.
        app.execute_keys("C-SPC");
        assert_eq!(app.buf_ref().point(), Pos::new(0, 8));
        assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 8)));
    }

    #[test]
    fn a_plain_c_spc_pops_again_when_repeat_pop_is_on() {
        let mut app = app_with_marks();
        app.set_mark_command_repeat_pop = true;
        app.execute_keys("C-u C-SPC");
        assert_eq!(app.buf_ref().point(), Pos::new(0, 8));
        // A plain C-SPC right after pops the next entry off the ring.
        app.execute_keys("C-SPC");
        assert_eq!(app.buf_ref().point(), Pos::new(0, 4));
        assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 0)));
    }

    #[test]
    fn c_u_c_spc_without_a_mark_says_so() {
        let mut app = app();
        app.buf().set_contents("one two three");
        app.execute_keys("C-u C-SPC");
        assert!(app.bell);
    }

    #[test]
    fn a_pop_becomes_the_last_command() {
        let mut app = app_with_marks();
        app.execute_keys("C-u C-SPC");
        // The command is renamed renames `this-command',
        // so `last-command' says what really ran.
        assert_eq!(app.last_command, "pop-to-mark-command");
    }

    #[test]
    fn a_big_argument_sets_the_mark_instead_of_popping() {
        let mut app = app_with_marks();
        // C-u C-u, and any argument past 4, sets the mark here.
        app.execute_keys("C-u C-u C-SPC");
        assert_eq!(app.buf_ref().point(), Pos::new(0, 12));
        assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 12)));
        assert_eq!(app.last_command, "set-mark-command");
        // C-u 5 says the same thing.
        app.execute_keys("C-u 5 C-SPC");
        assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 12)));
        // C-u 4 is still a plain C-u, so it pops: it jumps to the mark
        // and the next entry off the ring becomes the mark.
        app.execute_keys("C-u 4 C-SPC");
        assert_eq!(app.buf_ref().point(), Pos::new(0, 12));
        assert_eq!(app.buf_ref().mark(), Some(Pos::new(0, 8)));
    }

    #[test]
    fn a_repetition_stops_when_the_motion_stops_moving() {
        let mut calls = 0;
        repeat_until_stuck(1_000_000_000, || {
            calls += 1;
            calls < 3
        });
        assert_eq!(calls, 3, "the repetition ends with the motion");
    }

    #[test]
    fn a_big_count_does_not_spin_at_the_end_of_the_buffer() {
        let mut app = app();
        app.buf().set_contents("one two three");
        // A count far bigger than the buffer: the motion stops at the end
        // of it, and the command does not spin through the whole count.
        app.prefix_arg = Some(PrefixArg::Number(100_000_000));
        let start = std::time::Instant::now();
        assert!(execute(&mut app, "forward-word"));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "the motion stops at the end of the buffer"
        );
        assert_eq!(app.buf_ref().point_offset(), app.buf_ref().char_count());
    }
}
