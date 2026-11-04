// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  KeyTranslator.swift
//  Forge
//
//  macOS key events to Forge keys, traditional: Control is C-, Option
//  is M- (unless turned off in Settings), Command is left to the menus,
//  except ⌘O/⌘V, which the editor view delivers itself (see EditorNSView).
//

import AppKit

enum KeyTranslator {
    /// nil: not for Forge (Command shortcuts, keys it doesn't know).
    static func translate(_ event: NSEvent, optionIsMeta: Bool) -> KeyInput? {
        let flags = event.modifierFlags
        if flags.contains(.command) {
            return nil
        }
        let control = flags.contains(.control)
        let meta = optionIsMeta && flags.contains(.option)
        let shift = flags.contains(.shift)

        if event.keyCode == 53 {
            return KeyInput(key: .escape, control: control, meta: meta, shift: shift)
        }
        if let special = event.specialKey, let key = specialKey(special) {
            return KeyInput(key: key, control: control, meta: meta, shift: shift)
        }
        let chars = (control || meta) ? event.charactersIgnoringModifiers : event.characters
        guard let chars, !chars.isEmpty else { return nil }
        return KeyInput(key: .char(c: chars), control: control, meta: meta, shift: shift)
    }

    private static func specialKey(_ k: NSEvent.SpecialKey) -> Key? {
        switch k {
        case .upArrow: return .up
        case .downArrow: return .down
        case .leftArrow: return .left
        case .rightArrow: return .right
        case .home: return .home
        case .end: return .end
        case .pageUp: return .pageUp
        case .pageDown: return .pageDown
        case .insert: return .insert
        case .deleteForward: return .delete
        // The key labelled delete is Backspace (DEL).
        case .delete, .backspace: return .backspace
        case .tab: return .tab
        case .backTab: return .backTab
        case .carriageReturn, .enter, .newline: return .enter
        default:
            let f1 = NSEvent.SpecialKey.f1.rawValue
            let n = Int(k.rawValue) - Int(f1) + 1
            return (1...35).contains(n) ? .f(n: UInt8(n)) : nil
        }
    }
}
