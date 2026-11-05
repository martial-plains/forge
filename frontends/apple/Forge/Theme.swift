// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  Theme.swift
//  Forge
//
//  Colors for Forge's faces, from the system palette so light and dark
//  mode both work (the terminal does the same with its 16 colors).
//

import AppKit

struct FaceStyle {
    var foreground: NSColor = .textColor
    var background: NSColor?
    var underline: NSColor?
    var bold = false
    var italic = false
    var strike = false
}

extension NSColor {
    /// A `#rrggbb` (or `#rgb`) color, for theme overrides from the editor.
    convenience init?(hex: String) {
        var h = hex.trimmingCharacters(in: .whitespacesAndNewlines)
        guard h.removeFirst() == "#", h.allSatisfy(\.isHexDigit) else { return nil }
        if h.count == 3 {
            h = h.map { "\($0)\($0)" }.joined()
        }
        guard h.count == 6, let v = UInt32(h, radix: 16) else { return nil }
        self.init(
            red: CGFloat((v >> 16) & 0xff) / 255,
            green: CGFloat((v >> 8) & 0xff) / 255,
            blue: CGFloat(v & 0xff) / 255,
            alpha: 1
        )
    }
}

enum Theme {
    static let background = NSColor.textBackgroundColor
    static let hlLine = NSColor.unemphasizedSelectedContentBackgroundColor.withAlphaComponent(0.45)
    static let fillColumn = NSColor.separatorColor
    static let cursor = NSColor.controlAccentColor.withAlphaComponent(0.55)
    static let modeLine = NSColor.quaternaryLabelColor
    static let modeLineInactive = NSColor.quaternaryLabelColor.withAlphaComponent(0.4)

    static func syntax(_ name: String) -> FaceStyle? {
        switch name {
        case "keyword": FaceStyle(foreground: .systemPurple)
        case "string": FaceStyle(foreground: .systemGreen)
        case "escape": FaceStyle(foreground: .systemTeal, bold: true)
        case "comment": FaceStyle(foreground: .secondaryLabelColor, italic: true)
        case "function": FaceStyle(foreground: .systemBlue)
        case "type": FaceStyle(foreground: .systemBrown)
        case "constant": FaceStyle(foreground: .systemTeal)
        case "builtin": FaceStyle(foreground: .systemRed)
        case "property": FaceStyle(foreground: .systemIndigo)
        case "attribute": FaceStyle(foreground: .systemPink)
        case "link": FaceStyle(foreground: .linkColor, underline: .linkColor)
        case "org-bold": FaceStyle(bold: true)
        case "org-italic": FaceStyle(italic: true)
        case "org-underline": FaceStyle(underline: .labelColor)
        case "org-strike-through": FaceStyle(strike: true)
        default: nil
        }
    }

    /// A Dired face (the name without its "dired-" prefix).
    static func dired(_ part: String) -> FaceStyle {
        func fg(_ c: NSColor, bold: Bool = false) -> FaceStyle { FaceStyle(foreground: c, bold: bold) }
        switch part {
        case "header": return fg(.systemBlue, bold: true)
        case "column-header":
            var s = FaceStyle(bold: true)
            s.underline = .labelColor
            return s
        case "directory", "perm-dir": return fg(.systemBlue, bold: true)
        case "executable": return fg(.systemGreen, bold: true)
        case "symlink", "perm-link", "symlink-target": return fg(.systemCyan)
        case "marked": return fg(.systemYellow, bold: true)
        case "flagged": return fg(.systemRed, bold: true)
        case "perm-read": return fg(.systemYellow)
        case "perm-write": return fg(.systemRed)
        case "perm-exec": return fg(.systemGreen)
        case "dim", "tree-prefix", "temp", "git-ignored": return fg(.tertiaryLabelColor)
        case "size": return fg(.systemGreen)
        case "size-mega": return fg(.systemYellow)
        case "size-giga": return fg(.systemRed)
        case "size-huge": return fg(.systemPurple)
        case "user": return fg(.systemYellow)
        case "group": return fg(.systemPurple)
        case "date": return fg(.systemBlue)
        case "inode": return fg(.systemPurple)
        case "image": return fg(.systemPurple)
        case "video": return fg(.systemPurple, bold: true)
        case "music": return fg(.systemCyan)
        case "lossless": return fg(.systemCyan, bold: true)
        case "crypto": return fg(.systemGreen, bold: true)
        case "document": return fg(.systemGreen)
        case "compressed": return fg(.systemRed)
        case "compiled": return fg(.systemYellow)
        case "build":
            var s = fg(.systemYellow, bold: true)
            s.underline = .systemYellow
            return s
        case "source": return fg(.systemYellow, bold: true)
        case "git-new": return fg(.systemGreen)
        case "git-modified": return fg(.systemYellow)
        case "git-deleted", "git-conflicted": return fg(.systemRed, bold: true)
        case "git-renamed": return fg(.systemCyan)
        case "git-type-change": return fg(.systemPurple)
        default: return FaceStyle()
        }
    }

