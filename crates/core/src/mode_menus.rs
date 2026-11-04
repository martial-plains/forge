// SPDX-License-Identifier: MIT OR Apache-2.0
//! The menus major modes put in the menu bar, and the tool bars they
//! swap in, the way `easy-menu-define` menus and
//! `tool-bar-map` bindings do. Each menu shows only while its mode is
//! the buffer's major mode; the context menu's `local` entry and the
//! macOS menu bar read the same list, so a mode's commands are in the
//! same place everywhere.
//!
//! Submenus nest one level (see [`Item`]), so a mode with many commands
//! groups them the way own menus do: Dired's Mark, Flag and
//! Operate, Org's Show/Hide, Edit Structure and so on.

use crate::menu::{Item, Menu, When};
use crate::toolbar::Toolbar;

fn l(label: &str, command: &str) -> Item {
    Item::new(label, command)
}

fn sep() -> Item {
    Item::separator()
}

fn sub(label: &str, items: Vec<Item>) -> Item {
    Item::parent(label, items)
}

fn menu(title: &str, modes: &[&str], items: Vec<Item>) -> Menu {
    let when = if modes.len() == 1 {
        When::MajorMode(modes[0].into())
    } else {
        When::Any(modes.iter().map(|m| When::MajorMode((*m).into())).collect())
    };
    Menu {
        title: title.into(),
        owner: None,
        when,
        items,
    }
}

/// Every built-in major-mode menu, in the order they sit after Tools.
pub fn menus() -> Vec<Menu> {
    vec![
        dired(),
        org(),
        agenda(),
        info(),
        compilation(),
        occur(),
        packages(),
        vc_dir(),
        log_view(),
        diff(),
        annotate(),
        smerge(),
        term(),
        image(),
        few(),
        webkit(),
        text(),
        prog(),
        special(),
    ]
}

fn dired() -> Menu {
    menu(
        "Dired",
        &["dired-mode"],
        vec![
            l("Open", "dired-find-file"),
            l("Open in Other Window", "dired-find-file-other-window"),
            l("Open Alternate (Replace)", "dired-find-alternate-file"),
            l("View (Read Only)", "dired-view-file"),
            l("Display in Other Window", "dired-display-file"),
            l("Up Directory", "dired-up-directory"),
            l("Go to File...", "dired-goto-file"),
            sep(),
            sub(
                "Mark",
                vec![
                    l("Mark", "dired-mark"),
                    l("Unmark", "dired-unmark"),
                    l("Unmark Backward", "dired-unmark-backward"),
                    l("Unmark All", "dired-unmark-all-marks"),
                    l("Toggle Marks", "dired-toggle-marks"),
                    l("Change Marks...", "dired-change-marks"),
                    sep(),
                    l("Mark Files Matching Regexp...", "dired-mark-files-regexp"),
                    l(
                        "Mark Files Containing Regexp...",
                        "dired-mark-files-containing-regexp",
                    ),
                    l("Mark Extension...", "dired-mark-extension"),
                    l("Mark Directories", "dired-mark-directories"),
                    l("Mark Executables", "dired-mark-executables"),
                    l("Mark Symlinks", "dired-mark-symlinks"),
                    l("Mark Files in Subdirectory", "dired-mark-subdir-files"),
                    sep(),
                    l("Next Marked File", "dired-next-marked-file"),
                    l("Previous Marked File", "dired-prev-marked-file"),
                ],
            ),
            sub(
                "Flag for Deletion",
                vec![
                    l("Flag File", "dired-flag-file-deletion"),
                    l("Flag Backup Files", "dired-flag-backup-files"),
                    l("Flag Auto-Save Files", "dired-flag-auto-save-files"),
                    l("Flag Files Matching Regexp...", "dired-flag-files-regexp"),
                    l("Clean Directory", "dired-clean-directory"),
                    sep(),
                    l("Delete Flagged Files", "dired-do-flagged-delete"),
                ],
            ),
            sub(
                "Operate on Marked",
                vec![
                    l("Copy To...", "dired-do-copy"),
                    l("Rename To...", "dired-do-rename"),
                    l("Delete", "dired-do-delete"),
                    l("Compress", "dired-do-compress"),
                    l("Compress To...", "dired-do-compress-to"),
                    sep(),
                    l("Change Mode...", "dired-do-chmod"),
                    l("Change Owner...", "dired-do-chown"),
                    l("Change Group...", "dired-do-chgrp"),
                    l("Touch...", "dired-do-touch"),
                    sep(),
                    l("Hardlink To...", "dired-do-hardlink"),
                    l("Symlink To...", "dired-do-symlink"),
                    l("Relative Symlink To...", "dired-do-relsymlink"),
                    sep(),
                    l("Shell Command...", "dired-do-shell-command"),
                    l("Async Shell Command...", "dired-do-async-shell-command"),
                    l("Print...", "dired-do-print"),
                    sep(),
                    l("Open Marked Files", "dired-do-find-marked-files"),
                    l("Search Marked Files...", "dired-do-find-regexp"),
                    l(
                        "Search and Replace in Marked...",
                        "dired-do-find-regexp-and-replace",
                    ),
                    sep(),
                    l("Refresh Marked", "dired-do-redisplay"),
                    l("Copy Names to Kill Ring", "dired-copy-filename-as-kill"),
                ],
            ),
            sub(
                "Regexp Operate",
                vec![
                    l("Copy Regexp...", "dired-do-copy-regexp"),
                    l("Rename Regexp...", "dired-do-rename-regexp"),
                    l("Hardlink Regexp...", "dired-do-hardlink-regexp"),
                    l("Symlink Regexp...", "dired-do-symlink-regexp"),
                    l("Relative Symlink Regexp...", "dired-do-relsymlink-regexp"),
                    sep(),
                    l("Upcase File Names", "dired-upcase"),
                    l("Downcase File Names", "dired-downcase"),
                ],
            ),
            sub(
                "Subdirectories",
                vec![
                    l("Insert This Subdirectory", "dired-maybe-insert-subdir"),
                    l("Remove This Subdirectory", "dired-kill-subdir"),
                    l("Hide This Subdirectory", "dired-hide-subdir"),
                    l("Hide All Subdirectories", "dired-hide-all"),
                    sep(),
                    l("Next Subdirectory Line", "dired-next-dirline"),
                    l("Previous Subdirectory Line", "dired-prev-dirline"),
                    l("Set Tree Level...", "dired-tree-set-level"),
                    l("Remove Marked Lines", "dired-do-kill-lines"),
                ],
            ),
            sub(
                "Display",
                vec![
                    l("Revert (Refresh)", "revert-buffer"),
                    l("Toggle Sort (Name/Date)", "dired-toggle-sort"),
                    l("Toggle Directories First", "dired-toggle-dirs-first"),
                    l("Toggle ls Style", "dired-toggle-ls-style"),
                    l("Toggle Tree View", "dired-toggle-tree"),
                    l("Toggle Icons", "dired-toggle-icons"),
                    l("Toggle Colour Scale", "dired-toggle-color-scale"),
                    l("Hide Details", "dired-hide-details-mode"),
                    l("Omit Uninteresting Files", "dired-omit-mode"),
                    sep(),
                    l("Show File Type", "dired-show-file-type"),
                    l("Summary of Keys", "dired-summary"),
                ],
            ),
            sub(
                "Compare",
                vec![
                    l("Diff with Another File...", "dired-diff"),
                    l("VC Directory", "vc-dir"),
                ],
            ),
            sep(),
            l("Edit File Names (Wdired)", "wdired-change-to-wdired-mode"),
        ],
    )
}

