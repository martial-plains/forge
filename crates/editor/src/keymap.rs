// SPDX-License-Identifier: MIT OR Apache-2.0
//! Default key bindings and turning terminal key events into key
//! notation. The keymaps themselves (layers, lookup, remapping) live in
//! `forge_core::keymap`.

use forge_core::keymap::{Keymaps, Origin, Scope, shifted};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};

/// The default bindings: (key sequence, command name).
pub const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    // Motion
    ("C-f", "forward-char"),
    ("<right>", "forward-char"),
    ("C-b", "backward-char"),
    ("<left>", "backward-char"),
    ("C-n", "next-line"),
    ("<down>", "next-line"),
    ("C-p", "previous-line"),
    ("<up>", "previous-line"),
    ("C-a", "move-beginning-of-line"),
    ("<home>", "move-beginning-of-line"),
    ("C-e", "move-end-of-line"),
    ("<end>", "move-end-of-line"),
    ("M-f", "forward-word"),
    ("C-<right>", "forward-word"),
    ("M-b", "backward-word"),
    ("C-<left>", "backward-word"),
    ("M-@", "mark-word"),
    ("M-a", "backward-sentence"),
    ("M-e", "forward-sentence"),
    ("M-{", "backward-paragraph"),
    ("M-}", "forward-paragraph"),
    ("C-M-a", "beginning-of-defun"),
    ("C-M-e", "end-of-defun"),
    ("C-M-h", "mark-defun"),
    ("C-M-f", "forward-sexp"),
    ("C-M-b", "backward-sexp"),
    ("C-M-n", "forward-list"),
    ("C-M-p", "backward-list"),
    ("C-M-d", "down-list"),
    ("C-M-u", "up-list"),
    ("C-x [", "backward-page"),
    ("C-x ]", "forward-page"),
    ("M-<", "beginning-of-buffer"),
    ("C-<home>", "beginning-of-buffer"),
    ("M->", "end-of-buffer"),
    ("C-<end>", "end-of-buffer"),
    ("C-v", "scroll-up-command"),
    ("<next>", "scroll-up-command"),
    ("M-v", "scroll-down-command"),
    ("<prior>", "scroll-down-command"),
    ("C-l", "recenter-top-bottom"),
    ("M-g g", "goto-line"),
    ("M-g M-g", "goto-line"),
    // Editing
    ("RET", "newline"),
    ("C-j", "newline-without-indent"),
    ("M-m", "back-to-indentation"),
    ("C-M-\\", "indent-region"),
    ("C-o", "open-line"),
    ("TAB", "indent-for-tab-command"),
    ("C-d", "delete-char"),
    ("<deletechar>", "delete-char"),
    ("DEL", "delete-backward-char"),
    ("C-t", "transpose-chars"),
    ("M-t", "transpose-words"),
    ("C-M-t", "transpose-sexps"),
    ("M-u", "upcase-word"),
    ("M-l", "downcase-word"),
    ("M-c", "capitalize-word"),
    // Mark and region
    ("C-SPC", "set-mark-command"),
    ("C-x C-x", "exchange-point-and-mark"),
    ("C-x h", "mark-whole-buffer"),
    ("M-h", "mark-paragraph"),
    ("C-x C-p", "mark-page"),
    ("C-x l", "count-lines-page"),
    ("C-x i", "insert-file"),
    ("C-g", "keyboard-quit"),
    // Killing and yanking
    ("C-k", "kill-line"),
    ("C-w", "kill-region"),
    ("M-w", "kill-ring-save"),
    ("M-d", "kill-word"),
    ("M-DEL", "backward-kill-word"),
    ("M-k", "kill-sentence"),
    ("C-x DEL", "backward-kill-sentence"),
    ("C-M-k", "kill-sexp"),
    ("C-y", "yank"),
    ("M-y", "yank-pop"),
    // Undo
    ("C-/", "undo"),
    ("C-_", "undo"),
    ("C-x u", "undo"),
    ("C-M-/", "undo-redo"),
    ("C-M-_", "undo-redo"),
    // Search
    ("C-s", "isearch-forward"),
    ("C-r", "isearch-backward"),
    // Files and buffers
    ("C-x C-f", "find-file"),
    ("C-x C-s", "save-buffer"),
    ("C-x C-w", "write-file"),
    ("C-x b", "switch-to-buffer"),
    ("C-x k", "kill-buffer"),
    ("C-x C-b", "list-buffers"),
    ("C-x C-c", "save-buffers-kill-terminal"),
    ("C-x #", "server-edit"),
    ("C-q", "quoted-insert"),
    ("M-q", "fill-paragraph"),
    ("C-x f", "set-fill-column"),
    ("C-x C-t", "transpose-lines"),
    ("M-^", "delete-indentation"),
    // Prefix arguments
    ("C-u", "universal-argument"),
    ("M-0", "digit-argument"),
    ("M-1", "digit-argument"),
    ("M-2", "digit-argument"),
    ("M-3", "digit-argument"),
    ("M-4", "digit-argument"),
    ("M-5", "digit-argument"),
    ("M-6", "digit-argument"),
    ("M-7", "digit-argument"),
    ("M-8", "digit-argument"),
    ("M-9", "digit-argument"),
    ("M--", "negative-argument"),
    // Replacing
    ("M-%", "query-replace"),
    ("C-M-%", "query-replace-regexp"),
    // Keyboard macros
    ("C-x (", "start-kbd-macro"),
    ("C-x )", "end-kbd-macro"),
    ("C-x e", "kmacro-end-and-call-macro"),
    ("<f3>", "start-kbd-macro"),
    ("<f4>", "kmacro-end-or-call-macro"),
    // Rectangles and registers
    ("C-x r k", "kill-rectangle"),
    ("C-x r d", "delete-rectangle"),
    ("C-x r y", "yank-rectangle"),
    ("C-x r o", "open-rectangle"),
    ("C-x r c", "clear-rectangle"),
    ("C-x r t", "string-rectangle"),
    ("C-x r N", "rectangle-number-lines"),
    ("C-x r M-w", "copy-rectangle-as-kill"),
    ("C-x r s", "copy-to-register"),
    ("C-x r x", "copy-to-register"),
    ("C-x r r", "copy-rectangle-to-register"),
    ("C-x r i", "insert-register"),
    ("C-x r g", "insert-register"),
    ("C-x r SPC", "point-to-register"),
    ("C-x r C-SPC", "point-to-register"),
    ("C-x r j", "jump-to-register"),
    ("C-x r n", "number-to-register"),
    ("C-x r m", "bookmark-set"),
    ("C-x r b", "bookmark-jump"),
    ("C-x r +", "increment-register"),
    // Windows
    ("C-x 2", "split-window-below"),
    ("C-x 3", "split-window-right"),
    ("C-x o", "other-window"),
    ("C-x 0", "delete-window"),
    ("C-x 1", "delete-other-windows"),
    ("C-x +", "balance-windows"),
    ("C-x C-+", "text-scale-increase"),
    ("C-x C-=", "text-scale-increase"),
    ("C-x C--", "text-scale-decrease"),
    ("C-x C-0", "text-scale-reset"),
    ("C-x ^", "enlarge-window"),
    ("C-x }", "enlarge-window-horizontally"),
    ("C-x {", "shrink-window-horizontally"),
    ("C-x d", "dired"),
    ("C-x C-j", "dired-jump"),
    ("C-x 4 f", "find-file-other-window"),
    ("C-x 4 C-f", "find-file-other-window"),
    ("C-x 4 b", "switch-to-buffer-other-window"),
    ("C-M-v", "scroll-other-window"),
    // Frames
    ("C-x 5 2", "make-frame"),
    ("C-x 5 0", "delete-frame"),
    ("C-x 5 1", "delete-other-frames"),
    ("C-x 5 o", "other-frame"),
    ("C-x 5 u", "undelete-frame"),
    ("C-x 5 f", "find-file-other-frame"),
    ("C-x 5 C-f", "find-file-other-frame"),
    ("C-x 5 b", "switch-to-buffer-other-frame"),
    ("C-x 5 d", "dired-other-frame"),
    // Tabs (tab-bar-mode): each tab is a saved window configuration.
    ("C-x t 2", "tab-new"),
    ("C-x t 0", "tab-close"),
    ("C-x t 1", "tab-close-other"),
    ("C-x t o", "tab-next"),
    ("S-C-TAB", "tab-previous"),
    ("C-x t RET", "tab-switch"),
    ("C-x t r", "tab-rename"),
    ("C-x t m", "tab-move"),
    ("C-x t u", "tab-undo"),
    ("C-x t b", "switch-to-buffer-other-tab"),
    ("C-x t f", "find-file-other-tab"),
    ("C-x t d", "dired-other-tab"),
    // Misc
    ("M-x", "execute-extended-command"),
    ("M-:", "eval-expression"),
    // Built-in features (Phase 9)
    ("M-;", "comment-dwim"),
    ("C-x C-;", "comment-line"),
    ("M-/", "dabbrev-expand"),
    ("M-=", "count-words-region"),
    ("M-g i", "imenu"),
    ("C-M-i", "completion-at-point"),
    ("M-TAB", "completion-at-point"),
    // Projects, compiling, shell commands
    ("M-g n", "next-error"),
    ("M-g M-n", "next-error"),
    ("C-x `", "next-error"),
    ("M-g p", "previous-error"),
    ("M-g M-p", "previous-error"),
    ("M-!", "shell-command"),
    ("M-&", "async-shell-command"),
    ("C-x p f", "project-find-file"),
    ("C-x p g", "project-find-regexp"),
    ("C-x p c", "project-compile"),
    ("M-s o", "occur"),
    // Help
    ("C-h b", "describe-bindings"),
    ("C-h c", "describe-key-briefly"),
    ("C-h f", "describe-function"),
    ("C-h i", "info"),
    ("C-h k", "describe-key"),
    ("C-h m", "describe-mode"),
    ("C-h o", "describe-symbol"),
    ("C-h r", "manual"),
    ("C-h w", "where-is"),
    ("C-h v", "describe-variable"),
    ("C-h a", "apropos-command"),
    ("C-h d", "apropos-documentation"),
    ("C-h l", "view-lossage"),
    ("C-h n", "view-news"),
    ("C-h t", "help-with-tutorial"),
    ("C-h ?", "help-for-help"),
    ("C-h C-h", "help-for-help"),
    // The menu bar opens it: F10, or M-` for the mouse
    // users' reach.
    ("<f10>", "menu-bar-open"),
    ("M-`", "menu-bar-open"),
    // The context menu, from the keyboard (when context-menu-mode is on).
    ("S-<f10>", "context-menu-open"),
];

