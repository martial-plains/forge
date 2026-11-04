// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  FrameToolbar.swift
//  Forge
//
//  The window's native tool bar (tool-bar-mode): the editor names the
//  buttons, and this puts them in the title bar as real `NSToolbarItem`s.
//  The modifier bar is part of the same bar, so it only appears while the
//  tool bar does.
//

import AppKit

/// How much of each item to show, from `tool-bar-style`.
enum ToolBarItemStyle: String, Equatable {
    case image
    case text
    case both
    case textImage = "text-image-horiz"

    init(name: String) {
        switch name {
        case "image": self = .image
        case "text": self = .text
        case "text-image-horiz": self = .textImage
        default: self = .both
        }
    }

    var displayMode: NSToolbar.DisplayMode {
        switch self {
        case .image: return .iconOnly
        case .text: return .labelOnly
        case .both, .textImage: return .iconAndLabel
        }
    }

    var showsImage: Bool { self != .text }
    var showsLabel: Bool { self != .image }
    var imageBeforeLabel: Bool { self != .textImage }
}

/// Keeps one window's `NSToolbar` in step with the frame it shows.
@MainActor
final class FrameToolbarController: NSObject, NSToolbarDelegate {
    let toolbar: NSToolbar

    private weak var model: EditorModel?

    /// What the bar is currently made of. Availability isn't here: that
    /// changes without rebuilding, through validation.
    private struct Layout: Equatable {
        struct Command: Equatable {
            let label: String
            let icon: String
            let systemIcon: String
            let systemPlatform: String
            let iconFile: String
            let command: String
            let separator: Bool
        }

        var commands: [Command]
        var modifiers: [String]
        var style: ToolBarItemStyle
    }

    private var commands: [ToolBarButton] = []
    private var modifiers: [ModifierButton] = []
    private var layout: Layout?
    private var identifiers: [NSToolbarItem.Identifier] = []
    private var modifierIdentifiers: [NSToolbarItem.Identifier: Int] = [:]
    /// The command indices in each group, and the toolbar item holding it.
    private var groups: [[Int]] = []
    private var groupIdentifiers: [NSToolbarItem.Identifier: Int] = [:]
    /// The commands shown as individual toolbar items, and their index.
    private var commandIdentifiers: [NSToolbarItem.Identifier: Int] = [:]
    private var generation = 0
    private var modifierControls: [Int: NSButton] = [:]
    /// True while `update` applies `tool-bar-style`, so the display-mode
    /// observer doesn't rebuild a second time.
    private var changingDisplayMode = false
    /// AppKit can change the display mode while the toolbar loads items;
    /// don't reenter `rebuild` from the observer.
    private var rebuilding = false

    init(model: EditorModel, frameId: UInt32) {
        self.model = model
        toolbar = NSToolbar(identifier: NSToolbar.Identifier("ForgeToolbar-\(frameId)"))
        super.init()
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.allowsUserCustomization = false
        toolbar.autosavesConfiguration = false
        toolbar.addObserver(
            self,
            forKeyPath: #keyPath(NSToolbar.displayMode),
            options: [.new],
            context: nil
        )
    }

    deinit {
        toolbar.removeObserver(self, forKeyPath: #keyPath(NSToolbar.displayMode))
    }

    override func observeValue(
        forKeyPath keyPath: String?,
        of object: Any?,
        change: [NSKeyValueChangeKey: Any]?,
        context: UnsafeMutableRawPointer?
    ) {
        guard keyPath == #keyPath(NSToolbar.displayMode), !changingDisplayMode else {
            return
        }
        rebuild()
    }

    /// Bring the bar up to date with a frame. Returns whether the window
    /// should show it: the tool bar is on and at the top. A bottom tool
    /// bar is drawn inside the window instead, where the modifier buttons
    /// join it.
    func update(with frame: Frame?) -> Bool {
        guard let frame, frame.toolBar, !frame.toolBarBottom else {
            clear()
            return false
        }

        let modifiers = frame.modifierBar ? frame.modifierButtons : []
        let next = Layout(
            commands: frame.toolBarItems.map {
                Layout.Command(
                    label: $0.label,
                    icon: $0.icon,
                    systemIcon: $0.systemIcon,
                    systemPlatform: $0.systemPlatform,
                    iconFile: $0.iconFile,
                    command: $0.command,
                    separator: $0.separator
                )
            },
            modifiers: modifiers.map(\.label),
            style: ToolBarItemStyle(name: frame.toolBarStyle)
        )
        commands = frame.toolBarItems
        self.modifiers = modifiers

        if next != layout {
            let styleChanged = layout?.style != next.style
            layout = next
            if styleChanged {
                changingDisplayMode = true
                toolbar.displayMode = next.style.displayMode
                changingDisplayMode = false
            }
            rebuild()
        } else {
            updateAvailability()
            updateModifierStates()
        }
        return true
    }