fn org() -> Menu {
    menu(
        "Org",
        &["org-mode"],
        vec![
            sub(
                "Show/Hide",
                vec![
                    l("Cycle Visibility", "org-cycle"),
                    l("Cycle Whole Buffer", "org-global-cycle"),
                    l("Show All", "org-fold-show-all"),
                    l("Reveal Context", "org-reveal"),
                    l("Show Children", "org-show-children"),
                    l("Show Subtree", "org-show-subtree"),
                    l("Show Entry", "org-show-entry"),
                    l("Hide Subtree", "org-hide-subtree"),
                    sep(),
                    l("Sparse Tree...", "org-sparse-tree"),
                    l("Tags Sparse Tree...", "org-tags-sparse-tree"),
                    l("Match Sparse Tree...", "org-match-sparse-tree"),
                    l("Occur...", "org-occur"),
                    sep(),
                    l("Indent Mode", "org-indent-mode"),
                    l("Number Headings", "org-num-mode"),
                    l("Pretty Entities", "org-toggle-pretty-entities"),
                    l("Link Display", "org-toggle-link-display"),
                ],
            ),
            sub(
                "Edit Structure",
                vec![
                    l("Insert Heading", "org-insert-heading"),
                    l(
                        "Insert Heading After Subtree",
                        "org-insert-heading-respect-content",
                    ),
                    l("Insert TODO Heading", "org-insert-todo-heading"),
                    l("Toggle Heading", "org-toggle-heading"),
                    sep(),
                    l("Promote Heading", "org-do-promote"),
                    l("Demote Heading", "org-do-demote"),
                    l("Promote Subtree", "org-promote-subtree"),
                    l("Demote Subtree", "org-demote-subtree"),
                    l("Move Subtree Up", "org-move-subtree-up"),
                    l("Move Subtree Down", "org-move-subtree-down"),
                    sep(),
                    l("Cut Subtree", "org-cut-subtree"),
                    l("Copy Subtree", "org-copy-subtree"),
                    l("Paste Subtree", "org-paste-subtree"),
                    l(
                        "Clone Subtree with Time Shift...",
                        "org-clone-subtree-with-time-shift",
                    ),
                    l("Mark Subtree", "org-mark-subtree"),
                    l("Sort...", "org-sort"),
                    sep(),
                    l("Convert to Odd Levels", "org-convert-to-odd-levels"),
                    l(
                        "Convert to Odd-Even Levels",
                        "org-convert-to-oddeven-levels",
                    ),
                ],
            ),
            sub(
                "Navigate Headings",
                vec![
                    l("Jump...", "org-goto"),
                    l("Next Heading", "org-next-visible-heading"),
                    l("Previous Heading", "org-previous-visible-heading"),
                    l("Next Same Level", "org-forward-heading-same-level"),
                    l("Previous Same Level", "org-backward-heading-same-level"),
                    l("Up Heading", "outline-up-heading"),
                    l("Go to Sibling...", "org-goto-sibling"),
                    l("Open in Indirect Buffer", "org-tree-to-indirect-buffer"),
                ],
            ),
            sub(
                "TODO Lists",
                vec![
                    l("Rotate TODO State", "org-todo"),
                    l("All TODO Entries", "org-todo-list"),
                    l("Priority...", "org-priority"),
                    l("Set Effort...", "org-set-effort"),
                    l("Update Statistics Cookies", "org-update-statistics-cookies"),
                    l("Toggle Checkbox", "org-toggle-checkbox"),
                    l("Toggle Comment", "org-toggle-comment"),
                ],
            ),
            sub(
                "Archive",
                vec![
                    l("Archive Subtree", "org-archive-subtree-default"),
                    l("Archive to Sibling", "org-archive-to-archive-sibling"),
                    l("Toggle ARCHIVE Tag", "org-toggle-archive-tag"),
                    l("Refile...", "org-refile"),
                ],
            ),
            sub(
                "Hyperlinks",
                vec![
                    l("Insert Link...", "org-insert-link"),
                    l("Store Link", "org-store-link"),
                    l("Insert All Links", "org-insert-all-links"),
                    l("Open Link at Point", "org-open-at-point"),
                    l("Next Link", "org-next-link"),
                    l("Previous Link", "org-previous-link"),
                    l("Go Back", "org-mark-ring-goto"),
                    l("Remove Link", "org-remove-link"),
                    sep(),
                    l("Insert Footnote...", "org-footnote-action"),
                    l("Normalize Footnotes", "org-footnote-normalize"),
                ],
            ),
            sub(
                "Tags and Properties",
                vec![
                    l("Set Tags...", "org-set-tags-command"),
                    l("Toggle Tag...", "org-toggle-tag"),
                    l("Tags View...", "org-tags-view"),
                    sep(),
                    l("Set Property...", "org-set-property"),
                    l("Delete Property...", "org-delete-property"),
                    l("Insert Property Drawer", "org-insert-property-drawer"),
                    l("Insert Drawer...", "org-insert-drawer"),
                    l("Column View", "org-columns"),
                    l("Special Property Action...", "org-property-action"),
                ],
            ),
            sub(
                "Dates and Scheduling",
                vec![
                    l("Timestamp...", "org-time-stamp"),
                    l("Inactive Timestamp...", "org-time-stamp-inactive"),
                    l("Schedule...", "org-schedule"),
                    l("Deadline...", "org-deadline"),
                    sep(),
                    l("Timestamp Up", "org-timestamp-up-day"),
                    l("Timestamp Down", "org-timestamp-down-day"),
                    l("Evaluate Time Range", "org-evaluate-time-range"),
                    l("Check Deadlines...", "org-check-deadlines"),
                    l("Check Before Date...", "org-check-before-date"),
                    l("Check After Date...", "org-check-after-date"),
                ],
            ),
            sub(
                "Logging Work",
                vec![
                    l("Clock In", "org-clock-in"),
                    l("Clock Out", "org-clock-out"),
                    l("Clock In Last Task", "org-clock-in-last"),
                    l("Cancel Clock", "org-clock-cancel"),
                    l("Select Task...", "org-clock-select-task"),
                    l("Go to Clocked Task", "org-clock-goto"),
                    l("Display Times", "org-clock-display"),
                    l("Clock Report", "org-clock-report"),
                    l("Resolve Clocks", "org-resolve-clocks"),
                    sep(),
                    l("Add Note", "org-add-note"),
                    l("Timer: Start", "org-timer-start"),
                    l("Timer: Pause/Continue", "org-timer-pause-or-continue"),
                    l("Timer: Stop", "org-timer-stop"),
                    l("Timer: Set...", "org-timer-set-timer"),
                    l("Timer: Insert Item", "org-timer-item"),
                ],
            ),
            sub(
                "Table",
                vec![
                    l("Create Table...", "org-table-create"),
                    l(
                        "Convert Region to Table",
                        "org-table-create-or-convert-from-region",
                    ),
                    l("Align", "org-table-align"),
                    l("Next Field", "org-table-next-field"),
                    l("Previous Field", "org-table-previous-field"),
                    l("Next Row", "org-table-next-row"),
                    l("Blank Field", "org-table-blank-field"),
                    l("Copy Down", "org-table-copy-down"),
                    sep(),
                    l("Insert Row", "org-table-insert-row"),
                    l("Insert Column", "org-table-insert-column"),
                    l("Insert Horizontal Line", "org-table-insert-hline"),
                    l("Delete Row", "org-table-kill-row"),
                    l("Delete Column", "org-table-delete-column"),
                    l("Move Row Up", "org-table-move-row-up"),
                    l("Move Row Down", "org-table-move-row-down"),
                    l("Move Column Left", "org-table-move-column-left"),
                    l("Move Column Right", "org-table-move-column-right"),
                    l("Sort Lines...", "org-table-sort-lines"),
                    l("Transpose", "org-table-transpose-table-at-point"),
                    sep(),
                    l("Recalculate", "org-table-recalculate"),
                    l("Recalculate All", "org-table-recalculate-buffer-tables"),
                    l("Edit Formulas", "org-table-edit-formulas"),
                    l("Field Info", "org-table-field-info"),
                    l("Sum Column/Region", "org-table-sum"),
                    l("Import...", "org-table-import"),
                    l("Export...", "org-table-export"),
                    l("Header Line", "org-table-header-line-mode"),
                ],
            ),
            sub(
                "Lists",
                vec![
                    l("Insert Item", "org-list-insert-item"),
                    l("Indent Item", "org-list-indent-item-generic"),
                    l("Outdent Item", "org-list-outdent-item"),
                    l("Sort List...", "org-sort-list"),
                    l("Repair List", "org-list-repair"),
                    l("Item to Subtree", "org-list-item-to-subtree"),
                ],
            ),
            sub(
                "Source Blocks (Babel)",
                vec![
                    l("Execute Block", "org-babel-execute-src-block"),
                    l("Execute Buffer", "org-babel-execute-buffer"),
                    l("Execute Subtree", "org-babel-execute-subtree"),
                    l("Tangle", "org-babel-tangle"),
                    l("Expand Block", "org-babel-expand-src-block"),
                    l("Edit Block (Native Mode)", "org-edit-special"),
                    l("Next Block", "org-babel-next-src-block"),
                    l("Previous Block", "org-babel-previous-src-block"),
                    l("Go to Block Head", "org-babel-goto-src-block-head"),
                    l("Go to Named Block...", "org-babel-goto-named-src-block"),
                    l("Mark Block", "org-babel-mark-block"),
                    l("Split Block", "org-babel-demarcate-block"),
                    l("Remove Result", "org-babel-remove-result"),
                    l("Open Result", "org-babel-open-src-block-result"),
                    l("Insert Header Argument...", "org-babel-insert-header-arg"),
                    l("View Block Info", "org-babel-view-src-block-info"),
                    l(
                        "Insert Structure Template...",
                        "org-insert-structure-template",
                    ),
                ],
            ),
            sub(
                "Export, Publish, Capture",
                vec![
                    l("Export/Publish...", "org-export-dispatch"),
                    l("Repeat Last Export", "org-export-repeat"),
                    l("Publish Project...", "org-publish"),
                    l("Publish Current File", "org-publish-current-file"),
                    l("Publish All", "org-publish-all"),
                    sep(),
                    l("Capture...", "org-capture"),
                    l("Update All Dynamic Blocks", "org-update-all-dblocks"),
                    l("Preview LaTeX", "org-preview-latex-fragment"),
                    l("Clear LaTeX Previews", "org-clear-latex-preview"),
                    l("Encrypt Entry", "org-encrypt-entry"),
                    l("Decrypt Entry", "org-decrypt-entry"),
                ],
            ),
            sub(
                "Agenda Commands",
                vec![
                    l("Agenda Dispatcher...", "org-agenda"),
                    l("Agenda for the Week", "org-agenda-list"),
                    l("Search View...", "org-search-view"),
                    l("Stuck Projects", "org-agenda-list-stuck-projects"),
                    l("Add File to Agenda", "org-agenda-file-to-front"),
                    l("Remove File from Agenda", "org-remove-file"),
                    l("Cycle Agenda Files", "org-cycle-agenda-files"),
                ],
            ),
            sep(),
            l("Execute Context Action", "org-ctrl-c-ctrl-c"),
            l("Version", "org-version"),
        ],
    )
}