/// help-mode: RET and `<mouse-1>` follow the link at point; TAB and S-TAB
/// move between links. The rest comes from special-mode (q, SPC, DEL...).
pub const HELP_MODE_BINDINGS: &[(&str, &str)] = &[
    ("RET", "help-follow-link"),
    ("<mouse-1>", "help-follow-link"),
    ("TAB", "help-next-link"),
    ("<backtab>", "help-prev-link"),
];

/// info-mode: the Info-style manual browser (C-h i). Node navigation and
/// reference following are its own commands; the rest (q, SPC, DEL...) is
/// inherited from special-mode.
pub const INFO_MODE_BINDINGS: &[(&str, &str)] = &[
    ("RET", "info-follow-reference"),
    ("<mouse-1>", "info-follow-reference"),
    ("TAB", "info-next-reference"),
    ("<backtab>", "info-prev-reference"),
    ("n", "info-next"),
    ("p", "info-prev"),
    ("u", "info-up"),
    ("t", "info-top"),
    ("]", "info-forward-node"),
    ("[", "info-backward-node"),
    ("l", "info-last"),
    ("r", "info-history-forward"),
    ("g", "info-goto-node"),
    ("m", "info-menu"),
    ("T", "info-toc"),
    ("s", "info-search"),
    ("i", "info-index"),
    ("C-h R", "info-lookup-command"),
];

/// image-mode: the picture and its file (the "Viewing Image Files" node).
/// `i`/`s`/`a` are prefixes; the rest are single keys.
pub const IMAGE_MODE_BINDINGS: &[(&str, &str)] = &[
    ("C-c C-c", "image-toggle-display"),
    ("C-c C-x", "image-toggle-hex-display"),
    ("n", "image-next-file"),
    ("p", "image-previous-file"),
    ("m", "image-mode-mark-file"),
    ("u", "image-mode-unmark-file"),
    ("w", "image-mode-copy-file-name-as-kill"),
    ("RET", "image-toggle-animation"),
    ("f", "image-next-frame"),
    ("b", "image-previous-frame"),
    ("F", "image-goto-frame"),
    ("a +", "image-increase-speed"),
    ("a -", "image-decrease-speed"),
    ("a r", "image-reverse-speed"),
    ("a 0", "image-reset-speed"),
    ("s w", "image-transform-fit-to-window"),
    ("s p", "image-transform-set-percent"),
    ("s s", "image-transform-set-scale"),
    ("s 0", "image-transform-reset-to-initial"),
    ("s o", "image-transform-reset-to-original"),
    ("i +", "image-increase-size"),
    ("i -", "image-decrease-size"),
    ("i r", "image-rotate"),
    ("i h", "image-flip-horizontally"),
    ("i v", "image-flip-vertically"),
    ("i o", "image-save"),
];

/// tutorial-mode: n/p walk the exercises, q leaves, RET follows a link.
/// The rest (SPC, DEL...) is inherited from special-mode.
pub const TUTORIAL_MODE_BINDINGS: &[(&str, &str)] = &[
    ("RET", "help-follow-link"),
    ("<mouse-1>", "help-follow-link"),
    ("n", "tutorial-next-exercise"),
    ("p", "tutorial-prev"),
    ("q", "tutorial-exit"),
];

/// special-mode: read-only style buffers like *Help* and *Buffer List*.
pub const SPECIAL_MODE_BINDINGS: &[(&str, &str)] = &[
    ("q", "quit-window"),
    ("SPC", "scroll-up-command"),
    ("DEL", "scroll-down-command"),
    ("n", "next-line"),
    ("p", "previous-line"),
    ("<", "beginning-of-buffer"),
    (">", "end-of-buffer"),
];

/// Built-in major modes that derive from special-mode.
///
pub const SPECIAL_MODES: &[&str] = &[
    "help-mode",
    "info-mode",
    "tutorial-mode",
    "messages-buffer-mode",
    "Buffer-menu-mode",
    "occur-mode",
    "compilation-mode",
    "shell-command-mode",
    "package-menu-mode",
    "dired-mode",
    "image-dired-thumbnail-mode",
    "tether-dir-mode",
    "org-agenda-mode",
    "org-columns-mode",
    "few-mode",
    "webkit-mode",
];

/// *scratch*'s Rhai interaction mode: evaluate what is before point, like
/// lisp-interaction-mode.
pub const RHAI_INTERACTION_BINDINGS: &[(&str, &str)] = &[
    ("C-x C-e", "eval-rhai-last-sexp"),
    ("C-j", "eval-rhai-print-last-sexp"),
];

/// *Packages* (M-x list-packages).
pub const PACKAGE_MENU_BINDINGS: &[(&str, &str)] = &[
    ("RET", "package-menu-install"),
    ("i", "package-menu-install"),
    ("d", "package-menu-delete"),
    ("g", "package-menu-refresh"),
    ("U", "package-upgrade-all"),
];

