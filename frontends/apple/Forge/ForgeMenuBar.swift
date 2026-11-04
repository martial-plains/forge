// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  ForgeMenuBar.swift
//  Forge
//
//  The macOS menu bar, built from the editor's own table so both frontends
//  offer the same commands.
//

import AppKit

/// Builds and owns the app's menu bar.
@MainActor
final class ForgeMenuBar: NSObject, NSMenuDelegate {
    /// The editor the menus are read from.
    weak var model: EditorModel? {
        didSet { target.model = model }
    }

    /// Opens the Settings window, which lives outside this file.
    var showSettings: (() -> Void)?

    /// Called after the user flips Always Show Toolbar in Full Screen.
    var fullScreenToolbarChanged: (() -> Void)? {
        didSet { target.fullScreenToolbarChanged = fullScreenToolbarChanged }
    }

    /// Whether the editor's menu-bar-mode is on. When off, the app hides
    /// its native bar hides the tty one.
    private var shown = true

    /// Which editor menu an NSMenu came from, so it can be refilled on open.
    private var titles: [ObjectIdentifier: String] = [:]

    /// Which submenu an NSMenu is, so it can be refilled on open.
    private var subs: [ObjectIdentifier: (menu: String, label: String)] = [:]

    /// The one action every item sends to. The command travels in the
    /// item's `representedObject`.
    private lazy var target = MenuTarget(model: self.model, settings: { [weak self] in
        self?.showSettings?()
    })

    /// Build the whole bar and install it. Called at launch and whenever
    /// the editor's set of menu titles changes; items changing needs no
    /// rebuild, since each menu refills itself on open.
    func install() {
        titles.removeAll()
        subs.removeAll()
        NSApp.mainMenu = shown ? mainMenu() : nil
    }

    /// Follow the editor's menu-bar-mode: show the bar or take it away.
    func setShown(_ on: Bool) {
        guard on != shown else { return }
        shown = on
        install()
    }

    // MARK: - The bar

    private func mainMenu() -> NSMenu {
        let main = NSMenu()
        main.addItem(appMenuItem())
        let groups = model?.menus ?? []
        // The editor has no View menu of its own, so the app supplies one
        // for the window settings, after Edit where macOS puts it.
        let needsView = !groups.contains { $0.title == "View" }
        var viewAdded = !needsView
        for group in groups {
            main.addItem(submenuItem(for: group))
            if !viewAdded && group.title == "Edit" {
                main.addItem(viewMenuItem())
                viewAdded = true
            }
        }
        if !viewAdded {
            main.addItem(viewMenuItem())
        }
        return main
    }

    private func viewMenuItem() -> NSMenuItem {
        let item = NSMenuItem()
        item.title = "View"
        let menu = NSMenu(title: "View")
        let fullScreen = NSMenuItem(
            title: "Enter Full Screen",
            action: #selector(NSWindow.toggleFullScreen(_:)),
            keyEquivalent: "f"
        )
        fullScreen.keyEquivalentModifierMask = [.command, .control]
        menu.addItem(fullScreen)
        menu.addItem(.separator())
        // Sent to the editor view through the responder chain.
        menu.addItem(withTitle: "Zoom In", action: Selector(("zoomIn:")), keyEquivalent: "+")
        menu.addItem(withTitle: "Zoom Out", action: Selector(("zoomOut:")), keyEquivalent: "-")
        menu.addItem(withTitle: "Actual Size", action: Selector(("zoomReset:")), keyEquivalent: "0")
        menu.addItem(.separator())
        menu.addItem(fullScreenToolbarItem())
        item.submenu = menu
        return item
    }