fn agenda() -> Menu {
    menu(
        "Agenda",
        &["org-agenda-mode"],
        vec![
            sub(
                "Motion",
                vec![
                    l("Next Line", "org-agenda-next-line"),
                    l("Previous Line", "org-agenda-previous-line"),
                    l("Later", "org-agenda-later"),
                    l("Earlier", "org-agenda-earlier"),
                    l("Today", "org-agenda-goto-today"),
                    l("Go to Date...", "org-agenda-goto-date"),
                ],
            ),
            sub(
                "View",
                vec![
                    l("Day", "org-agenda-day-view"),
                    l("Week", "org-agenda-week-view"),
                    l("Month", "org-agenda-month-view"),
                    l("Year", "org-agenda-year-view"),
                    l("Reset View", "org-agenda-reset-view"),
                    sep(),
                    l("Log Mode", "org-agenda-log-mode"),
                    l("Clock Report Mode", "org-agenda-clockreport-mode"),
                    l("Entry Text Mode", "org-agenda-entry-text-mode"),
                    l("Follow Mode", "org-agenda-follow-mode"),
                    l("Show Archived", "org-agenda-archives-mode"),
                    l("Show Diary", "org-agenda-toggle-diary"),
                    sep(),
                    l("Refresh", "org-agenda-redo"),
                    l("Recenter", "org-agenda-recenter"),
                ],
            ),
            sub(
                "Filter",
                vec![
                    l("By Tag...", "org-agenda-filter-by-tag"),
                    l("By Category...", "org-agenda-filter-by-category"),
                    l("By Regexp...", "org-agenda-filter-by-regexp"),
                    l("By Effort...", "org-agenda-filter-by-effort"),
                    l("By Top Headline", "org-agenda-filter-by-top-headline"),
                    l("Limit Interactively", "org-agenda-limit-interactively"),
                    l("Remove Filters", "org-agenda-filter"),
                ],
            ),
            sub(
                "Entry",
                vec![
                    l("Show", "org-agenda-show-and-scroll-up"),
                    l("Go To (Other Window)", "org-agenda-goto"),
                    l("Switch To", "org-agenda-switch-to"),
                    l(
                        "Open in Indirect Buffer",
                        "org-agenda-tree-to-indirect-buffer",
                    ),
                    sep(),
                    l("Set TODO State...", "org-agenda-todo"),
                    l("Set Tags...", "org-agenda-set-tags"),
                    l("Set Priority...", "org-agenda-priority"),
                    l("Priority Up", "org-agenda-priority-up"),
                    l("Priority Down", "org-agenda-priority-down"),
                    l("Set Effort...", "org-agenda-set-effort"),
                    l("Add Note", "org-agenda-add-note"),
                    sep(),
                    l("Schedule...", "org-agenda-schedule"),
                    l("Deadline...", "org-agenda-deadline"),
                    l("Date Later", "org-agenda-do-date-later"),
                    l("Date Earlier", "org-agenda-do-date-earlier"),
                    sep(),
                    l("Refile...", "org-agenda-refile"),
                    l("Archive", "org-agenda-archive"),
                    l("Toggle ARCHIVE Tag", "org-agenda-toggle-archive-tag"),
                    l("Capture...", "org-agenda-capture"),
                    l("Undo", "org-agenda-undo"),
                ],
            ),
            sub(
                "Clock",
                vec![
                    l("Clock In", "org-agenda-clock-in"),
                    l("Clock Out", "org-agenda-clock-out"),
                    l("Cancel Clock", "org-agenda-clock-cancel"),
                    l("Go to Clocked Task", "org-agenda-clock-goto"),
                ],
            ),
            sub(
                "Bulk Actions",
                vec![
                    l("Mark", "org-agenda-bulk-mark"),
                    l("Mark All", "org-agenda-bulk-mark-all"),
                    l("Mark Regexp...", "org-agenda-bulk-mark-regexp"),
                    l("Toggle Mark", "org-agenda-bulk-toggle"),
                    l("Toggle All Marks", "org-agenda-bulk-toggle-all"),
                    l("Unmark", "org-agenda-bulk-unmark"),
                    l("Unmark All", "org-agenda-bulk-unmark-all"),
                    l("Act on Marked...", "org-agenda-bulk-action"),
                ],
            ),
            sub(
                "Restrict",
                vec![
                    l("Lock to This Subtree", "org-agenda-set-restriction-lock"),
                    l("Remove Lock", "org-agenda-remove-restriction-lock"),
                    l("Write Agenda to File...", "org-agenda-write"),
                ],
            ),
            sep(),
            l("Quit", "org-agenda-quit"),
            l("Exit (Kill Agenda Buffers)", "org-agenda-exit"),
        ],
    )
}