/// What a click does in a listing: `<mouse-1>` visits the name under point,
/// as RET does. This is a region map rather than a mode binding, so the
/// keys apply only while point is on a name — the columns beside it are
/// not part of the link, which is why a click there does nothing.
const DIRED_ENTRY_BINDINGS: &[(&str, &str)] = &[
    ("<mouse-1>", "dired-find-file"),
    ("<mouse-2>", "dired-find-file-other-window"),
];

/// Dired (M-x dired, C-x d).
pub const DIRED_MODE_BINDINGS: &[(&str, &str)] = &[
    ("RET", "dired-find-file"),
    ("f", "dired-find-file"),
    ("e", "dired-find-file"),
    ("o", "dired-find-file-other-window"),
    ("^", "dired-up-directory"),
    ("m", "dired-mark"),
    ("u", "dired-unmark"),
    ("U", "dired-unmark-all-marks"),
    ("d", "dired-flag-file-deletion"),
    ("x", "dired-do-flagged-delete"),
    ("D", "dired-do-delete"),
    ("C", "dired-do-copy"),
    ("R", "dired-do-rename"),
    ("+", "dired-create-directory"),
    ("g", "dired-revert"),
    // The layout toggles, like eza's own flags. `s` cycles --sort,
    // `T` switches --tree for everything, and the rest switch their part
    // on and off. `i` lists one directory inline as a tree.
    ("s", "dired-toggle-sort"),
    ("i", "dired-maybe-insert-subdir"),
    ("I", "dired-toggle-inodes"),
    ("r", "dired-toggle-reverse"),
    ("/", "dired-toggle-only-dirs"),
    ("B", "dired-toggle-icons"),
    ("C-c C-e", "dired-toggle-ls-style"),
    // Inline subdirectories: `i` inserts one as a tree, `k` removes one.
    ("C-M-n", "dired-next-subdir"),
    ("C-M-p", "dired-prev-subdir"),
    ("?", "dired-summary"),
    // Marking: the `*` and `%` families and friends.
    ("* *", "dired-mark-executables"),
    ("* /", "dired-mark-directories"),
    ("* @", "dired-mark-symlinks"),
    ("* s", "dired-mark-subdir-files"),
    ("* t", "dired-toggle-marks"),
    ("t", "dired-toggle-marks"),
    ("* c", "dired-change-marks"),
    ("* %", "dired-mark-files-regexp"),
    ("* .", "dired-mark-extension"),
    ("* m", "dired-mark"),
    ("* u", "dired-unmark"),
    ("* !", "dired-unmark-all-marks"),
    ("* DEL", "dired-unmark-backward"),
    ("DEL", "dired-unmark-backward"),
    ("% m", "dired-mark-files-regexp"),
    ("% d", "dired-flag-files-regexp"),
    ("% g", "dired-mark-files-containing-regexp"),
    ("% u", "dired-upcase"),
    ("% l", "dired-downcase"),
    ("% R", "dired-do-rename-regexp"),
    ("% C", "dired-do-copy-regexp"),
    ("% H", "dired-do-hardlink-regexp"),
    ("% S", "dired-do-symlink-regexp"),
    ("% Y", "dired-do-relsymlink-regexp"),
    ("~", "dired-flag-backup-files"),
    ("#", "dired-flag-auto-save-files"),
    (".", "dired-clean-directory"),
    ("M-}", "dired-next-marked-file"),
    ("M-{", "dired-prev-marked-file"),
    (">", "dired-next-dirline"),
    ("<", "dired-prev-dirline"),
    ("n", "dired-next-line"),
    ("p", "dired-previous-line"),
    ("SPC", "dired-next-line"),
    ("j", "dired-goto-file"),
    // Operations on the marked files (or this one).
    ("S", "dired-do-symlink"),
    ("H", "dired-do-hardlink"),
    ("Y", "dired-do-relsymlink"),
    ("M", "dired-do-chmod"),
    ("O", "dired-do-chown"),
    ("Z", "dired-do-compress"),
    ("P", "dired-do-print"),
    ("!", "dired-do-shell-command"),
    ("X", "dired-do-shell-command"),
    ("&", "dired-do-async-shell-command"),
    ("A", "dired-do-find-regexp"),
    ("Q", "dired-do-find-regexp-and-replace"),
    ("v", "dired-view-file"),
    ("C-o", "dired-display-file"),
    ("=", "dired-diff"),
    ("y", "dired-show-file-type"),
    ("w", "dired-copy-filename-as-kill"),
    // own keys come first. The eza layout toggles that used to sit
    // on some of them live under C-c C-: a all, d dirs first, t tree, c
    // colour scale, g git-ignore, f only files, l tree level, k kill the
    // inline listing.
    ("a", "dired-find-alternate-file"),
    ("c", "dired-do-compress-to"),
    ("G", "dired-do-chgrp"),
    ("T", "dired-do-touch"),
    ("F", "dired-do-find-marked-files"),
    ("l", "dired-do-redisplay"),
    ("k", "dired-do-kill-lines"),
    ("h", "describe-mode"),
    ("(", "dired-hide-details-mode"),
    ("$", "dired-hide-subdir"),
    ("M-$", "dired-hide-all"),
    ("C-x M-o", "dired-omit-mode"),
    ("C-t d", "image-dired-display-thumbs"),
    ("C-t i", "dired-find-file"),
    ("C-c C-a", "dired-toggle-hidden"),
    ("C-c C-d", "dired-toggle-dirs-first"),
    ("C-c C-t", "dired-toggle-tree"),
    ("C-c C-c", "dired-toggle-color-scale"),
    ("C-c C-g", "dired-toggle-git-ignore"),
    ("C-c C-f", "dired-toggle-only-files"),
    ("C-c C-l", "dired-tree-set-level"),
    ("C-c C-k", "dired-kill-subdir"),
    // Editing names in place.
    ("C-x C-q", "wdired-change-to-wdired-mode"),
];

/// The thumbnails buffer of Dired's C-t d.
pub const IMAGE_DIRED_BINDINGS: &[(&str, &str)] = &[("RET", "image-dired-thumbnail-open")];

/// wdired: names edited in a Dired buffer; nothing is renamed until C-c C-c.
pub const WDIRED_MODE_BINDINGS: &[(&str, &str)] = &[
    ("C-c C-c", "wdired-finish-edit"),
    ("C-x C-s", "wdired-finish-edit"),
    ("C-c C-k", "wdired-abort-changes"),
    ("C-c ESC", "wdired-abort-changes"),
];

/// org-cdlatex-mode: math input in Org buffers. The mode is buffer-local
/// and off until enabled.
pub const ORG_CDLATEX_MODE_BINDINGS: &[(&str, &str)] = &[
    ("_", "cdlatex-subscript"),
    ("^", "cdlatex-superscript"),
    ("`", "cdlatex-math-symbol"),
    ("$", "cdlatex-dollar"),
    ("C-c {", "cdlatex-environment"),
];

/// *compilation* and *grep* (grep-mode derives from compilation-mode).
pub const COMPILATION_MODE_BINDINGS: &[(&str, &str)] = &[
    ("RET", "compile-goto-error"),
    ("g", "recompile"),
    ("n", "compilation-next-error"),
    ("p", "compilation-previous-error"),
    ("C-c C-k", "kill-compilation"),
];

/// *grep*: the keys of Grep mode, on top of Compilation mode's.
pub const GREP_MODE_BINDINGS: &[(&str, &str)] = &[
    ("e", "grep-edit"),
    ("C-c C-c", "grep-edit-exit"),
    ("M-n", "compilation-next-error"),
    ("M-p", "compilation-previous-error"),
];

/// *Occur*: RET goes to the line.
pub const OCCUR_MODE_BINDINGS: &[(&str, &str)] = &[("RET", "occur-mode-goto-occurrence")];

/// Programming modes: M-q refills a comment or reindents the defun, as
/// prog-mode does.
pub const PROG_MODE_BINDINGS: &[(&str, &str)] = &[("M-q", "fill-reindent-defun")];

