// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  KeyTranslatorTests.swift
//  ForgeCoreTests
//
//  The macOS key mapping, which has no other coverage: Control is C-,
//  Option is M- (unless turned off), Command stays with the menus, and
//  the special keys (arrows, F-keys, delete) get their Forge names.
//
//  Run with `cd apple && swift test` (no Xcode or Rust build needed).
//

import AppKit
import XCTest
@testable import ForgeCore

final class KeyTranslatorTests: XCTestCase {
    /// An NSEvent as `EditorNSView` hands one to `KeyTranslator`.
    private func event(
        _ characters: String,
        ignoringModifiers: String? = nil,
        flags: NSEvent.ModifierFlags = [],
        keyCode: UInt16 = 0
    ) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: flags,
            timestamp: 0,
            windowNumber: 0,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: ignoringModifiers ?? characters,
            isARepeat: false,
            keyCode: keyCode
        )!
    }

    /// A special key's event: AppKit reports `specialKey` from the
    /// character, so the event's `characters` is that key's code point.
    private func specialEvent(
        _ special: NSEvent.SpecialKey,
        flags: NSEvent.ModifierFlags = []
    ) -> NSEvent {
        let scalar = UnicodeScalar(UInt32(special.rawValue))!
        return event(String(scalar), flags: flags)
    }

    /// Command is the app's (Edit menu, Cmd-O, Cmd-V): never a Forge key.
    func testCommandIsLeftToTheMenus() {
        let key = event("o", flags: .command)
        XCTAssertNil(KeyTranslator.translate(key, optionIsMeta: true))
        XCTAssertNil(KeyTranslator.translate(key, optionIsMeta: false))
    }

    /// Option as Meta: Option-x is M-x, the character ignoring Option.
    func testOptionIsMetaWhenEnabled() {
        let key = event("≈", ignoringModifiers: "x", flags: .option)
        XCTAssertEqual(
            KeyTranslator.translate(key, optionIsMeta: true),
            KeyInput(key: .char(c: "x"), control: false, meta: true, shift: false)
        )
    }

    /// Option off: Option-x types its character (there is no Meta).
    func testOptionIsNotMetaWhenDisabled() {
        let key = event("≈", ignoringModifiers: "x", flags: .option)
        XCTAssertEqual(
            KeyTranslator.translate(key, optionIsMeta: false),
            KeyInput(key: .char(c: "≈"), control: false, meta: false, shift: false)
        )
    }

    /// Control names the plain character too (C-a is "a", not "\u{1}").
    func testControlNamesThePlainCharacter() {
        let key = event("a", flags: .control)
        XCTAssertEqual(
            KeyTranslator.translate(key, optionIsMeta: true),
            KeyInput(key: .char(c: "a"), control: true, meta: false, shift: false)
        )
    }

    func testShiftIsReported() {
        let key = event("A", flags: .shift)
        XCTAssertEqual(
            KeyTranslator.translate(key, optionIsMeta: true),
            KeyInput(key: .char(c: "A"), control: false, meta: false, shift: true)
        )
    }

    /// Escape is its key code (53), not the ESC character.
    func testEscape() {
        let key = event("\u{1b}", keyCode: 53)
        XCTAssertEqual(
            KeyTranslator.translate(key, optionIsMeta: true),
            KeyInput(key: .escape, control: false, meta: false, shift: false)
        )
    }

    func testArrowKeys() {
        XCTAssertEqual(
            KeyTranslator.translate(specialEvent(.upArrow), optionIsMeta: true),
            KeyInput(key: .up, control: false, meta: false, shift: false)
        )
        XCTAssertEqual(
            KeyTranslator.translate(specialEvent(.rightArrow), optionIsMeta: true),
            KeyInput(key: .right, control: false, meta: false, shift: false)
        )
    }

    func testFunctionKeys() {
        XCTAssertEqual(
            KeyTranslator.translate(specialEvent(.f1), optionIsMeta: true),
            KeyInput(key: .f(n: 1), control: false, meta: false, shift: false)
        )
        XCTAssertEqual(
            KeyTranslator.translate(specialEvent(.f12), optionIsMeta: true),
            KeyInput(key: .f(n: 12), control: false, meta: false, shift: false)
        )
    }

    /// The key labelled delete is Backspace; the forward delete is Delete.
    func testDeleteAndForwardDelete() {
        XCTAssertEqual(
            KeyTranslator.translate(specialEvent(.delete), optionIsMeta: true),
            KeyInput(key: .backspace, control: false, meta: false, shift: false)
        )
        XCTAssertEqual(
            KeyTranslator.translate(specialEvent(.deleteForward), optionIsMeta: true),
            KeyInput(key: .delete, control: false, meta: false, shift: false)
        )
    }

    /// Modifiers ride along with the special keys.
    func testSpecialKeysKeepModifiers() {
        let key = specialEvent(.upArrow, flags: [.control, .shift])
        XCTAssertEqual(
            KeyTranslator.translate(key, optionIsMeta: true),
            KeyInput(key: .up, control: true, meta: false, shift: true)
        )
    }
}