fn info() -> Menu {
    menu(
        "Info",
        &["info-mode"],
        vec![
            sub(
                "Move",
                vec![
                    l("Next Node", "info-next"),
                    l("Previous Node", "info-prev"),
                    l("Up", "info-up"),
                    l("Top", "info-top"),
                    l("Forward (Reading Order)", "info-forward-node"),
                    l("Backward (Reading Order)", "info-backward-node"),
                    l("Last Visited", "info-last"),
                    l("History Forward", "info-history-forward"),
                    l("Goto Node...", "info-goto-node"),
                ],
            ),
            sub(
                "References",
                vec![
                    l("Menu Item...", "info-menu"),
                    l("Follow Reference at Point", "info-follow-reference"),
                    l("Next Reference", "info-next-reference"),
                    l("Previous Reference", "info-prev-reference"),
                ],
            ),
            sub(
                "Search",
                vec![
                    l("Search...", "info-search"),
                    l("Index...", "info-index"),
                    l("Look Up Command...", "info-lookup-command"),
                    l("Table of Contents", "info-toc"),
                ],
            ),
            sep(),
            l("Quit", "info-quit"),
        ],
    )
}

fn compilation() -> Menu {
    menu(
        "Compilation",
        &["compilation-mode"],
        vec![
            l("Next Error", "compilation-next-error"),
            l("Previous Error", "compilation-previous-error"),
            l("First Error", "first-error"),
            l("Go to Error", "compile-goto-error"),
            sep(),
            l("Recompile", "recompile"),
            l("Compile...", "compile"),
            l("Kill Compilation", "kill-compilation"),
            sep(),
            l("Toggle Read-Only", "read-only-mode"),
        ],
    )
}

fn occur() -> Menu {
    menu(
        "Grep",
        &["grep-mode", "occur-mode"],
        vec![
            l("Go to Match", "occur-mode-goto-occurrence"),
            l("Next Match", "next-error"),
            l("Previous Match", "previous-error"),
            l("First Match", "first-error"),
            sep(),
            l("Grep...", "grep"),
            l("Recursive Grep...", "rgrep"),
            l("Find Grep...", "grep-find"),
            l("Local Grep...", "lgrep"),
            l(
                "Toggle Abbreviated Command",
                "grep-find-toggle-abbreviation",
            ),
            l("Kill Grep", "kill-grep"),
            sep(),
            l("Edit Matches in Place", "grep-edit"),
            l("Finish Editing", "grep-edit-exit"),
            sep(),
            l("Occur...", "occur"),
        ],
    )
}

fn packages() -> Menu {
    menu(
        "Package",
        &["package-menu-mode"],
        vec![
            l("Install Package...", "package-install"),
            l("Install from VC...", "package-vc-install"),
            l("Delete Package...", "package-delete"),
            l("Refresh Contents", "package-refresh-contents"),
            l("Upgrade All", "package-upgrade-all"),
            l("Upgrade VC Packages", "package-vc-upgrade-all"),
            sep(),
            l("Revert List", "revert-buffer"),
        ],
    )
}