/// org-mode (built in, for .org files).
pub const ORG_MODE_BINDINGS: &[(&str, &str)] = &[
    ("TAB", "org-cycle"),
    ("<backtab>", "org-shifttab"),
    ("C-c C-t", "org-todo"),
    ("C-c C-e", "org-export-dispatch"),
    ("C-c C-x f", "org-footnote-action"),
    ("C-c C-x C-f", "org-emphasize"),
    ("C-c C-v t", "org-babel-tangle"),
    ("C-c C-v e", "org-babel-execute-src-block"),
    ("C-c C-v b", "org-babel-execute-buffer"),
    ("C-c C-v s", "org-babel-execute-subtree"),
    ("C-c C-v n", "org-babel-next-src-block"),
    ("C-c C-v p", "org-babel-previous-src-block"),
    ("C-c C-v u", "org-babel-goto-src-block-head"),
    ("C-c C-v g", "org-babel-goto-named-src-block"),
    ("C-c C-v r", "org-babel-goto-named-result"),
    ("C-c C-v I", "org-babel-view-src-block-info"),
    ("C-c C-v j", "org-babel-insert-header-arg"),
    ("C-c C-v d", "org-babel-demarcate-block"),
    ("C-c C-v x", "org-babel-expand-src-block"),
    ("C-c C-v o", "org-babel-open-src-block-result"),
    ("C-c C-v h", "org-babel-mark-block"),
    ("C-c C-v k", "org-babel-remove-result"),
    ("C-c C-v l", "org-babel-lob-ingest"),
    ("C-c C-x @", "org-cite-insert"),
    ("C-c C-x C-v", "org-link-preview"),
    ("C-c C-x C-M-v", "org-link-preview-refresh"),
    ("C-c C-c", "org-ctrl-c-ctrl-c"),
    ("M-RET", "org-meta-return"),
    ("M-<left>", "org-metaleft"),
    ("M-<right>", "org-metaright"),
    ("M-<up>", "org-metaup"),
    ("M-<down>", "org-metadown"),
    ("C-c C-n", "org-next-visible-heading"),
    ("C-c C-p", "org-previous-visible-heading"),
    ("C-c C-f", "org-forward-heading-same-level"),
    ("C-c C-b", "org-backward-heading-same-level"),
    ("C-c C-u", "outline-up-heading"),
    ("C-c C-j", "org-goto"),
    ("C-c ^", "org-sort"),
    ("C-c C-q", "org-set-tags-command"),
    ("C-c C-a", "org-attach"),
    ("C-c C-x C-c", "org-columns"),
    ("C-c C-x p", "org-set-property"),
    ("C-c C-x e", "org-set-effort"),
    ("C-c C-.", "org-time-stamp"),
    ("C-c C-!", "org-time-stamp-inactive"),
    ("C-c C-s", "org-schedule"),
    ("C-c C-d", "org-deadline"),
    ("C-c C-o", "org-open-at-point"),
    ("<mouse-1>", "org-mouse-open"),
    ("<mouse-2>", "org-mouse-open"),
    ("C-c C-l", "org-insert-link"),
    ("C-c C-x C-b", "org-toggle-checkbox"),
    ("C-c C-,", "org-insert-structure-template"),
    ("C-c ;", "org-toggle-comment"),
    ("S-<up>", "org-shiftup"),
    ("S-<down>", "org-shiftdown"),
    ("S-<left>", "org-shiftleft"),
    ("S-<right>", "org-shiftright"),
    ("C-c C-x C-i", "org-clock-in"),
    ("C-c C-x C-o", "org-clock-out"),
    ("C-c C-x C-x", "org-clock-cancel"),
    ("C-c C-x C-j", "org-clock-goto"),
    ("C-c C-x C-d", "org-clock-display"),
    ("C-c C-x C-r", "org-clock-report"),
    ("C-c C-x C-u", "org-dblock-update"),
    ("C-c C-x x", "org-dynamic-block-insert-dblock"),
    ("C-c C-x ;", "org-timer-set-timer"),
    ("C-c C-x 0", "org-timer-start"),
    ("C-c C-x ,", "org-timer-pause-or-continue"),
    ("C-c C-x _", "org-timer-stop"),
    ("C-c C-w", "org-refile"),
    ("C-c C-x C-w", "org-cut-subtree"),
    ("C-c C-x M-w", "org-copy-subtree"),
    ("C-c C-x C-y", "org-paste-subtree"),
    ("C-c C-x C-a", "org-archive-subtree-default"),
    ("C-c C-x a", "org-toggle-archive-tag"),
    ("C-c C-x A", "org-archive-to-archive-sibling"),
    ("C-c C-x <", "org-agenda-set-restriction-lock"),
    ("C-c C-x >", "org-agenda-remove-restriction-lock"),
    ("C-c |", "org-table-create-or-convert-from-region"),
    ("C-c -", "org-ctrl-c-minus"),
    ("C-c RET", "org-table-hline-and-move"),
    ("C-c }", "org-table-toggle-coordinate-overlays"),
    ("C-c {", "org-table-toggle-formula-debugger"),
    // C-c C-* makes a subtree of the list item; recalculating a
    // table is C-c * (below).
    ("C-c C-*", "org-list-item-to-subtree"),
    ("C-c *", "org-ctrl-c-star"),
    ("C-c .", "org-time-stamp"),
    ("C-c !", "org-time-stamp-inactive"),
    ("C-c /", "org-sparse-tree"),
    ("C-c \\", "org-match-sparse-tree"),
    ("C-c C-r", "org-reveal"),
    ("C-c C-^", "org-up-element"),
    ("C-c [", "org-agenda-file-to-front"),
    ("C-c ]", "org-remove-file"),
    ("C-c &", "org-mark-ring-goto"),
    ("C-c C-x d", "org-insert-drawer"),
    ("C-c C-x o", "org-toggle-ordered-property"),
    ("C-c C-x \\", "org-toggle-pretty-entities"),
    ("C-c C-x C-l", "org-latex-preview"),
    ("C-c C-x C-n", "org-next-link"),
    ("C-c C-x C-p", "org-previous-link"),
    ("C-c C-x c", "org-clone-subtree-with-time-shift"),
    ("C-c C-x b", "org-tree-to-indirect-buffer"),
    ("C-c C-x C-z", "org-resolve-clocks"),
    ("C-c C-x C-e", "org-clock-modify-effort-estimate"),
    ("C-c C-x -", "org-timer-item"),
    ("C-c C-x C-s", "org-archive-subtree"),
    ("C-c ,", "org-priority"),
    ("C-c C-z", "org-add-note"),
    ("C-c C-y", "org-evaluate-time-range"),
    ("C-c SPC", "org-table-blank-field"),
    ("C-c =", "org-table-eval-formula"),
    ("S-RET", "org-table-copy-down"),
    ("M-S-<up>", "org-shiftmetaup"),
    ("M-S-<down>", "org-shiftmetadown"),
    ("RET", "org-return"),
    ("C-RET", "org-insert-heading-respect-content"),
    ("C-S-RET", "org-insert-todo-heading-respect-content"),
    ("M-S-RET", "org-insert-todo-heading"),
    ("M-S-<left>", "org-shiftmetaleft"),
    ("M-S-<right>", "org-shiftmetaright"),
    ("C-c +", "org-table-sum"),
    ("C-c ?", "org-table-field-info"),
    ("C-c '", "org-table-edit-formulas"),
];