    private func clear() {
        guard layout != nil else { return }
        layout = nil
        commands = []
        modifiers = []
        identifiers = []
        modifierIdentifiers = [:]
        groups = []
        groupIdentifiers = [:]
        commandIdentifiers = [:]
        generation += 1
        modifierControls = [:]
        while !toolbar.items.isEmpty {
            toolbar.removeItem(at: 0)
        }
    }

    /// Lay the items out again. A separator in the editor's bar ends a
    /// group; each group is one toolbar item holding the buttons next to
    /// one another, so they look like one segmented control. The
    /// identifiers carry a generation so a changed bar can't reuse a
    /// stale item the toolbar has cached.
    private func rebuild() {
        guard !rebuilding else { return }
        rebuilding = true
        defer { rebuilding = false }
        generation += 1
        identifiers = []
        modifierIdentifiers = [:]
        groups = []
        groupIdentifiers = [:]
        commandIdentifiers = [:]
        modifierControls = [:]
        while !toolbar.items.isEmpty {
            toolbar.removeItem(at: 0)
        }

        var current: [Int] = []
        for (index, command) in commands.enumerated() {
            if command.separator {
                if !current.isEmpty {
                    groups.append(current)
                    current = []
                }
                continue
            }
            current.append(index)
        }
        if !current.isEmpty {
            groups.append(current)
        }

        if toolbar.displayMode == .iconAndLabel {
            for (groupIndex, group) in groups.enumerated() {
                if groupIndex > 0 {
                    identifiers.append(.space)
                }
                for index in group {
                    let identifier = NSToolbarItem.Identifier(
                        "forge.toolbar.\(generation).command.\(index)")
                    identifiers.append(identifier)
                    commandIdentifiers[identifier] = index
                }
            }
        } else {
            for groupIndex in groups.indices {
                if groupIndex > 0 {
                    identifiers.append(.space)
                }
                let identifier = NSToolbarItem.Identifier(
                    "forge.toolbar.\(generation).group.\(groupIndex)")
                identifiers.append(identifier)
                groupIdentifiers[identifier] = groupIndex
            }
        }
        if !modifiers.isEmpty, !identifiers.isEmpty {
            identifiers.append(.space)
        }
        for index in modifiers.indices {
            let identifier = NSToolbarItem.Identifier("forge.toolbar.\(generation).modifier.\(index)")
            identifiers.append(identifier)
            modifierIdentifiers[identifier] = index
        }

        for (index, identifier) in identifiers.enumerated() {
            toolbar.insertItem(withItemIdentifier: identifier, at: index)
        }
        updateAvailability()
    }

    private func updateModifierStates() {
        for (index, control) in modifierControls {
            guard index < modifiers.count else { continue }
            control.state = modifiers[index].pending ? .on : .off
        }
    }

    /// Availability can change without the bar's shape changing (a region
    /// starts or ends), so update the buttons in place.
    private func updateAvailability() {
        for item in toolbar.items {
            updateAvailability(of: item)
        }
    }

    private func updateAvailability(of item: NSToolbarItem) {
        if let group = item as? NSToolbarItemGroup,
           let groupIndex = groupIdentifiers[item.itemIdentifier],
           groupIndex < groups.count
        {
            for (offset, index) in groups[groupIndex].enumerated()
            where offset < group.subitems.count && index < commands.count
            {
                group.subitems[offset].isEnabled = commands[index].available
            }
            return
        }
        if let index = commandIdentifiers[item.itemIdentifier], index < commands.count {
            item.isEnabled = commands[index].available
        }
    }