fn vc_dir() -> Menu {
    menu(
        "VC-Dir",
        &["vc-dir-mode"],
        vec![
            l("Open File", "vc-dir-find-file"),
            l("Open File in Other Window", "vc-dir-find-file-other-window"),
            sep(),
            sub(
                "Move",
                vec![
                    l("Next Line", "vc-dir-next-line"),
                    l("Previous Line", "vc-dir-previous-line"),
                    l("Next Group", "vc-dir-next-group"),
                    l("Previous Group", "vc-dir-previous-group"),
                ],
            ),
            sub(
                "Mark",
                vec![
                    l("Mark", "vc-dir-mark"),
                    l("Unmark", "vc-dir-unmark"),
                    l("Unmark Previous", "vc-dir-unmark-file-up"),
                    l("Mark All", "vc-dir-mark-all-files"),
                    l("Unmark All", "vc-dir-unmark-all-files"),
                ],
            ),
            sub(
                "Commit",
                vec![
                    l("Next Action (Check In/Out)", "vc-next-action"),
                    l("Register", "vc-register"),
                    l("Stage", "vc-dir-stage"),
                    l("Unstage", "vc-dir-unstage"),
                    l("Revert", "vc-revert"),
                    l("Delete File", "vc-delete-file"),
                    l("Ignore File", "vc-ignore"),
                    l("Rename File...", "vc-rename-file"),
                    l("Clean Untracked Files", "vc-dir-clean-files"),
                ],
            ),
            sub(
                "History",
                vec![
                    l("Compare with Base", "vc-diff"),
                    l("Compare Whole Project", "vc-root-diff"),
                    l("Compare with Branch Point", "vc-diff-mergebase"),
                    l("Show Log", "vc-print-log"),
                    l("Show Project Log", "vc-print-root-log"),
                    l("Show Incoming", "vc-log-incoming"),
                    l("Show Outgoing", "vc-log-outgoing"),
                    l("Search Log...", "vc-log-search"),
                    l("Annotate", "vc-annotate"),
                    l("Search Files (Git Grep)...", "vc-git-grep"),
                ],
            ),
            sub(
                "Remote",
                vec![
                    l("Fetch", "vc-fetch"),
                    l("Pull", "vc-pull"),
                    l("Push", "vc-push"),
                    l("Update", "vc-update"),
                    l("Merge...", "vc-merge"),
                ],
            ),
            sub(
                "Branches and Tags",
                vec![
                    l("Switch Branch...", "vc-switch-branch"),
                    l("Create Branch...", "vc-create-branch"),
                    l("Delete Branch...", "vc-delete-branch"),
                    l("Rename Branch...", "vc-rename-branch"),
                    l("List Branches", "vc-list-branches"),
                    l("Create Tag...", "vc-create-tag"),
                    l("Retrieve Tag...", "vc-retrieve-tag"),
                    l("List Tags", "vc-list-tags"),
                ],
            ),
            sub(
                "Stash",
                vec![
                    l("Stash", "vc-git-stash"),
                    l("Snapshot", "vc-git-stash-snapshot"),
                    l("List Stashes", "vc-list-stashes"),
                    l("Apply", "vc-git-stash-apply"),
                    l("Pop", "vc-git-stash-pop"),
                    l("Show", "vc-git-stash-show"),
                    l("Delete", "vc-git-stash-delete"),
                ],
            ),
            sep(),
            l("Hide Up-to-Date", "vc-dir-hide-up-to-date"),
            l("Toggle Ignored", "vc-dir-toggle-ignored"),
            l("Copy File Names", "vc-dir-copy-file-names"),
            l("Refresh", "vc-dir-refresh"),
            l("Help", "vc-dir-help"),
        ],
    )
}

fn log_view() -> Menu {
    menu(
        "Log",
        &["log-view-mode", "vc-list-mode"],
        vec![
            l("Next Entry", "log-view-next-entry"),
            l("Previous Entry", "log-view-previous-entry"),
            l("Show More", "log-view-show-more"),
            l("Refresh", "log-view-refresh"),
            l("Toggle Entry Display", "log-view-toggle-entry-display"),
            l("Toggle Mark", "log-view-toggle-mark-entry"),
            sep(),
            l("Find Revision", "log-view-find-revision"),
            l("Diff This Entry", "log-view-diff"),
            l("Diff Changeset", "log-view-diff-changeset"),
            l("Diff with Working Tree", "log-view-diff-working"),
            l("Annotate Version", "log-view-annotate-version"),
            sep(),
            l("Search Log...", "vc-log-search"),
        ],
    )
}

fn diff() -> Menu {
    menu(
        "Diff",
        &["diff-mode"],
        vec![
            sub(
                "Move",
                vec![
                    l("Next Hunk", "diff-hunk-next"),
                    l("Previous Hunk", "diff-hunk-prev"),
                    l("Next File", "diff-file-next"),
                    l("Previous File", "diff-file-prev"),
                    l("Go to Source", "diff-goto-source"),
                ],
            ),
            sub(
                "Edit",
                vec![
                    l("Apply Hunk", "diff-apply-hunk"),
                    l("Undo Apply", "diff-apply-hunk-undo"),
                    l("Kill Hunk", "diff-hunk-kill"),
                    l("Kill File", "diff-file-kill"),
                    l("Split Hunk", "diff-split-hunk"),
                    l("Refine Hunk", "diff-refine-hunk"),
                    l("Reverse Direction", "diff-reverse-direction"),
                    l("Regenerate", "diff-regenerate"),
                    l("Backup Original", "diff-backup"),
                ],
            ),
            sub(
                "View",
                vec![
                    l("Side by Side", "diff-side-by-side"),
                    l("Fold Hunk", "diff-toggle-fold"),
                    l("Fold All", "diff-toggle-fold-all"),
                    l("Toggle Whitespace", "diff-toggle-whitespace"),
                ],
            ),
        ],
    )
}

fn annotate() -> Menu {
    menu(
        "Annotate",
        &["vc-annotate-mode"],
        vec![
            l(
                "Show Log of Revision",
                "vc-annotate-show-log-revision-at-line",
            ),
            l(
                "Show Diff of Revision",
                "vc-annotate-show-diff-revision-at-line",
            ),
            l(
                "Show Changeset Diff",
                "vc-annotate-show-changeset-diff-revision-at-line",
            ),
            l(
                "Annotate Previous Revision",
                "vc-annotate-revision-previous-to-line",
            ),
            l("Annotate Revision at Line", "vc-annotate-revision-at-line"),
            l("Annotate Working Revision", "vc-annotate-working-revision"),
            l("Open Revision at Line", "vc-annotate-find-revision-at-line"),
            sep(),
            l("Next Revision", "vc-annotate-next-revision"),
            l("Previous Revision", "vc-annotate-prev-revision"),
            l("Go to Line...", "vc-annotate-goto-line"),
            l(
                "Toggle Annotation Visibility",
                "vc-annotate-toggle-annotation-visibility",
            ),
        ],
    )
}

fn smerge() -> Menu {
    let mut m = menu(
        "SMerge",
        &["smerge-mode"],
        vec![
            l("Next Conflict", "smerge-next"),
            l("Previous Conflict", "smerge-prev"),
            sep(),
            l("Keep Mine (Upper)", "smerge-keep-upper"),
            l("Keep Other (Lower)", "smerge-keep-lower"),
            l("Keep Base", "smerge-keep-base"),
            l("Keep All", "smerge-keep-all"),
            l("Keep Current", "smerge-keep-current"),
            sep(),
            l("Refine", "smerge-refine"),
            l("Ediff", "smerge-ediff"),
        ],
    );
    // smerge-mode is a minor mode.
    m.when = When::MinorMode("smerge-mode".into());
    m
}