/// Set up the built-in keymaps. Everything here is bound as a package
/// binding, so it can never take a `C-c <letter>` key from the user.
/// What a prompt answers to. Every prompt that is not one of the special
/// cases below uses this map, so a prompt can be rebound as a whole or one
/// key of it can be.
const PROMPT_BINDINGS: &[(&str, &str)] = &[
    ("C-g", "minibuffer-abort"),
    ("ESC", "minibuffer-abort"),
    ("RET", "minibuffer-exit"),
    ("C-j", "minibuffer-exit"),
    ("DEL", "minibuffer-delete-backward"),
    ("TAB", "minibuffer-complete"),
    ("M-p", "minibuffer-history-prev"),
    ("<up>", "minibuffer-history-prev"),
    ("M-n", "minibuffer-history-next"),
    ("<down>", "minibuffer-history-next"),
    ("?", "minibuffer-completion-help"),
    ("C-a", "minibuffer-beginning-of-line"),
    ("<home>", "minibuffer-beginning-of-line"),
    ("C-e", "minibuffer-end-of-line"),
    ("<end>", "minibuffer-end-of-line"),
    ("C-f", "minibuffer-forward-char"),
    ("<right>", "minibuffer-forward-char"),
    ("C-b", "minibuffer-backward-char"),
    ("<left>", "minibuffer-backward-char"),
    ("M-f", "minibuffer-forward-word"),
    ("C-<right>", "minibuffer-forward-word"),
    ("M-b", "minibuffer-backward-word"),
    ("C-<left>", "minibuffer-backward-word"),
    ("C-d", "minibuffer-delete-char"),
    ("<deletechar>", "minibuffer-delete-char"),
    ("M-d", "minibuffer-kill-word"),
    ("M-DEL", "minibuffer-backward-kill-word"),
    ("C-k", "minibuffer-kill-line"),
    ("C-y", "minibuffer-yank"),
    ("C-t", "minibuffer-transpose-chars"),
    ("M-u", "minibuffer-upcase-word"),
    ("M-l", "minibuffer-downcase-word"),
    ("M-c", "minibuffer-capitalize-word"),
];

/// The same keys, plus the ones that only mean something while the vertical
/// completion list is up: moving in it, taking the candidate under the
/// cursor, and M-RET to answer with the text as typed.
const COMPLETION_LIST_BINDINGS: &[(&str, &str)] = &[
    ("C-g", "minibuffer-abort"),
    ("ESC", "minibuffer-abort"),
    ("RET", "minibuffer-exit-candidate"),
    ("M-RET", "minibuffer-exit-with-input"),
    ("C-j", "minibuffer-exit"),
    ("DEL", "minibuffer-delete-backward"),
    ("TAB", "minibuffer-insert-candidate"),
    ("M-p", "minibuffer-history-prev"),
    ("M-n", "minibuffer-history-next"),
    ("?", "minibuffer-completion-help"),
    ("C-a", "minibuffer-beginning-of-line"),
    ("<home>", "minibuffer-beginning-of-line"),
    ("C-e", "minibuffer-end-of-line"),
    ("<end>", "minibuffer-end-of-line"),
    ("C-f", "minibuffer-forward-char"),
    ("<right>", "minibuffer-forward-char"),
    ("C-b", "minibuffer-backward-char"),
    ("<left>", "minibuffer-backward-char"),
    ("M-f", "minibuffer-forward-word"),
    ("C-<right>", "minibuffer-forward-word"),
    ("M-b", "minibuffer-backward-word"),
    ("C-<left>", "minibuffer-backward-word"),
    ("C-d", "minibuffer-delete-char"),
    ("<deletechar>", "minibuffer-delete-char"),
    ("M-d", "minibuffer-kill-word"),
    ("M-DEL", "minibuffer-backward-kill-word"),
    ("C-k", "minibuffer-kill-line"),
    ("C-y", "minibuffer-yank"),
    ("C-t", "minibuffer-transpose-chars"),
    ("M-u", "minibuffer-upcase-word"),
    ("M-l", "minibuffer-downcase-word"),
    ("M-c", "minibuffer-capitalize-word"),
    ("C-n", "minibuffer-list-next"),
    ("<down>", "minibuffer-list-next"),
    ("C-p", "minibuffer-list-prev"),
    ("<up>", "minibuffer-list-prev"),
    ("<next>", "minibuffer-list-next-page"),
    ("C-M-n", "minibuffer-list-next-page"),
    ("<prior>", "minibuffer-list-prev-page"),
    ("C-M-p", "minibuffer-list-prev-page"),
];

/// Prompts that answer yes or no.
const YES_NO_BINDINGS: &[(&str, &str)] = &[
    ("C-g", "minibuffer-abort"),
    ("ESC", "minibuffer-abort"),
    ("y", "minibuffer-yes"),
    ("Y", "minibuffer-yes"),
    ("n", "minibuffer-no"),
    ("N", "minibuffer-no"),
];

/// The register prompt takes one character, and that character is the
/// register name rather than a command, so only the ways out are bound.
const REGISTER_BINDINGS: &[(&str, &str)] =
    &[("C-g", "minibuffer-abort"), ("ESC", "minibuffer-abort")];

/// An open menu's keys, in the transient map it pushes while it is up, so
/// they can be rebound like any other binding.
const MENU_BINDINGS: &[(&str, &str)] = &[
    ("C-g", "menu-bar-menu-quit"),
    ("ESC", "menu-bar-menu-close"),
    ("C-n", "menu-bar-menu-next-item"),
    ("<down>", "menu-bar-menu-next-item"),
    ("C-p", "menu-bar-menu-prev-item"),
    ("<up>", "menu-bar-menu-prev-item"),
    ("C-f", "menu-bar-menu-next-menu"),
    ("<right>", "menu-bar-menu-next-menu"),
    ("C-b", "menu-bar-menu-prev-menu"),
    ("<left>", "menu-bar-menu-prev-menu"),
    ("<home>", "menu-bar-menu-first-item"),
    ("<end>", "menu-bar-menu-last-item"),
    ("TAB", "menu-bar-menu-choose"),
    ("RET", "menu-bar-menu-choose"),
];

/// The completion popup's keys, while it is open.
const COMPLETION_POPUP_BINDINGS: &[(&str, &str)] = &[
    ("C-g", "completion-popup-abort"),
    ("ESC", "completion-popup-abort"),
    ("C-n", "completion-popup-next"),
    ("<down>", "completion-popup-next"),
    ("C-p", "completion-popup-prev"),
    ("<up>", "completion-popup-prev"),
    ("M-TAB", "completion-popup-expand"),
    ("TAB", "completion-popup-insert"),
    ("RET", "completion-popup-insert"),
    ("M-m", "completion-popup-move-to-minibuffer"),
    // corfu-map (M-<, M->, C-v, M-v, C-q, M-q) is bound by the corfu
    // plugin, not the editor.
];

/// install the prompt maps, so a key in a prompt is looked up like any
/// other key and can be rebound by the user or a plugin.
pub fn install_prompt_defaults(keymaps: &mut Keymaps) {
    for (name, bindings) in [
        (forge_core::keymap::PROMPT_DEFAULT, PROMPT_BINDINGS),
        (crate::minibuffer::COMPLETION_LIST, COMPLETION_LIST_BINDINGS),
        ("register", REGISTER_BINDINGS),
        ("key-choice", REGISTER_BINDINGS),
        (crate::capf::PROMPT, COMPLETION_POPUP_BINDINGS),
        ("plugin-grant", YES_NO_BINDINGS),
        ("plugin-y-or-n", YES_NO_BINDINGS),
        ("confirm", YES_NO_BINDINGS),
    ] {
        let scope = Scope::Prompt(name.into());
        for (keys, cmd) in bindings {
            keymaps
                .bind(&scope, keys, cmd, Origin::Package)
                .expect("valid default prompt binding");
        }
    }
}