    /// The style of a face name from forge-ffi (`face_name` in Rust).
    static func style(face: String, syntax under: String?) -> FaceStyle {
        let base = under.flatMap(syntax) ?? FaceStyle()
        func underlined(_ color: NSColor) -> FaceStyle {
            var s = base
            s.underline = color
            return s
        }
        switch face {
        case "default": return FaceStyle()
        case "region": return FaceStyle(background: .selectedTextBackgroundColor)
        case "match", "highlight": return FaceStyle(foreground: .black, background: .findHighlightColor)
        case "symbol-highlight":
            var s = base
            s.bold = true
            s.underline = .secondaryLabelColor
            return s
        case "underline": return underlined(.labelColor)
        case "escape": return FaceStyle(foreground: .systemPurple)
        case "truncation", "border", "line-number", "whitespace": return FaceStyle(foreground: .tertiaryLabelColor)
        case "fill-column-indicator": return FaceStyle(foreground: Theme.fillColumn)
        case "fold": return FaceStyle(foreground: .systemTeal)
        case "popup": return FaceStyle(background: NSColor.windowBackgroundColor.blended(withFraction: 0.12, of: .gray) ?? .controlBackgroundColor)
        case "popup-selected": return FaceStyle(foreground: .white, background: .selectedContentBackgroundColor)
        case "popup-annotation": return FaceStyle(foreground: .secondaryLabelColor, background: NSColor.windowBackgroundColor.blended(withFraction: 0.12, of: .gray) ?? .controlBackgroundColor)
        case "popup-info": return FaceStyle(foreground: .secondaryLabelColor, background: NSColor.windowBackgroundColor.blended(withFraction: 0.12, of: .gray) ?? .controlBackgroundColor)
        case "popup-margin": return FaceStyle(foreground: .systemTeal, background: NSColor.windowBackgroundColor.blended(withFraction: 0.12, of: .gray) ?? .controlBackgroundColor)
        case "popup-match": return FaceStyle(foreground: .labelColor, background: NSColor.windowBackgroundColor.blended(withFraction: 0.12, of: .gray) ?? .controlBackgroundColor, bold: true)
        case "popup-deprecated":
            var s = FaceStyle(foreground: .tertiaryLabelColor, background: NSColor.windowBackgroundColor.blended(withFraction: 0.12, of: .gray) ?? .controlBackgroundColor)
            s.strike = true
            return s
        case "line-number-current": return FaceStyle(foreground: .secondaryLabelColor, bold: true)
        case "menu-bar": return FaceStyle(background: modeLine)
        case "tab-bar": return FaceStyle(foreground: .secondaryLabelColor, background: modeLine)
        case "tab-bar-selected": return FaceStyle(foreground: .labelColor, background: .windowBackgroundColor)
        case "menu-selected": return FaceStyle(foreground: .white, background: .selectedContentBackgroundColor)
        case "menu-disabled": return FaceStyle(foreground: .tertiaryLabelColor, background: modeLine)
        case "mode-line": return FaceStyle(background: modeLine)
        case "mode-line-inactive": return FaceStyle(foreground: .secondaryLabelColor, background: modeLineInactive)
        case "paren-match": return FaceStyle(background: NSColor.systemTeal.withAlphaComponent(0.35))
        case "paren-mismatch": return FaceStyle(background: NSColor.systemRed.withAlphaComponent(0.5))
        case "trailing-whitespace": return FaceStyle(background: NSColor.systemRed.withAlphaComponent(0.35))
        case "error", "spell-error": return underlined(.systemRed)
        case "warning": return underlined(.systemOrange)
        case "info": return underlined(.systemBlue)
        case "rainbow-unmatched": return FaceStyle(foreground: .systemRed, bold: true)
        case "rainbow-mismatched":
            var s = FaceStyle(foreground: .systemRed, bold: true)
            s.underline = .systemRed
            return s
        case let name where name.hasPrefix("rainbow-depth-"):
            // Nested delimiter colours, depth 1 to 9 (rainbow-delimiters).
            let depths: [NSColor] = [
                .systemYellow, .systemPurple, .systemTeal, .systemGreen, .systemBlue,
                .systemOrange, .systemPink, .systemIndigo, .systemMint,
            ]
            let n = Int(name.dropFirst("rainbow-depth-".count)) ?? 1
            return FaceStyle(foreground: depths[min(max(n, 1), depths.count) - 1])
        // org-modern: labels as blocks of background color.
        case "org-modern-label", "org-modern-tag", "org-modern-date-active":
            return FaceStyle(foreground: .labelColor, background: NSColor.gray.withAlphaComponent(0.25))
        case "org-modern-todo":
            return FaceStyle(foreground: .black, background: NSColor.systemRed.withAlphaComponent(0.85), bold: true)
        case "org-modern-done":
            return FaceStyle(foreground: .secondaryLabelColor, background: NSColor.gray.withAlphaComponent(0.2))
        case "org-modern-priority":
            return FaceStyle(foreground: .black, background: NSColor.systemPink.withAlphaComponent(0.85), bold: true)
        case "org-modern-time-active":
            return FaceStyle(foreground: .black, background: NSColor.gray.withAlphaComponent(0.7), bold: true)
        case "org-modern-date-inactive":
            return FaceStyle(foreground: .secondaryLabelColor, background: NSColor.gray.withAlphaComponent(0.15))
        case "org-modern-time-inactive":
            return FaceStyle(foreground: .white, background: NSColor.gray.withAlphaComponent(0.5))
        case "org-modern-progress-complete":
            return FaceStyle(foreground: .black, background: NSColor.gray.withAlphaComponent(0.7))
        case "org-modern-progress-incomplete":
            return FaceStyle(foreground: .labelColor, background: NSColor.gray.withAlphaComponent(0.2))
        case "org-modern-block-name":
            var s = FaceStyle(foreground: .tertiaryLabelColor)
            s.italic = true
            return s
        case "org-modern-symbol": return base
        case "org-modern-horizontal-rule": return FaceStyle(foreground: .tertiaryLabelColor)
        case "org-modern-target":
            return FaceStyle(foreground: .systemTeal, background: NSColor.gray.withAlphaComponent(0.25))
        // Dired follows its default colours: bold blue directories, bold green
        // executables, cyan links, yellow/red/green r/w/x, sizes shaded by
        // magnitude, blue dates, purple inodes, underlined column headers.
        // `term` cells carry their own colours: term:<fg>:<bg>:<attrs>,
        // colours as rrggbb or "-", attributes as hex bits (1 bold,
        // 2 dim, 4 italic, 8 underline, 16 reverse, 32 strike).
        case let name where name.hasPrefix("term:"):
            let parts = name.split(separator: ":", omittingEmptySubsequences: false)
            guard parts.count == 4, let attrs = Int(parts[3], radix: 16) else { return FaceStyle() }
            var fg = parts[1] == "-" ? nil : NSColor(hex: "#" + parts[1])
            var bg = parts[2] == "-" ? nil : NSColor(hex: "#" + parts[2])
            if attrs & 16 != 0 {
                let f = fg ?? .textColor, b = bg ?? .textBackgroundColor
                fg = b
                bg = f
            }
            var style = FaceStyle(foreground: fg ?? .textColor)
            style.background = bg
            style.bold = attrs & 1 != 0
            style.italic = attrs & 4 != 0
            style.strike = attrs & 32 != 0
            if attrs & 8 != 0 { style.underline = fg ?? .textColor }
            if attrs & 2 != 0 { style.foreground = style.foreground.withAlphaComponent(0.6) }
            return style
        case let name where name.hasPrefix("dired-shade:"):
            if let color = NSColor(hex: "#" + name.dropFirst("dired-shade:".count)) {
                return FaceStyle(foreground: color)
            }
            return FaceStyle()
        case let name where name.hasPrefix("dired-"):
            return dired(String(name.dropFirst("dired-".count)))
        case "grep-heading": return FaceStyle(foreground: .systemBlue, bold: true)
        case "log-view-commit-body": return FaceStyle(foreground: .secondaryLabelColor)
        case "diff-context", "vc-dir-file": return FaceStyle()
        // Version control (diff-mode, log-view, vc-dir, smerge, log-edit).
        case "diff-added", "vc-dir-status-added", "smerge-lower": return FaceStyle(foreground: .systemGreen)
        case "diff-removed", "smerge-upper": return FaceStyle(foreground: .systemRed)
        case "diff-refine-added", "smerge-refined-added": return FaceStyle(foreground: .black, background: NSColor.systemGreen.withAlphaComponent(0.6))
        case "diff-refine-removed", "smerge-refined-removed": return FaceStyle(foreground: .black, background: NSColor.systemRed.withAlphaComponent(0.6))
        case "diff-header", "diff-file-header", "log-view-message": return FaceStyle(bold: true)
        case "diff-hunk-header", "vc-dir-header": return FaceStyle(foreground: .systemTeal)
        case "diff-nonexistent", "vc-dir-status-up-to-date", "vc-dir-status-ignored", "log-edit-unknown-header", "log-edit-files":
            return FaceStyle(foreground: .tertiaryLabelColor)
        case "log-view-revision", "vc-dir-status-edited", "smerge-base": return FaceStyle(foreground: .systemOrange)
        case "log-view-author": return FaceStyle(foreground: .systemTeal)
        case "log-view-date": return FaceStyle(foreground: .systemBlue)
        case "log-view-ref", "log-edit-header", "smerge-markers": return FaceStyle(foreground: .systemPurple, bold: true)
        case "vc-dir-header-value", "vc-dir-mark-indicator": return FaceStyle(foreground: .systemOrange, bold: true)
        case "vc-dir-directory", "log-edit-summary": return FaceStyle(foreground: .systemBlue, bold: true)
        case "vc-dir-status-warning": return FaceStyle(foreground: .systemRed, bold: true)
        case "vc-dir-status-unregistered": return FaceStyle(foreground: .systemPurple)
        case "log-edit-overlong": return underlined(.systemRed)
        case let name where name.hasPrefix("vc-annotate-age-"):
            let ages: [NSColor] = [
                .systemRed, .systemOrange, .systemYellow, .systemGreen, .systemMint,
                .systemTeal, .systemCyan, .systemBlue, .systemIndigo, .systemGray,
            ]
            let n = Int(name.dropFirst("vc-annotate-age-".count)) ?? 1
            return FaceStyle(foreground: ages[min(max(n, 1), ages.count) - 1])
        default: return syntax(face) ?? FaceStyle()
        }
    }
}