fn term() -> Menu {
    menu(
        "Term",
        &["term-mode"],
        vec![
            l("Char Mode (Send Keys)", "term-char-mode"),
            l("Copy Mode (Edit Text)", "term-copy-mode"),
            l("Paste", "term-paste"),
            sep(),
            l("Previous Prompt", "term-previous-prompt"),
            l("Next Prompt", "term-next-prompt"),
            sep(),
            l("Interrupt (C-c)", "term-interrupt-subjob"),
            l("Kill Shell", "term-kill-subjob"),
            sep(),
            l("New Terminal", "term"),
            l("New Terminal with Shell...", "term-choose-shell"),
            l("Show Shell Integration", "term-show-integration"),
        ],
    )
}

fn image() -> Menu {
    menu(
        "Image",
        &["image-mode"],
        vec![
            sub(
                "Size",
                vec![
                    l("Fit to Window", "image-transform-fit-to-window"),
                    l("Original Size", "image-transform-reset-to-original"),
                    l("Reset to Initial", "image-transform-reset-to-initial"),
                    l("Set Scale...", "image-transform-set-scale"),
                    l("Set Percent...", "image-transform-set-percent"),
                    l("Increase", "image-increase-size"),
                    l("Decrease", "image-decrease-size"),
                ],
            ),
            sub(
                "Transform",
                vec![
                    l("Rotate", "image-rotate"),
                    l("Flip Horizontally", "image-flip-horizontally"),
                    l("Flip Vertically", "image-flip-vertically"),
                ],
            ),
            sub(
                "Animation",
                vec![
                    l("Toggle Animation", "image-toggle-animation"),
                    l("Next Frame", "image-next-frame"),
                    l("Previous Frame", "image-previous-frame"),
                    l("Go to Frame...", "image-goto-frame"),
                    l("Faster", "image-increase-speed"),
                    l("Slower", "image-decrease-speed"),
                    l("Reverse", "image-reverse-speed"),
                    l("Reset Speed", "image-reset-speed"),
                ],
            ),
            sub(
                "Files",
                vec![
                    l("Next Image File", "image-next-file"),
                    l("Previous Image File", "image-previous-file"),
                    l("Mark File", "image-mode-mark-file"),
                    l("Unmark File", "image-mode-unmark-file"),
                    l("Copy File Name", "image-mode-copy-file-name-as-kill"),
                    l("Thumbnails", "image-dired-display-thumbs"),
                ],
            ),
            sep(),
            l("Toggle Image/Text", "image-toggle-display"),
            l("Toggle Hex Display", "image-toggle-hex-display"),
            l("Save Image As...", "image-save"),
        ],
    )
}

fn few() -> Menu {
    menu(
        "Few",
        &["few-mode"],
        vec![
            l("Open URL or Search...", "few-browse-url"),
            l("Search the Web...", "few-search"),
            l("Search for Words...", "few-search-words"),
            l("Open File...", "few-open-file"),
            sep(),
            l("Back", "few-back"),
            l("Forward", "few-forward"),
            l("Reload", "few-reload"),
            l("Up", "few-up-url"),
            l("Top of Site", "few-top-url"),
            l("Next Page", "few-next-url"),
            l("Previous Page", "few-previous-url"),
            l("History", "few-list-history"),
            sep(),
            l("Next Link", "few-next-link"),
            l("Previous Link", "few-previous-link"),
            l("Follow Link", "few-follow-link"),
            l("List Links", "few-list-links"),
            l("Table of Contents", "few-toc"),
            sep(),
            l("Find in Page...", "few-find"),
            l("Find Next", "few-find-next"),
            l("Find Previous", "few-find-previous"),
            sep(),
            l("Toggle Readable View", "few-toggle-readable"),
            l("Toggle Images", "few-toggle-images"),
            l("Toggle Colors", "few-toggle-colors"),
            l("Toggle Fonts", "few-toggle-fonts"),
            l("Set Character Encoding...", "few-set-character-encoding"),
            l("View Source", "few-view-source"),
            l("Download...", "few-download"),
            l("Copy Page URL", "few-copy-url"),
            l("Copy Alternate URL", "few-copy-alternate-url"),
            l("Show Alt Text", "few-show-alt-text"),
            l("Open Picture Externally", "few-browse-image"),
            l("Open in External Browser", "few-open-external"),
            sep(),
            l("Add Bookmark", "few-add-bookmark"),
            l("List Bookmarks", "few-list-bookmarks"),
            l("Remove Bookmark...", "few-remove-bookmark"),
            l("Set Search Engine...", "few-set-search-engine"),
            sep(),
            l("Quit", "few-quit"),
        ],
    )
}

fn webkit() -> Menu {
    menu(
        "Webkit",
        &["webkit-mode", "webkit-edit-mode"],
        vec![
            sub(
                "Navigate",
                vec![
                    l("Open URL or Search...", "webkit-browse-url"),
                    l("Back", "webkit-back"),
                    l("Forward", "webkit-forward"),
                    l("Reload", "webkit-reload"),
                    l("Stop", "webkit-stop"),
                    l("Up One Level", "webkit-up-url"),
                    l("Site Root", "webkit-root-url"),
                    l("History...", "webkit-browse-history"),
                    l("Open URL from Clipboard", "webkit-open-clipboard"),
                    l("Set Search Engine...", "webkit-set-search-engine"),
                ],
            ),
            sub(
                "Pages",
                vec![
                    l("New Page...", "webkit-new"),
                    l("Clone Below", "webkit-clone-below"),
                    l("Clone on Right", "webkit-clone-right"),
                    l("List Pages", "webkit-list-pages"),
                    l("Next Page", "webkit-next-page"),
                    l("Previous Page", "webkit-previous-page"),
                    l("Reopen Closed Page", "webkit-reopen-closed"),
                    l(
                        "Open Clipboard URL in New Page",
                        "webkit-open-clipboard-new",
                    ),
                    l("Close", "webkit-quit"),
                ],
            ),
            sub(
                "Find",
                vec![
                    l("Find in Page...", "webkit-find"),
                    l("Find Next", "webkit-find-next"),
                    l("Find Previous", "webkit-find-previous"),
                ],
            ),
            sub(
                "Zoom",
                vec![
                    l("Zoom In", "webkit-zoom-in"),
                    l("Zoom Out", "webkit-zoom-out"),
                    l("Actual Size", "webkit-zoom-reset"),
                    l("Fit Width", "webkit-fit-width"),
                ],
            ),
            sub(
                "Links and Hints",
                vec![
                    l("Hints (Follow Link)", "webkit-hints"),
                    l("Hints (Copy Link)", "webkit-hints-copy"),
                    l("Hints (New Page)", "webkit-hints-new-page"),
                    l("Next Link", "webkit-next-link"),
                    l("Previous Link", "webkit-previous-link"),
                    l("Copy Link", "webkit-copy-link"),
                    l("Copy Page URL", "webkit-copy-url"),
                    l("Copy Selection", "webkit-copy-selection"),
                ],
            ),
            sub(
                "Scroll",
                vec![
                    l("Page Down", "webkit-scroll-forward"),
                    l("Page Up", "webkit-scroll-backward"),
                    l("Half Page Down", "webkit-scroll-half-down"),
                    l("Half Page Up", "webkit-scroll-half-up"),
                    l("Top", "webkit-scroll-top"),
                    l("Bottom", "webkit-scroll-bottom"),
                ],
            ),
            sub(
                "Forms",
                vec![
                    l("Focus Field", "webkit-focus-input"),
                    l("Edit Mode", "webkit-edit-mode"),
                    l("Insert Text...", "webkit-insert-string"),
                ],
            ),
            sub(
                "Developer",
                vec![
                    l("Inspect Element", "webkit-show-element"),
                    l("Show Element by ID...", "webkit-show-id-element"),
                    l("Show Element by Name...", "webkit-show-named-element"),
                ],
            ),
        ],
    )
}