pub fn install_defaults(keymaps: &mut Keymaps) {
    install_prompt_defaults(keymaps);
    // An open menu's map is a transient one, since it is up only while a
    // menu is open, but its keys are bound here so the map is never empty.
    let menu = Scope::Transient(crate::menu::TRANSIENT.into());
    for (keys, cmd) in MENU_BINDINGS {
        keymaps
            .bind(&menu, keys, cmd, Origin::Package)
            .expect("valid default menu binding");
    }
    for (keys, cmd) in DEFAULT_BINDINGS {
        keymaps
            .bind(&Scope::Global, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let special = Scope::Mode("special-mode".into());
    for (keys, cmd) in SPECIAL_MODE_BINDINGS {
        keymaps
            .bind(&special, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    for mode in SPECIAL_MODES {
        keymaps.set_mode_parent(mode, "special-mode");
    }
    let help = Scope::Mode("help-mode".into());
    for (keys, cmd) in HELP_MODE_BINDINGS {
        keymaps
            .bind(&help, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let info = Scope::Mode(crate::info::MODE.into());
    for (keys, cmd) in INFO_MODE_BINDINGS {
        keymaps
            .bind(&info, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let image = Scope::Mode(crate::image_mode::MODE.into());
    for (keys, cmd) in IMAGE_MODE_BINDINGS {
        keymaps
            .bind(&image, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let canvas = Scope::Mode(forge_core::canvas::MODE.into());
    for (keys, cmd) in crate::canvas::CANVAS_MODE_BINDINGS {
        keymaps
            .bind(&canvas, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let tutorial = Scope::Mode(crate::tutorial::MODE.into());
    for (keys, cmd) in TUTORIAL_MODE_BINDINGS {
        keymaps
            .bind(&tutorial, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let rhai = Scope::Mode(forge_core::rhai::INTERACTION_MODE.into());
    keymaps.set_mode_parent(forge_core::rhai::INTERACTION_MODE, "rhai-mode");
    for (keys, cmd) in RHAI_INTERACTION_BINDINGS {
        keymaps
            .bind(&rhai, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let org = Scope::Mode(forge_core::org::MODE.into());
    keymaps.set_mode_parent(forge_core::org::MODE, "text-mode");
    for (keys, cmd) in ORG_MODE_BINDINGS {
        keymaps
            .bind(&org, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let cdlatex = Scope::Mode("org-cdlatex-mode".into());
    for (keys, cmd) in ORG_CDLATEX_MODE_BINDINGS {
        keymaps
            .bind(&cdlatex, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    keymaps.set_mode_parent("grep-mode", "compilation-mode");
    let compilation = Scope::Mode("compilation-mode".into());
    for (keys, cmd) in COMPILATION_MODE_BINDINGS {
        keymaps
            .bind(&compilation, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let grep = Scope::Mode("grep-mode".into());
    for (keys, cmd) in GREP_MODE_BINDINGS {
        keymaps
            .bind(&grep, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let menu = Scope::Mode("package-menu-mode".into());
    for (keys, cmd) in PACKAGE_MENU_BINDINGS {
        keymaps
            .bind(&menu, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let agenda = Scope::Mode(crate::org_agenda_ui::MODE.into());
    for (keys, cmd) in crate::org_agenda_ui::AGENDA_BINDINGS {
        keymaps
            .bind(&agenda, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let columns = Scope::Mode(crate::org_extra::COLUMNS_MODE.into());
    for (keys, cmd) in crate::org_extra::COLUMNS_BINDINGS {
        keymaps
            .bind(&columns, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let few = Scope::Mode(crate::few::MODE.into());
    for (keys, cmd) in crate::few::BINDINGS {
        keymaps
            .bind(&few, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    // What a click does on a link, button or picture in a page: <mouse-1>
    // uses whatever is under point, as RET does. A region map rather than
    // a mode binding, so a click beside one does nothing.
    let few_click = Scope::Region(crate::few::CLICK_REGION.into());
    for (keys, cmd) in crate::few::CLICK_BINDINGS {
        keymaps
            .bind(&few_click, keys, cmd, Origin::Package)
            .expect("valid default region binding");
    }
    let webkit = Scope::Mode(crate::webkit::MODE.into());
    for (keys, cmd) in crate::webkit::BINDINGS {
        keymaps
            .bind(&webkit, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let dired = Scope::Mode("dired-mode".into());
    for (keys, cmd) in DIRED_MODE_BINDINGS {
        keymaps
            .bind(&dired, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let thumbs = Scope::Mode(crate::dired::THUMBS_MODE.into());
    for (keys, cmd) in IMAGE_DIRED_BINDINGS {
        keymaps
            .bind(&thumbs, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let tether = Scope::Mode(crate::tether::DIR_MODE.into());
    for (keys, cmd) in crate::tether::DIR_MODE_BINDINGS {
        keymaps
            .bind(&tether, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let wdired = Scope::Mode(crate::dired::WDIRED_MODE.into());
    for (keys, cmd) in WDIRED_MODE_BINDINGS {
        keymaps
            .bind(&wdired, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let entry = Scope::Region(crate::dired::ENTRY_REGION.into());
    for (keys, cmd) in DIRED_ENTRY_BINDINGS {
        keymaps
            .bind(&entry, keys, cmd, Origin::Package)
            .expect("valid default region binding");
    }
    let occur = Scope::Mode("occur-mode".into());
    for (keys, cmd) in OCCUR_MODE_BINDINGS {
        keymaps
            .bind(&occur, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    let prog = Scope::Mode("prog-mode".into());
    for (keys, cmd) in PROG_MODE_BINDINGS {
        keymaps
            .bind(&prog, keys, cmd, Origin::Package)
            .expect("valid default binding");
    }
    // Version control: C-x v and its buffers' modes.
    crate::vc::install_keymaps(keymaps);
    crate::term::install_keymaps(keymaps);
    crate::vc::dir::install_help_map(keymaps);
    // C-x 8 RET, C-x 8 e ..., C-x = and *Emoji* (the translations
    // themselves are iso_transl::filter's).
    crate::iso_transl::install_keymaps(keymaps);
    // C-\, C-x \, C-x RET C-\, C-h I.
    crate::input_method::install_keymaps(keymaps);
}

/// "C-x C-s" from ["C-x", "C-s"].
pub fn describe_key_sequence(keys: &[String]) -> String {
    keys.join(" ")
}

/// Describe a key press notation. `meta` is set when ESC was
/// pressed just before (ESC works as a Meta prefix).
///
/// Shift, Hyper, Super and the terminal's Meta are written when the
/// terminal can report them, which with the kitty keyboard protocol it
/// can: `C-S-a`, `H-a`, `s-super-a`, `s-m-a`. Shift on a *letter* is
/// written on the lower-case letter whichever way it arrives, so the same
/// binding works on a terminal that reports Shift as a modifier and on one
/// that folds it into the character. Shift on a *symbol* is the character
/// it makes (`S-2` is `@`), for the same reason: that character is the
/// one to insert, and a layout Forge doesn't know produces one it can't
/// fold back.
///
/// Ctrl+I and Ctrl+M come out as themselves rather than as TAB and RET,
/// which a terminal with the protocol can tell apart; `Keymaps` looks for
/// the other name when nothing is bound this way, so both keep working.
pub fn describe_key(key: &KeyEvent, meta: bool) -> Option<String> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let meta = meta || key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // Shift goes on the description of a letter and of a named key
    // ("S-a", "S-<left>"). On a symbol it has been folded into the
    // character instead (see `shifted`): that character is the one to
    // insert, and a layout-dependent key produces one Forge can't fold
    // back.
    // BackTab *is* Shift+TAB: a terminal sends it with the Shift bit set
    // too, and the key is "<backtab>", not "S-<backtab>".
    let spelled_shift = shift
        && key.code != KeyCode::BackTab
        && (matches!(key.code, KeyCode::Char(c) if c.is_ascii_alphabetic())
            || !matches!(key.code, KeyCode::Char(_)));
    let base = match key.code {
        KeyCode::Char(c) if ctrl => match c {
            // Terminals send Ctrl+Space as NUL, which can arrive as any of these.
            ' ' | '@' | '2' => "SPC".to_string(),
            // Ctrl+/ and Ctrl+_ both send 0x1F, often decoded as Ctrl+7.
            '7' | '/' => "/".to_string(),
            // Ctrl+\ sends 0x1C, often decoded as Ctrl+4.
            '4' | '\\' => "\\".to_string(),
            c => c.to_ascii_lowercase().to_string(),
        },
        KeyCode::Char(' ') => "SPC".to_string(),
        // Shift is written on the letter in lower case, the way Forge
        // writes it, whichever way the terminal sent it: one reports
        // Shift+a as 'a' with the Shift bit, the other as 'A' with it.
        KeyCode::Char(c) if shift && c.is_ascii_alphabetic() => c.to_ascii_lowercase().to_string(),
        // Shift on a symbol: what Shift makes of the key on a US layout,
        // because that is the character a terminal without the protocol
        // sends, and the one to insert.
        KeyCode::Char(c) => shifted(c)
            .filter(|_| shift)
            .map_or_else(|| c.to_string(), |s| s.to_string()),
        KeyCode::Enter if ctrl => "C-m".to_string(),
        KeyCode::Tab if ctrl => "C-i".to_string(),
        KeyCode::Enter => "RET".to_string(),
        KeyCode::Tab => "TAB".to_string(),
        KeyCode::BackTab => "<backtab>".to_string(),
        KeyCode::Backspace => "DEL".to_string(),
        KeyCode::Delete => "<deletechar>".to_string(),
        KeyCode::Esc => "ESC".to_string(),
        KeyCode::Left => "<left>".to_string(),
        KeyCode::Right => "<right>".to_string(),
        KeyCode::Up => "<up>".to_string(),
        KeyCode::Down => "<down>".to_string(),
        KeyCode::Home => "<home>".to_string(),
        KeyCode::End => "<end>".to_string(),
        KeyCode::PageUp => "<prior>".to_string(),
        KeyCode::PageDown => "<next>".to_string(),
        KeyCode::Insert => "<insert>".to_string(),
        KeyCode::F(n) => format!("<f{n}>"),
        _ => return None,
    };

    let mut s = String::new();
    if ctrl {
        s.push_str("C-");
    }
    if meta {
        s.push_str("M-");
    }
    if spelled_shift {
        s.push_str("S-");
    }
    for (flag, name) in [
        (KeyModifiers::HYPER, "H-"),
        (KeyModifiers::META, "s-m-"),
        (KeyModifiers::SUPER, "s-super-"),
    ] {
        if key.modifiers.contains(flag) {
            s.push_str(name);
        }
    }
    s.push_str(&base);
    Some(s)
}

/// A mouse button in key notation, the same numbers Forge uses:
/// "`<mouse-1>`", "`<mouse-2>`", "`<mouse-3>`". A click is dispatched as the key
/// this returns, so a binding can name it.
pub fn describe_mouse_button(button: MouseButton) -> String {
    let n = match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
    };
    format!("<mouse-{n}>")
}

/// The button a description stands for, the inverse of
/// [`describe_mouse_button`]: which button a `<mouse-1>` binding is about,
/// for a frontend that reports clicks as buttons.
pub fn mouse_button(desc: &str) -> Option<MouseButton> {
    match desc {
        "<mouse-1>" => Some(MouseButton::Left),
        "<mouse-2>" => Some(MouseButton::Middle),
        "<mouse-3>" => Some(MouseButton::Right),
        _ => None,
    }
}

/// The key event a description stands for ("C-x", "M-f", "RET", "a"):
/// the inverse of [`describe_key`], for running keys as if typed.
pub fn key_event(desc: &str) -> Option<KeyEvent> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = desc;
    loop {
        // The spellings Forge writes for a modifier, longest first, and
        // the bit each one is (see `forge_core::keymap`).
        if let Some(r) = rest.strip_prefix("s-super-").filter(|r| !r.is_empty()) {
            mods |= KeyModifiers::SUPER;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("s-m-").filter(|r| !r.is_empty()) {
            mods |= KeyModifiers::META;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("C-").filter(|r| !r.is_empty()) {
            mods |= KeyModifiers::CONTROL;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("M-").filter(|r| !r.is_empty()) {
            mods |= KeyModifiers::ALT;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("H-").filter(|r| !r.is_empty()) {
            mods |= KeyModifiers::HYPER;
            rest = r;
        } else if let Some(r) = rest
            .strip_prefix("S-")
            .or_else(|| rest.strip_prefix("s-"))
            .filter(|r| !r.is_empty())
        {
            mods |= KeyModifiers::SHIFT;
            rest = r;
        } else {
            break;
        }
    }
    let mut code = match rest {
        "SPC" => KeyCode::Char(' '),
        "RET" => KeyCode::Enter,
        "TAB" => KeyCode::Tab,
        "<backtab>" => KeyCode::BackTab,
        "DEL" => KeyCode::Backspace,
        "<deletechar>" => KeyCode::Delete,
        "ESC" => KeyCode::Esc,
        "<left>" => KeyCode::Left,
        "<right>" => KeyCode::Right,
        "<up>" => KeyCode::Up,
        "<down>" => KeyCode::Down,
        "<home>" => KeyCode::Home,
        "<end>" => KeyCode::End,
        "<prior>" => KeyCode::PageUp,
        "<next>" => KeyCode::PageDown,
        "<insert>" => KeyCode::Insert,
        f if f.starts_with("<f") && f.ends_with('>') => KeyCode::F(f[2..f.len() - 1].parse().ok()?),
        c if c.chars().count() == 1 => KeyCode::Char(c.chars().next()?),
        _ => return None,
    };
    // Shift on a letter is described on the lower-case letter, and typed
    // as the upper-case one: what the key does to the character is the
    // character, and what it does to the key is the S-.
    if mods.contains(KeyModifiers::SHIFT)
        && let KeyCode::Char(c) = code
        && c.is_ascii_lowercase()
    {
        code = KeyCode::Char(c.to_ascii_uppercase());
    }
    Some(KeyEvent::new(code, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_events_round_trip() {
        for d in [
            "C-x", "M-f", "C-M-/", "RET", "SPC", "a", "X", "<f5>", "M-DEL", "C-SPC", "<up>",
        ] {
            assert_eq!(
                describe_key(&key_event(d).unwrap(), false).as_deref(),
                Some(d),
                "{d}"
            );
        }
        assert!(key_event("nope").is_none());
        for d in [
            "S-a",
            "C-S-a",
            "H-a",
            "s-super-a",
            "s-m-a",
            "C-M-S-a",
            "S-<left>",
        ] {
            assert_eq!(
                describe_key(&key_event(d).unwrap(), false).as_deref(),
                Some(d),
                "{d}"
            );
        }
        // Shift on a symbol is spelled as the symbol, because that is what
        // a terminal without the keyboard protocol sends.
        assert_eq!(
            describe_key(&key_event("@").unwrap(), false).as_deref(),
            Some("@")
        );
        assert_eq!(
            forge_core::keymap::parse_keys("S-2").unwrap(),
            ["@".to_string()]
        );
        // A click has no key event, but it has a notation and a button.
        assert!(key_event("<mouse-1>").is_none());
        for b in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            assert_eq!(mouse_button(&describe_mouse_button(b)), Some(b));
        }
        assert_eq!(mouse_button("nope"), None);
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn describes_keys() {
        let c = KeyModifiers::CONTROL;
        let m = KeyModifiers::ALT;
        let none = KeyModifiers::NONE;
        assert_eq!(
            describe_key(&key(KeyCode::Char('x'), c), false).unwrap(),
            "C-x"
        );
        assert_eq!(
            describe_key(&key(KeyCode::Char('f'), m), false).unwrap(),
            "M-f"
        );
        assert_eq!(
            describe_key(&key(KeyCode::Char('f'), none), true).unwrap(),
            "M-f"
        );
        assert_eq!(
            describe_key(&key(KeyCode::Char(' '), c), false).unwrap(),
            "C-SPC"
        );
        assert_eq!(
            describe_key(&key(KeyCode::Char('7'), c), false).unwrap(),
            "C-/"
        );
        assert_eq!(
            describe_key(&key(KeyCode::Char('/'), c | m), false).unwrap(),
            "C-M-/"
        );
        assert_eq!(
            describe_key(&key(KeyCode::Char('<'), m | KeyModifiers::SHIFT), false).unwrap(),
            "M-<"
        );
        assert_eq!(
            describe_key(&key(KeyCode::Backspace, m), false).unwrap(),
            "M-DEL"
        );
    }

    #[test]
    fn prefix_keys() {
        use forge_core::{BufferId, Lookup};
        let mut km = Keymaps::new();
        install_defaults(&mut km);
        let active = km.active(
            BufferId(0),
            "fundamental-mode",
            &[],
            &[],
            None,
            &forge_core::keymap::Context::default(),
        );
        let seq = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(active.lookup(&seq("C-x")), Lookup::Prefix);
        assert!(
            matches!(active.lookup(&seq("C-x C-s")), Lookup::Command { command, .. } if command == "save-buffer")
        );
        assert_eq!(active.lookup(&seq("C-x z")), Lookup::Undefined);
        assert_eq!(active.lookup(&seq("M-g")), Lookup::Prefix);
    }

    #[test]
    fn shift_tab_is_backtab_whether_or_not_the_shift_bit_is_set() {
        for mods in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            let key = KeyEvent::new(KeyCode::BackTab, mods);
            assert_eq!(describe_key(&key, false).as_deref(), Some("<backtab>"));
        }
        let ctrl = KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::SHIFT | KeyModifiers::CONTROL,
        );
        assert_eq!(describe_key(&ctrl, false).as_deref(), Some("C-<backtab>"));
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(describe_key(&left, false).as_deref(), Some("S-<left>"));
    }

    #[test]
    fn every_binding_is_a_real_command() {
        let bindings = DEFAULT_BINDINGS
            .iter()
            .chain(SPECIAL_MODE_BINDINGS)
            .chain(HELP_MODE_BINDINGS)
            .chain(INFO_MODE_BINDINGS)
            .chain(IMAGE_MODE_BINDINGS)
            .chain(TUTORIAL_MODE_BINDINGS)
            .chain(PACKAGE_MENU_BINDINGS)
            .chain(DIRED_MODE_BINDINGS)
            .chain(WDIRED_MODE_BINDINGS)
            .chain(IMAGE_DIRED_BINDINGS)
            .chain(COMPILATION_MODE_BINDINGS)
            .chain(GREP_MODE_BINDINGS)
            .chain(OCCUR_MODE_BINDINGS)
            .chain(PROG_MODE_BINDINGS);
        for (keys, cmd) in bindings {
            assert!(
                crate::commands::is_command(cmd),
                "{keys} -> unknown command {cmd}"
            );
        }
    }

    #[test]
    fn every_command_has_a_doc_string() {
        for cd in crate::commands::COMMANDS {
            assert!(
                !cd.summary().is_empty(),
                "{} has no documentation",
                cd.name()
            );
        }
    }

    #[test]
    fn defaults_leave_c_c_letter_free() {
        // install_defaults binds with Origin::Package, which rejects C-c <letter>.
        install_defaults(&mut Keymaps::new());
        for (keys, _) in DEFAULT_BINDINGS.iter().chain(SPECIAL_MODE_BINDINGS) {
            assert!(!keys.starts_with("C-c"), "{keys} takes a user key");
        }
    }

    /// The keys the README's tables promise, checked against the keymaps so
    /// the README cannot drift from what the editor actually does. Cells
    /// that list several alternatives (`C-x 2 / 3 / o`) are expanded, and
    /// cells that run several independent sequences (`C-f C-b C-n C-p`) are
    /// split by walking the keymap trie. A cell whose parse does not fit
    /// those shapes is skipped; at least 20 keys must be checked so the test
    /// cannot silently stop testing anything.
    #[test]
    fn readme_key_tables_are_bound() {
        use forge_core::BufferId;
        let mut km = Keymaps::new();
        install_defaults(&mut km);
        let active = km.active(
            BufferId(0),
            "fundamental-mode",
            &[],
            &[],
            None,
            &forge_core::keymap::Context::default(),
        );
        let seq = |tokens: &[&str]| tokens.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let lookup = |tokens: &[String]| active.lookup(tokens);

        let readme = include_str!("../../../README.md");
        let mut checked = 0usize;
        for line in readme.lines() {
            let Some(cell) = readme_key_cell(line) else {
                continue;
            };
            checked += check_key_cell(&cell, &lookup, &seq);
        }
        assert!(
            checked >= 20,
            "only {checked} README keys were checked; the parser may have stopped matching"
        );
    }

    /// The key cell of a README table row, or `None` if the line is not a
    /// row whose first column looks like keys.
    fn readme_key_cell(line: &str) -> Option<String> {
        let line = line.trim();
        if !line.starts_with('|') || line.contains("---") {
            return None;
        }
        let cell = line.trim_start_matches('|').split('|').next()?.trim();
        if cell.is_empty() {
            return None;
        }
        // A qualifier like "M-p / M-n in prompts" names keys that only exist
        // in a prompt's local map; skip it.
        if cell.contains(" in ") {
            return None;
        }
        // "M-x hl-line-mode, ..." runs commands by name; the only key there
        // is a bare `M-x`.
        if cell.starts_with("M-x ") {
            return None;
        }
        let looks_like_keys = cell.contains("C-")
            || cell.contains("M-")
            || cell.contains("RET")
            || cell.contains("TAB")
            || cell.contains("DEL")
            || cell.contains("SPC");
        looks_like_keys.then(|| cell.to_string())
    }

    /// Check one key cell, returning the number of key sequences verified.
    fn check_key_cell(
        cell: &str,
        lookup: &dyn Fn(&[String]) -> forge_core::Lookup,
        seq: &dyn Fn(&[&str]) -> Vec<String>,
    ) -> usize {
        let mut checked = 0;
        let mut prev: Option<Vec<String>> = None;
        let cell = &cell.replace("\\\\", "\\");
        let normalized = cell.replace(" or ", " / ").replace(", ", " / ");
        for part in normalized.split(" / ").map(str::trim) {
            let tokens: Vec<&str> = part.split_whitespace().collect();
            if tokens.is_empty() || part.contains("..") {
                continue;
            }
            // A single short token after a longer sequence continues its
            // prefix: `C-x r k / y` means `C-x r k`, `C-x r y`.
            if tokens.len() == 1
                && tokens[0].len() <= 2
                && let Some(p) = &prev
                && p.len() >= 2
            {
                let mut cand = p[..p.len() - 1].to_vec();
                cand.push(tokens[0].to_string());
                if matches!(lookup(&cand), forge_core::Lookup::Command { .. }) {
                    checked += 1;
                    prev = Some(cand);
                    continue;
                }
            }
            // Otherwise walk the tokens, cutting a new sequence wherever the
            // keymap resolves a command.
            let mut i = 0;
            let mut ok = true;
            let mut last: Option<Vec<String>> = None;
            while i < tokens.len() {
                let mut j = i;
                let mut found = None;
                loop {
                    match lookup(&seq(&tokens[i..=j])) {
                        forge_core::Lookup::Command { .. } => {
                            found = Some(j);
                            break;
                        }
                        forge_core::Lookup::Prefix if j + 1 < tokens.len() => j += 1,
                        _ => break,
                    }
                }
                match found {
                    Some(j) => {
                        last = Some(seq(&tokens[i..=j]));
                        checked += 1;
                        i = j + 1;
                    }
                    None => {
                        // A lone token that is not bound at all is drift
                        // worth failing on; a longer run is skipped.
                        if tokens.len() == 1 {
                            panic!("README lists {part:?}, which is not bound");
                        }
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                prev = last.or(prev);
            }
        }
        checked
    }
}