    private func appMenuItem() -> NSMenuItem {
        let item = NSMenuItem()
        item.title = (Bundle.main.infoDictionary?["CFBundleName"] as? String) ?? "Forge"
        let menu = NSMenu(title: item.title)
        menu.addItem(withTitle: "About \(item.title)", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        let settings = NSMenuItem(
            title: "Settings…",
            action: #selector(MenuTarget.runSettings(_:)),
            keyEquivalent: ","
        )
        settings.target = target
        menu.addItem(settings)
        menu.addItem(.separator())
        menu.addItem(withTitle: "Hide \(item.title)", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit \(item.title)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        item.submenu = menu
        return item
    }

    private func submenuItem(for group: MenuGroup) -> NSMenuItem {
        let item = NSMenuItem()
        item.title = group.title
        let menu = NSMenu(title: group.title)
        menu.delegate = self
        titles[ObjectIdentifier(menu)] = group.title
        fill(menu, from: group)
        item.submenu = menu
        return item
    }

    // MARK: - Filling a menu

    private func fill(_ menu: NSMenu, from group: MenuGroup) {
        menu.removeAllItems()
        let column = keyColumn(
            labels: group.items.map(\.label),
            keys: group.items.map(\.keys)
        )
        for entry in group.items {
            if entry.separator {
                menu.addItem(.separator())
            } else if entry.submenu.isEmpty {
                menu.addItem(commandItem(for: entry, keyColumn: column))
            } else {
                menu.addItem(parentItem(for: entry, in: group.title))
            }
        }
        if group.title == "View" {
            menu.addItem(.separator())
            menu.addItem(fullScreenToolbarItem())
        }
    }

    /// View → Always Show Toolbar in Full Screen. A native setting, so it
    /// isn't part of the editor's menu table.
    private func fullScreenToolbarItem() -> NSMenuItem {
        let item = NSMenuItem(
            title: "Always Show Toolbar in Full Screen",
            action: #selector(MenuTarget.toggleFullScreenToolbar(_:)),
            keyEquivalent: ""
        )
        item.target = target
        item.state = FullScreenToolbar.alwaysShown ? .on : .off
        return item
    }

    /// One level only: submenu items are always leaves.
    private func fillSubmenu(_ menu: NSMenu, entries: [MenuSubEntry]) {
        menu.removeAllItems()
        let column = keyColumn(
            labels: entries.map(\.label),
            keys: entries.map(\.keys)
        )
        for entry in entries {
            if entry.separator {
                menu.addItem(.separator())
            } else {
                menu.addItem(commandItem(for: entry, keyColumn: column))
            }
        }
    }

    private func parentItem(for entry: MenuEntry, in menuTitle: String) -> NSMenuItem {
        let item = NSMenuItem()
        item.attributedTitle = NSAttributedString(string: entry.label)
        item.isEnabled = entry.available
        let submenu = NSMenu(title: entry.label)
        submenu.delegate = self
        subs[ObjectIdentifier(submenu)] = (menuTitle, entry.label)
        fillSubmenu(submenu, entries: entry.submenu)
        item.submenu = submenu
        return item
    }

    /// The x position the keys column starts at: the widest label, a gap,
    /// then the widest set of keys.
    private func keyColumn(labels: [String], keys: [String]) -> CGFloat {
        let label = labels.map(width(of:)).max() ?? 0
        let keys = keys.map(width(of:)).max() ?? 0
        return label + Self.columnGap + keys + Self.rightMargin
    }

    private func width(of text: String) -> CGFloat {
        NSAttributedString(
            string: text,
            attributes: [.font: NSFont.menuFont(ofSize: 0)]
        ).size().width
    }

    /// The space between a label and its keys.
    private static let columnGap: CGFloat = 20
    /// The space AppKit leaves to the right of an item's keys.
    private static let rightMargin: CGFloat = 16

    /// An item that runs a command when chosen.
    ///
    /// Keys go in the title after a tab, right-aligned by the paragraph
    /// style -- never in the item's key equivalent: a chord can't be one,
    /// and single keys would draw in AppKit's own column while chords sit
    /// in this one. One column keeps them aligned.
    private func commandItem(for entry: any MenuCommand, keyColumn: CGFloat) -> NSMenuItem {
        let item = NSMenuItem()
        item.target = target
        item.action = #selector(MenuTarget.runCommand(_:))
        item.representedObject = entry.command
        item.isEnabled = entry.available
        item.state = entry.checked ? .on : .off
        let text = entry.keys.isEmpty ? entry.label : "\(entry.label)\t\(entry.keys)"
        item.attributedTitle = rightAligned(text, keyColumn)
        return item
    }

    private func rightAligned(_ text: String, _ column: CGFloat) -> NSAttributedString {
        let style = NSMutableParagraphStyle()
        style.tabStops = [
            NSTextTab(textAlignment: .right, location: column, options: [:])
        ]
        return NSAttributedString(
            string: text,
            attributes: [.paragraphStyle: style]
        )
    }

    // MARK: - NSMenuDelegate

    /// None of these menus' items has a key equivalent (the keys are only
    /// drawn in the title), so say so. With this answered, AppKit doesn't
    /// refill the menus (`menuNeedsUpdate`) every time a key is pressed.
    func menuHasKeyEquivalent(
        _ menu: NSMenu,
        for event: NSEvent,
        target: AutoreleasingUnsafeMutablePointer<AnyObject?>,
        action: UnsafeMutablePointer<Selector?>
    ) -> Bool {
        false
    }

    /// Refill a menu as it opens, so an item's condition is answered
    /// against the buffer as it is now. The menu's data is awaited off
    /// the main thread; the menu fills in as soon as it arrives.
    func menuNeedsUpdate(_ menu: NSMenu) {
        // AppKit also calls this while it looks for a menu item matching a
        // key press. Arrow keys and the other function keys are offered to
        // the menus first, so each press refilled every menu from the editor
        // (a round trip and a rebuild per menu), which made arrow keys, and
        // Vertico with them, crawl. A key press never needs the menus filled.
        if let event = NSApp.currentEvent, event.type == .keyDown || event.type == .keyUp {
            return
        }
        if let title = titles[ObjectIdentifier(menu)] {
            Task { @MainActor [weak self] in
                guard let self else { return }
                await self.model?.refreshMenus()
                if let group = self.model?.menus.first(where: { $0.title == title }) {
                    self.fill(menu, from: group)
                }
            }
            return
        }
        if let sub = subs[ObjectIdentifier(menu)] {
            Task { @MainActor [weak self] in
                guard let self else { return }
                await self.model?.refreshMenus()
                if let entry = self.model?.menus
                    .first(where: { $0.title == sub.menu })?
                    .items.first(where: { $0.label == sub.label })
                {
                    self.fillSubmenu(menu, entries: entry.submenu)
                }
            }
        }
    }
}

/// The fields every runnable menu row shares.
protocol MenuCommand {
    var label: String { get }
    var keys: String { get }
    var command: String { get }
    var available: Bool { get }
    var checked: Bool { get }
}

extension MenuEntry: MenuCommand {}
extension MenuSubEntry: MenuCommand {}

/// The object every menu item sends its action to.
@MainActor
final class MenuTarget: NSObject {
    weak var model: EditorModel?
    private let settings: () -> Void
    var fullScreenToolbarChanged: (() -> Void)?

    init(model: EditorModel?, settings: @escaping () -> Void) {
        self.model = model
        self.settings = settings
    }

    @objc func runCommand(_ sender: NSMenuItem) {
        guard let command = sender.representedObject as? String else { return }
        model?.runCommand(command)
    }

    @objc func runSettings(_ sender: Any?) {
        settings()
    }

    @objc func toggleFullScreenToolbar(_ sender: NSMenuItem) {
        FullScreenToolbar.alwaysShown.toggle()
        sender.state = FullScreenToolbar.alwaysShown ? .on : .off
        fullScreenToolbarChanged?()
    }
}

/// Whether the toolbar (and title bar) stay on screen in full screen. Off by
/// default: they slide in when the pointer reaches the top edge.
enum FullScreenToolbar {
    private static let key = "ForgeAlwaysShowToolbarInFullScreen"

    static var alwaysShown: Bool {
        get { UserDefaults.standard.bool(forKey: key) }
        set { UserDefaults.standard.set(newValue, forKey: key) }
    }

    /// The system's full screen options, plus hiding the toolbar unless
    /// the user wants it kept.
    static func presentationOptions(
        _ proposed: NSApplication.PresentationOptions = [.fullScreen, .autoHideMenuBar, .autoHideDock]
    ) -> NSApplication.PresentationOptions {
        alwaysShown ? proposed.subtracting(.autoHideToolbar) : proposed.union(.autoHideToolbar)
    }
}