fn text() -> Menu {
    menu(
        "Text",
        &["text-mode"],
        vec![
            l("Fill Paragraph", "fill-paragraph"),
            l("Fill Region", "fill-region"),
            l("Unfill Paragraph", "unfill-paragraph"),
            l("Center Line", "center-line"),
            l("Auto Fill", "auto-fill-mode"),
            l("Set Fill Column...", "set-fill-column"),
            sep(),
            l("Mark Paragraph", "mark-paragraph"),
            l("Transpose Words", "transpose-words"),
            l("Transpose Lines", "transpose-lines"),
            l("Capitalize Word", "capitalize-word"),
            l("Upcase Word", "upcase-word"),
            l("Downcase Word", "downcase-word"),
            sep(),
            l("Spell-Check Buffer", "spell-check-buffer"),
            l("Count Words", "count-words"),
            l("Insert Emoji...", "emoji-insert"),
        ],
    )
}

fn prog() -> Menu {
    menu(
        "Code",
        &["prog-mode"],
        vec![
            sub(
                "Navigate",
                vec![
                    l("Imenu", "imenu"),
                    l("Find Definition...", "lsp-find-definition"),
                    l("Find References", "lsp-references"),
                    l("Go Back", "xref-go-back"),
                    l("Go Forward", "xref-go-forward"),
                    l("Beginning of Defun", "beginning-of-defun"),
                    l("End of Defun", "end-of-defun"),
                    l("Which Function", "which-function"),
                    l("Mark Defun", "mark-defun"),
                ],
            ),
            sub(
                "Edit",
                vec![
                    l("Comment/Uncomment", "comment-dwim"),
                    l("Comment Line", "comment-line"),
                    l("Indent Region", "indent-region"),
                    l("Indent Line", "indent-for-tab-command"),
                    l("Fill Reindent Defun", "fill-reindent-defun"),
                    l("Complete at Point", "completion-at-point"),
                    l("Electric Pair", "electric-pair-mode"),
                    l("Delete Trailing Whitespace", "delete-trailing-whitespace"),
                ],
            ),
            sub(
                "Sexps",
                vec![
                    l("Forward Sexp", "forward-sexp"),
                    l("Backward Sexp", "backward-sexp"),
                    l("Kill Sexp", "kill-sexp"),
                    l("Transpose Sexps", "transpose-sexps"),
                    l("Up List", "up-list"),
                    l("Down List", "down-list"),
                ],
            ),
            sub(
                "Build",
                vec![
                    l("Compile...", "compile"),
                    l("Recompile", "recompile"),
                    l("Next Error", "next-error"),
                    l("Previous Error", "previous-error"),
                ],
            ),
        ],
    )
}

fn special() -> Menu {
    menu(
        "Buffer",
        &["special-mode", "messages-buffer-mode", "help-mode"],
        vec![
            l("Quit Window", "quit-window"),
            l("Scroll Forward", "scroll-up-command"),
            l("Scroll Backward", "scroll-down-command"),
            l("Top of Buffer", "beginning-of-buffer"),
            l("End of Buffer", "end-of-buffer"),
            sep(),
            l("Search Forward", "isearch-forward"),
            l("Occur...", "occur"),
            l("Copy", "kill-ring-save"),
            l("Toggle Read-Only", "read-only-mode"),
            sep(),
            l("Follow Link", "help-follow-link"),
            l("Next Link", "help-next-link"),
            l("Previous Link", "help-prev-link"),
            l("Revert (Refresh)", "revert-buffer"),
        ],
    )
}

// ---------------------------------------------------------------- tool bars

fn bar(items: Vec<Item2>) -> Toolbar {
    use crate::toolbar::Item as T;
    Toolbar::new(
        items
            .into_iter()
            .map(|i| match i {
                Item2::Sep => T::separator(),
                Item2::B(label, icon, cmd) => T::new(label, icon, cmd),
            })
            .collect(),
    )
}