    // MARK: - NSToolbarDelegate

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        identifiers
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        identifiers
    }

    func toolbar(
        _ toolbar: NSToolbar,
        itemForItemIdentifier itemIdentifier: NSToolbarItem.Identifier,
        willBeInsertedIntoToolbar flag: Bool
    ) -> NSToolbarItem? {
        if let groupIndex = groupIdentifiers[itemIdentifier], groupIndex < groups.count {
            return groupItem(
                groups[groupIndex],
                groupIndex: groupIndex,
                identifier: itemIdentifier
            )
        }
        if let index = commandIdentifiers[itemIdentifier], index < commands.count {
            return commandItem(commands[index], index: index, identifier: itemIdentifier)
        }
        if let index = modifierIdentifiers[itemIdentifier], index < modifiers.count {
            return modifierItem(modifiers[index], index: index, identifier: itemIdentifier)
        }
        return nil
    }

    // MARK: - Items

    /// One group of buttons, drawn by AppKit as a single segmented control.
    private func groupItem(
        _ indices: [Int],
        groupIndex: Int,
        identifier: NSToolbarItem.Identifier
    ) -> NSToolbarItem {
        let commands = indices.map { self.commands[$0] }
        let labels = commands.map(\.label)
        let group: NSToolbarItemGroup
        switch toolbar.displayMode {
        case .labelOnly:
            group = NSToolbarItemGroup(
                itemIdentifier: identifier,
                titles: labels,
                selectionMode: .momentary,
                labels: labels,
                target: self,
                action: #selector(clickGroupSegment(_:))
            )
        default:
            let images = commands.compactMap { command in
                Self.image(for: command)
            }
            if images.count == commands.count {
                group = NSToolbarItemGroup(
                    itemIdentifier: identifier,
                    images: images,
                    selectionMode: .momentary,
                    labels: labels,
                    target: self,
                    action: #selector(clickGroupSegment(_:))
                )
            } else {
                group = NSToolbarItemGroup(
                    itemIdentifier: identifier,
                    titles: labels,
                    selectionMode: .momentary,
                    labels: labels,
                    target: self,
                    action: #selector(clickGroupSegment(_:))
                )
            }
        }
        group.label = labels.joined(separator: ", ")
        group.paletteLabel = group.label
        group.toolTip = group.label
        group.controlRepresentation = .expanded
        group.autovalidates = false
        group.selectedIndex = -1
        for (offset, command) in commands.enumerated() where offset < group.subitems.count {
            group.subitems[offset].autovalidates = false
            group.subitems[offset].isEnabled = command.available
        }
        return group
    }

    /// One button on its own, used when AppKit shows icons and labels.
    private func commandItem(
        _ command: ToolBarButton,
        index: Int,
        identifier: NSToolbarItem.Identifier
    ) -> NSToolbarItem {
        let item = NSToolbarItem(itemIdentifier: identifier)
        item.label = command.label
        item.paletteLabel = command.label
        item.toolTip = command.command
        item.image = Self.image(for: command)
        item.target = self
        item.action = #selector(clickCommand(_:))
        item.tag = index
        item.autovalidates = false
        item.isEnabled = command.available
        return item
    }

    private func modifierItem(
        _ modifier: ModifierButton,
        index: Int,
        identifier: NSToolbarItem.Identifier
    ) -> NSToolbarItem {
        let item = NSToolbarItem(itemIdentifier: identifier)
        let control = NSButton(
            title: modifier.label,
            target: self,
            action: #selector(clickModifier(_:))
        )
        control.setButtonType(.pushOnPushOff)
        control.bezelStyle = .texturedRounded
        control.controlSize = .small
        control.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        control.state = modifier.pending ? .on : .off
        control.tag = index
        control.toolTip = "Apply \(modifier.name) to the next key"
        control.sizeToFit()
        item.view = control
        item.label = modifier.label
        item.paletteLabel = modifier.name
        item.toolTip = control.toolTip
        modifierControls[index] = control
        return item
    }

    // MARK: - Clicks

    @objc private func clickGroupSegment(_ sender: NSToolbarItemGroup) {
        guard let groupIndex = groupIdentifiers[sender.itemIdentifier],
              groupIndex < groups.count,
              sender.selectedIndex >= 0,
              sender.selectedIndex < groups[groupIndex].count
        else { return }
        model?.clickToolBarItem(UInt32(groups[groupIndex][sender.selectedIndex]))
    }

    @objc private func clickCommand(_ sender: NSToolbarItem) {
        model?.clickToolBarItem(UInt32(sender.tag))
    }

    @objc private func clickModifier(_ sender: NSButton) {
        model?.clickModifierButton(UInt32(sender.tag))
    }

    // MARK: - Icons

    private static var icons: [String: NSImage] = [:]
    private static var groupImages: [String: NSImage] = [:]

    /// A segment image with the icon and label drawn together. A segmented
    /// control can show an image or a label, but not both, so icon-and-label
    /// mode bakes the two into one template image.
    static func groupImage(for command: ToolBarButton, style: ToolBarItemStyle) -> NSImage? {
        let icon = command.icon.isEmpty ? command.label : command.icon
        guard !icon.isEmpty else { return nil }
        let key = "\(icon)\u{1F}\(command.label)\u{1F}\(style.rawValue)"
        if let cached = groupImages[key] { return cached }

        let iconAttributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 14),
            .foregroundColor: NSColor.black,
        ]
        let labelAttributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 12),
            .foregroundColor: NSColor.black,
        ]
        let iconText = icon as NSString
        let labelText = command.label as NSString
        let iconSize = iconText.size(withAttributes: iconAttributes)
        let labelSize = labelText.size(withAttributes: labelAttributes)
        let spacing: CGFloat = 4
        let height: CGFloat = 18
        let width = ceil(iconSize.width + spacing + labelSize.width)
        let image = NSImage(size: NSSize(width: width, height: height), flipped: false) { rect in
            let iconY = rect.midY - iconSize.height / 2
            let labelY = rect.midY - labelSize.height / 2
            if style.imageBeforeLabel {
                iconText.draw(
                    at: NSPoint(x: rect.minX, y: iconY),
                    withAttributes: iconAttributes
                )
                labelText.draw(
                    at: NSPoint(x: rect.minX + iconSize.width + spacing, y: labelY),
                    withAttributes: labelAttributes
                )
            } else {
                labelText.draw(
                    at: NSPoint(x: rect.minX, y: labelY),
                    withAttributes: labelAttributes
                )
                iconText.draw(
                    at: NSPoint(x: rect.minX + labelSize.width + spacing, y: iconY),
                    withAttributes: iconAttributes
                )
            }
            return true
        }
        image.isTemplate = true
        groupImages[key] = image
        return image
    }

    /// The button's picture, by what `icon-preference` chose: a file the
    /// user named, an SF Symbol (the native set here), or the glyph. Other
    /// platforms' icon sets (Material, Segoe Fluent, freedesktop) are named
    /// by `describe-icon` but only their own frontends can draw them, so
    /// here they fall back to the glyph.
    private static func image(for command: ToolBarButton) -> NSImage? {
        if !command.iconFile.isEmpty {
            let key = "file:" + command.iconFile
            if let cached = icons[key] { return cached }
            let path = (command.iconFile as NSString).expandingTildeInPath
            if let picture = NSImage(contentsOfFile: path) {
                picture.size = NSSize(width: 18, height: 18)
                picture.isTemplate = false
                icons[key] = picture
                return picture
            }
        }
        if command.systemPlatform == "macos", !command.systemIcon.isEmpty {
            let key = "sf:" + command.systemIcon
            if let cached = icons[key] { return cached }
            if let symbol = NSImage(
                systemSymbolName: command.systemIcon,
                accessibilityDescription: command.label
            ) {
                symbol.isTemplate = true
                icons[key] = symbol
                return symbol
            }
        }
        return image(for: command.icon.isEmpty ? command.label : command.icon)
    }

    /// The editor's icons are single characters (`↶`, `⌕`, …). Draw one as
    /// a template image so the toolbar tints it like any other glyph.
    private static func image(for icon: String) -> NSImage? {
        guard !icon.isEmpty else { return nil }
        if let cached = icons[icon] { return cached }
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { rect in
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 14),
                .foregroundColor: NSColor.black,
            ]
            let text = icon as NSString
            let size = text.size(withAttributes: attributes)
            text.draw(
                at: NSPoint(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2),
                withAttributes: attributes
            )
            return true
        }
        image.isTemplate = true
        icons[icon] = image
        return image
    }
}