enum Item2 {
    Sep,
    B(&'static str, &'static str, &'static str),
}
use Item2::{B, Sep};

/// The tool bars of the modes that have their own, besides the three in
/// [`crate::toolbar::Registry::builtin`].
pub fn bars() -> Vec<(&'static str, Toolbar)> {
    vec![
        (
            "webkit-mode",
            bar(vec![
                B("Back", "←", "webkit-back"),
                B("Forward", "→", "webkit-forward"),
                B("Reload", "↻", "webkit-reload"),
                B("Stop", "✕", "webkit-stop"),
                B("Home", "⌂", "webkit-root-url"),
                B("Up", "↑", "webkit-up-url"),
                Sep,
                B("Address", "⌕", "webkit-browse-url"),
                B("Find", "⌖", "webkit-find"),
                B("Hints", "⌗", "webkit-hints"),
                Sep,
                B("Zoom In", "⊕", "webkit-zoom-in"),
                B("Zoom Out", "⊖", "webkit-zoom-out"),
                B("100%", "◎", "webkit-zoom-reset"),
                Sep,
                B("New", "⊞", "webkit-new"),
                B("Pages", "▤", "webkit-list-pages"),
                B("Reopen", "↶", "webkit-reopen-closed"),
                B("History", "◷", "webkit-browse-history"),
                Sep,
                B("Copy URL", "⎘", "webkit-copy-url"),
                B("Inspect", "⌥", "webkit-show-element"),
            ]),
        ),
        (
            "few-mode",
            bar(vec![
                B("Back", "←", "few-back"),
                B("Forward", "→", "few-forward"),
                B("Reload", "↻", "few-reload"),
                B("Up", "↑", "few-up-url"),
                B("Top", "⌂", "few-top-url"),
                B("Open", "⌕", "few-browse-url"),
                Sep,
                B("Find", "⌖", "few-find"),
                B("Links", "⛓", "few-list-links"),
                B("Contents", "≡", "few-toc"),
                B("Readable", "☰", "few-toggle-readable"),
                Sep,
                B("Bookmark", "★", "few-add-bookmark"),
                B("Bookmarks", "▤", "few-list-bookmarks"),
                B("History", "◷", "few-list-history"),
                Sep,
                B("Source", "‹›", "few-view-source"),
                B("Download", "⤓", "few-download"),
                B("External", "↗", "few-open-external"),
            ]),
        ),
        (
            "image-mode",
            bar(vec![
                B("Previous", "←", "image-previous-file"),
                B("Next", "→", "image-next-file"),
                Sep,
                B("Zoom In", "⊕", "image-increase-size"),
                B("Zoom Out", "⊖", "image-decrease-size"),
                B("Fit", "⛶", "image-transform-fit-to-window"),
                B("100%", "◎", "image-transform-reset-to-original"),
                Sep,
                B("Rotate", "⟳", "image-rotate"),
                B("Flip H", "⇆", "image-flip-horizontally"),
                B("Flip V", "⇅", "image-flip-vertically"),
                Sep,
                B("Play", "▶", "image-toggle-animation"),
                B("Save", "▣", "image-save"),
            ]),
        ),
        (
            "info-mode",
            bar(vec![
                B("Back", "←", "info-last"),
                B("Forward", "→", "info-history-forward"),
                B("Up", "↑", "info-up"),
                B("Top", "⇤", "info-top"),
                Sep,
                B("Prev", "◂", "info-prev"),
                B("Next", "▸", "info-next"),
                Sep,
                B("Menu", "☰", "info-menu"),
                B("TOC", "≡", "info-toc"),
                B("Index", "⌗", "info-index"),
                B("Search", "⌕", "info-search"),
                B("Goto", "↗", "info-goto-node"),
                Sep,
                B("Quit", "✕", "info-quit"),
            ]),
        ),
        (
            "compilation-mode",
            bar(vec![
                B("Recompile", "↻", "recompile"),
                B("Kill", "■", "kill-compilation"),
                Sep,
                B("Next Error", "▼", "compilation-next-error"),
                B("Previous Error", "▲", "compilation-previous-error"),
                B("Go to Error", "↗", "compile-goto-error"),
            ]),
        ),
        (
            "grep-mode",
            bar(vec![
                B("Next Match", "▼", "next-error"),
                B("Previous Match", "▲", "previous-error"),
                B("Go to Match", "↗", "occur-mode-goto-occurrence"),
                Sep,
                B("Grep", "⌕", "grep"),
                B("Edit", "✎", "grep-edit"),
                B("Kill", "■", "kill-grep"),
            ]),
        ),
        (
            "vc-dir-mode",
            bar(vec![
                B("Open", "⊞", "vc-dir-find-file"),
                B("Refresh", "↻", "vc-dir-refresh"),
                Sep,
                B("Mark", "✓", "vc-dir-mark"),
                B("Unmark", "○", "vc-dir-unmark"),
                B("Stage", "＋", "vc-dir-stage"),
                B("Unstage", "－", "vc-dir-unstage"),
                Sep,
                B("Commit", "▣", "vc-next-action"),
                B("Diff", "±", "vc-diff"),
                B("Log", "◷", "vc-print-log"),
                Sep,
                B("Pull", "⤓", "vc-pull"),
                B("Push", "⤒", "vc-push"),
                B("Branch", "⑂", "vc-switch-branch"),
            ]),
        ),
        (
            "diff-mode",
            bar(vec![
                B("Prev Hunk", "▲", "diff-hunk-prev"),
                B("Next Hunk", "▼", "diff-hunk-next"),
                B("Source", "↗", "diff-goto-source"),
                Sep,
                B("Apply", "✓", "diff-apply-hunk"),
                B("Kill Hunk", "✕", "diff-hunk-kill"),
                B("Refine", "◈", "diff-refine-hunk"),
                B("Side by Side", "▥", "diff-side-by-side"),
            ]),
        ),
        (
            "package-menu-mode",
            bar(vec![
                B("Install", "⤓", "package-install"),
                B("Delete", "⌫", "package-delete"),
                Sep,
                B("Upgrade", "⤒", "package-upgrade-all"),
                B("Refresh", "↻", "package-refresh-contents"),
            ]),
        ),
        (
            "term-mode",
            bar(vec![
                B("New", "⊞", "term"),
                B("Shell", "▤", "term-choose-shell"),
                Sep,
                B("Copy Mode", "✎", "term-copy-mode"),
                B("Char Mode", "⌨", "term-char-mode"),
                B("Paste", "⎘", "term-paste"),
                Sep,
                B("Interrupt", "⌃", "term-interrupt-subjob"),
                B("Kill", "■", "term-kill-subjob"),
            ]),
        ),
        (
            "org-agenda-mode",
            bar(vec![
                B("Day", "①", "org-agenda-day-view"),
                B("Week", "⑦", "org-agenda-week-view"),
                B("Month", "Ⓜ", "org-agenda-month-view"),
                Sep,
                B("Earlier", "◂", "org-agenda-earlier"),
                B("Today", "◉", "org-agenda-goto-today"),
                B("Later", "▸", "org-agenda-later"),
                Sep,
                B("TODO", "☑", "org-agenda-todo"),
                B("Schedule", "◷", "org-agenda-schedule"),
                B("Deadline", "⚐", "org-agenda-deadline"),
                B("Tags", "⌗", "org-agenda-set-tags"),
                Sep,
                B("Filter", "⌕", "org-agenda-filter"),
                B("Refresh", "↻", "org-agenda-redo"),
                B("Quit", "✕", "org-agenda-quit"),
            ]),
        ),
        (
            "help-mode",
            bar(vec![
                B("Previous", "◂", "help-prev-link"),
                B("Next", "▸", "help-next-link"),
                B("Follow", "↗", "help-follow-link"),
                Sep,
                B("Search", "⌕", "isearch-forward"),
                B("Quit", "✕", "quit-window"),
            ]),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menus_have_unique_titles_and_no_empty_submenus() {
        let ms = menus();
        let mut seen = std::collections::HashSet::new();
        for m in &ms {
            assert!(seen.insert(m.title.clone()), "{} twice", m.title);
            assert!(!m.items.is_empty());
            for it in &m.items {
                if it.has_submenu() {
                    assert!(
                        it.submenu.iter().all(|c| !c.has_submenu()),
                        "{} nests deeper",
                        it.label
                    );
                } else if !it.is_separator() {
                    assert!(!it.command.is_empty());
                }
            }
        }
    }

    #[test]
    fn bars_have_one_row_of_buttons() {
        for (mode, b) in bars() {
            assert!(b.items.iter().any(|i| !i.is_separator()), "{mode} is empty");
        }
    }
}

#[cfg(test)]
mod placement {
    use super::*;
    use crate::menu::Registry;

    #[test]
    fn mode_menus_sit_before_help_and_only_show_in_their_mode() {
        let r = Registry::builtin();
        let titles: Vec<&str> = r.menus().iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles.last(), Some(&"Help"));
        assert_eq!(titles[..5], ["File", "Edit", "Options", "Buffers", "Tools"]);
        assert!(titles.contains(&"Dired") && titles.contains(&"Webkit"));
        let dired = r.menus().iter().find(|m| m.title == "Dired").unwrap();
        assert_eq!(dired.when, When::MajorMode("dired-mode".into()));
    }
}
