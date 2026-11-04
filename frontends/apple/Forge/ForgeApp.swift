// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  ForgeApp.swift
//  Forge
//
//  The macOS app: a window that draws what Forge's core describes, the same
//  editor as the terminal, from the forge-ffi Rust library.
//
//  AppKit owns the app, not a SwiftUI `App`: a SwiftUI `Scene` builds a menu
//  bar of its own and keeps rebuilding it, dropping its own items into any
//  menu whose title matches, so it and a Forge menu bar would fight over the
//  same bar. The editor itself is still SwiftUI, hosted in an `NSHostingView`.
//

import AppKit
import Combine
import SwiftUI

/// The app's entry point.
///
/// With no storyboard or SwiftUI `Scene` to create the delegate or window,
/// this creates both explicitly: the delegate is retained for the app's
/// lifetime (`NSApplication.delegate` doesn't keep it alive) and `run()`
/// drives the event loop.
@main
struct ForgeMain {
    private static var delegate: AppDelegate?

    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let delegate = AppDelegate()
        Self.delegate = delegate
        app.delegate = delegate
        app.run()
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// Created here, not by a SwiftUI `Scene`, so it exists before the menu
    /// bar is built: the menus come from it.
    private let model = EditorModel()

    private let menuBar = ForgeMenuBar()
    /// One window per frame, keyed by the frame's id.
    private var windows: [UInt32: NSWindow] = [:]
    /// The last selected frame, so a selection change can raise the right
    /// window without fighting a click.
    private var lastSelected: UInt32?
    /// True while quitting, so closing the last windows doesn't try to
    /// delete the frames underneath them.
    private var terminating = false
    private var settingsWindow: NSWindow?
    /// Shown instead of the editor when the editor couldn't start.
    private var errorWindow: NSWindow?
    private var pendingFiles: [String] = []
    private var cancellables = Set<AnyCancellable>()

    // MARK: - Launching

    func applicationWillFinishLaunching(_ notification: Notification) {
        fixAppPath()
        menuBar.model = model
        menuBar.showSettings = { [weak self] in self?.showSettings() }
        menuBar.fullScreenToolbarChanged = { [weak self] in self?.applyFullScreenToolbar() }
        model.onMenusChanged = { [weak menuBar] in menuBar?.install() }
        // The editor starts off the main thread; its menus arrive when
        // it's up, and `onMenusChanged` rebuilds the bar then.
        menuBar.install()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // The editor starts off the main thread, so a failure arrives as
        // a published `startError` and gets its window when it does.
        model.$startError
            .receive(on: DispatchQueue.main)
            .compactMap { $0 }
            .first()
            .sink { [weak self] _ in
                guard let self, self.errorWindow == nil else { return }
                self.errorWindow = self.makeWindow(for: 0)
                NSApp.activate(ignoringOtherApps: true)
            }
            .store(in: &cancellables)
        model.$frames
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in self?.reconcileFrames() }
            .store(in: &cancellables)
        model.$frameCache
            .receive(on: DispatchQueue.main)
            .sink { [weak self] cache in self?.reconcileToolbars(cache) }
            .store(in: &cancellables)
        // menu-bar-mode hides the app's own menu bar when it's turned off.
        model.$frame
            .receive(on: DispatchQueue.main)
            .sink { [weak self] frame in
                self?.menuBar.setShown(frame?.menuBar ?? true)
            }
            .store(in: &cancellables)
        NSApp.activate(ignoringOtherApps: true)
        openPendingFiles()
    }

    // MARK: - Frames and windows

    /// Make the windows match the editor's frames: a window for each new
    /// frame, close the windows of frames that are gone, and raise the
    /// selected frame's window.
    private func reconcileFrames() {
        let frames = model.frames
        // Never act on an empty frame list: it means the model hasn't
        // heard from the editor yet, not that every frame is gone.
        // Closing the last window here would quit the app out from under
        // a busy editor.
        guard !frames.isEmpty else { return }
        let live = Set(frames.map(\.id))

        for (id, window) in windows where !live.contains(id) {
            window.delegate = nil
            window.close()
            windows[id] = nil
        }
        for frame in frames where windows[frame.id] == nil {
            model.ensureFrame(frame.id)
            windows[frame.id] = makeWindow(for: frame.id)
        }
        // Raise the selected frame on a selection change (C-x 5 2, C-x 5 5,
        // closing a frame) but not on every redraw, so a click between
        // windows isn't undone here.
        if let sel = frames.first(where: { $0.selected }), lastSelected != sel.id {
            lastSelected = sel.id
            windows[sel.id]?.makeKeyAndOrderFront(nil)
        }
    }

    private func makeWindow(for frameId: UInt32) -> NSWindow {
        let window = FrameWindow(
            contentRect: NSRect(x: 0, y: 0, width: 800, height: 500),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.frameId = frameId
        window.title = model.frames.first(where: { $0.id == frameId })?.name ?? "Forge"
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.contentView = NSHostingView(rootView: RootView(model: model, frameId: frameId))
        window.minSize = NSSize(width: 400, height: 240)
        window.toolbarStyle = .unified
        let toolbar = FrameToolbarController(model: model, frameId: frameId)
        window.toolbarController = toolbar
        if toolbar.update(with: model.frame(for: frameId)) {
            window.toolbar = toolbar.toolbar
        }
        if windows.isEmpty {
            window.center()
        } else {
            // Land a new window a little off the others, like a cascade.
            let offset = CGFloat(windows.count) * 24
            window.setFrameOrigin(NSPoint(x: 100 + offset, y: 100 - offset))
        }
        window.makeKeyAndOrderFront(nil)
        return window
    }

    /// Show each window's native tool bar when its frame asks for one at
    /// the top. A bottom bar is drawn in the window's content instead.
    private func reconcileToolbars(_ cache: [UInt32: Frame]) {
        for (id, window) in windows {
            guard let window = window as? FrameWindow,
                  let controller = window.toolbarController
            else { continue }
            if controller.update(with: cache[id]) {
                if window.toolbar == nil {
                    window.toolbar = controller.toolbar
                }
            } else if window.toolbar != nil {
                window.toolbar = nil
            }
        }
    }

    // MARK: - Files

    func application(_ sender: NSApplication, openFiles filenames: [String]) {
        pendingFiles.append(contentsOf: filenames)
        openPendingFiles()
        sender.reply(toOpenOrPrint: .success)
    }

    func openPendingFiles() {
        guard !pendingFiles.isEmpty else { return }
        let files = pendingFiles
        pendingFiles.removeAll()
        for path in files {
            model.open(path: path)
        }
        NSApp.activate(ignoringOtherApps: true)
    }

    // MARK: - Settings

    /// The Settings window, opened from the Forge menu: a SwiftUI view in an
    /// AppKit window, since there's no `Settings` scene here to hold it.
    func showSettings() {
        if let settingsWindow {
            settingsWindow.makeKeyAndOrderFront(nil)
            return
        }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 380, height: 140),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        window.title = "Settings"
        window.isReleasedWhenClosed = false
        window.contentView = NSHostingView(rootView: SettingsView())
        window.center()
        window.makeKeyAndOrderFront(nil)
        settingsWindow = window
    }

    // MARK: - Ending

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func applicationWillTerminate(_ notification: Notification) {
        terminating = true
        model.saveSession()
    }
}

/// A window that knows which frame it shows, so closing it can delete that
/// frame in the editor.
@MainActor
final class FrameWindow: NSWindow {
    var frameId: UInt32 = 0
    /// Keeps the window's native tool bar in step with its frame.
    var toolbarController: FrameToolbarController?
}

extension AppDelegate: NSWindowDelegate {
    /// Full screen hides the toolbar and title bar until the pointer
    /// reaches the top edge, unless View → Always Show Toolbar in Full
    /// Screen is on.
    func window(
        _ window: NSWindow,
        willUseFullScreenPresentationOptions proposedOptions: NSApplication.PresentationOptions
    ) -> NSApplication.PresentationOptions {
        FullScreenToolbar.presentationOptions(proposedOptions)
    }

    /// The menu item was flipped: apply it to windows already in full screen.
    func applyFullScreenToolbar() {
        guard windows.values.contains(where: { $0.styleMask.contains(.fullScreen) }) else { return }
        NSApp.presentationOptions = FullScreenToolbar.presentationOptions()
    }

    /// The user clicked a window's close button: delete that frame. The
    /// editor refuses to delete the last frame, and a closed last window
    /// quits the app.
    func windowWillClose(_ notification: Notification) {
        guard !terminating else { return }
        guard let window = notification.object as? FrameWindow else { return }
        let id = window.frameId
        windows[id] = nil
        Task { @MainActor [weak self] in
            guard let self else { return }
            guard await self.model.closeFrame(id) else { return }
            self.model.refresh(full: true)
            self.reconcileFrames()
        }
    }
}

/// One window's contents: the editor, or the reason there isn't one.
struct RootView: View {
    @ObservedObject var model: EditorModel
    let frameId: UInt32

    var body: some View {
        Group {
            if let crash = model.crash {
                CrashView(model: model, message: crash)
            } else if let error = model.startError {
                Text("Forge couldn't start: \(error)")
                    .padding()
            } else {
                VStack(spacing: 0) {
                    if let frame = model.frame(for: frameId) {
                        if frame.tabBar, !frame.tabs.isEmpty {
                            NativeTabView(model: model, frameId: frameId, tabs: frame.tabs)
                        } else {
                            EditorView(model: model, frameId: frameId)
                        }
                        if frame.toolBar && frame.toolBarBottom {
                            NativeToolBar(
                                model: model,
                                items: frame.toolBarItems,
                                style: frame.toolBarStyle,
                                modifiers: frame.modifierBar ? frame.modifierButtons : []
                            )
                        }
                    } else {
                        EditorView(model: model, frameId: frameId)
                    }
                }
            }
        }
        .frame(minWidth: 400, minHeight: 240)
    }
}

/// Shown when the editor thread panicked: what went wrong and where the
/// log is, so a crash is visible (with a message to report) rather than a
/// window that quietly stops responding.
struct CrashView: View {
    @ObservedObject var model: EditorModel
    let message: String

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 40))
                .foregroundStyle(.yellow)
            Text("Forge's editor stopped")
                .font(.title2).bold()
            Text(message)
                .font(.system(.body, design: .monospaced))
                .textSelection(.enabled)
                .multilineTextAlignment(.center)
            if !model.logPath.isEmpty {
                Text(model.logPath)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                HStack {
                    Button("Copy Log Path") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(model.logPath, forType: .string)
                    }
                    Button("Reveal in Finder") {
                        NSWorkspace.shared.selectFile(
                            model.logPath, inFileViewerRootedAtPath: "")
                    }
                }
            }
        }
        .padding(40)
        .frame(minWidth: 400, minHeight: 240)
    }
}

/// The tool bar when `tool-bar-position` puts it at the bottom of the
/// window, where AppKit's native bar can't go. The modifier bar joins it,
/// the same as it joins the native bar at the top.
struct NativeToolBar: View {
    @ObservedObject var model: EditorModel
    let items: [ToolBarButton]
    let style: String
    var modifiers: [ModifierButton] = []

    private var itemStyle: ToolBarItemStyle {
        ToolBarItemStyle(name: style)
    }

    private var groupedItems: [[(offset: Int, element: ToolBarButton)]] {
        var groups: [[(offset: Int, element: ToolBarButton)]] = []
        var current: [(offset: Int, element: ToolBarButton)] = []
        for (index, item) in items.enumerated() {
            if item.separator {
                if !current.isEmpty {
                    groups.append(current)
                    current = []
                }
            } else {
                current.append((index, item))
            }
        }
        if !current.isEmpty {
            groups.append(current)
        }
        return groups
    }

    var body: some View {
        HStack(spacing: 2) {
            ForEach(Array(groupedItems.enumerated()), id: \.offset) { _, group in
                ControlGroup {
                    ForEach(group, id: \.offset) { index, item in
                        Button(action: {
                            model.clickToolBarItem(UInt32(index))
                        }) {
                            if itemStyle.showsImage && itemStyle.showsLabel,
                               let image = FrameToolbarController.groupImage(
                                   for: item, style: itemStyle)
                            {
                                Image(nsImage: image)
                            } else {
                                HStack(spacing: 3) {
                                    if itemStyle.imageBeforeLabel {
                                        icon(item)
                                        label(item)
                                    } else {
                                        label(item)
                                        icon(item)
                                    }
                                }
                            }
                        }
                        .disabled(!item.available)
                        .help(item.command)
                    }
                }
                .controlSize(.small)
            }
            if !modifiers.isEmpty {
                Divider().frame(height: 18)
                ForEach(Array(modifiers.enumerated()), id: \.offset) { index, button in
                    Button(button.label) {
                        model.clickModifierButton(UInt32(index))
                    }
                    .buttonStyle(.bordered)
                    .background(
                        button.pending ? Color.accentColor.opacity(0.35) : Color.clear,
                        in: RoundedRectangle(cornerRadius: 5)
                    )
                    .controlSize(.small)
                    .help("Apply \(button.name) to the next key")
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(.bar)
    }

    @ViewBuilder
    private func icon(_ item: ToolBarButton) -> some View {
        if itemStyle.showsImage, !item.icon.isEmpty {
            Text(item.icon)
        } else if itemStyle.showsImage, !itemStyle.showsLabel {
            Text(item.label)
        }
    }

    @ViewBuilder
    private func label(_ item: ToolBarButton) -> some View {
        if itemStyle.showsLabel {
            Text(item.label)
        }
    }
}

/// The macOS tab bar: a Liquid Glass strip holding equal-width tabs, the
/// selected one raised as an inner glass pill, with a round "new tab"
/// button beside it. Tabs can be dragged to reorder them. The core renders
/// only the selected tab's content, so a single `EditorView` sits under the
/// bar and the other tabs are empty.
struct NativeTabView: View {
    @ObservedObject var model: EditorModel
    let frameId: UInt32
    let tabs: [EditorTab]

    /// A tab being dragged: where it started and how far it has moved.
    private struct TabDrag {
        let id: UInt32
        let from: Int
        var dx: CGFloat
    }

    @State private var drag: TabDrag?
    /// The order the user just dropped the tabs in, shown until the core
    /// reports the same order back so the tabs don't jump in between.
    @State private var droppedOrder: [UInt32]?

    private let stripHeight: CGFloat = 26
    private let tabGap: CGFloat = 2
    private let stripInset: CGFloat = 2

    var body: some View {
        VStack(spacing: 0) {
            GlassEffectContainer(spacing: 8) {
                HStack(spacing: 8) {
                    strip
                    newTabButton
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(Color(nsColor: Theme.background))

            EditorView(model: model, frameId: frameId)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .onChange(of: tabs.map(\.id)) {
            droppedOrder = nil
        }
    }

    /// The tabs in the order to draw them: the dropped order until the core
    /// catches up, otherwise the core's.
    private var orderedTabs: [EditorTab] {
        guard let droppedOrder else { return tabs }
        let byId = Dictionary(uniqueKeysWithValues: tabs.map { ($0.id, $0) })
        let ordered = droppedOrder.compactMap { byId[$0] }
        return ordered.count == tabs.count ? ordered : tabs
    }

    private var strip: some View {
        GeometryReader { geo in
            let count = max(orderedTabs.count, 1)
            // How far one tab is from the next, gap included.
            let pitch = (geo.size.width - 2 * stripInset + tabGap) / CGFloat(count)

            HStack(spacing: tabGap) {
                ForEach(Array(orderedTabs.enumerated()), id: \.element.id) { index, tab in
                    let shift = xOffset(of: tab.id, at: index, count: count, pitch: pitch)
                    let dragged = drag?.id == tab.id
                    TabBarItem(
                        tab: tab,
                        onSelect: {
                            if !tab.selected {
                                model.selectTab(frameId: frameId, tab: tab.id)
                            }
                        },
                        onClose: { model.closeTab(frameId: frameId, tab: tab.id) }
                    )
                    .frame(maxWidth: .infinity)
                    .offset(x: shift)
                    .animation(dragged ? nil : .snappy(duration: 0.2), value: shift)
                    .zIndex(dragged ? 1 : 0)
                    .gesture(reorderGesture(tab: tab, index: index, count: count, pitch: pitch))
                }
            }
            .padding(stripInset)
            .frame(width: geo.size.width, height: geo.size.height)
            .glassEffect(.regular, in: Capsule())
        }
        .frame(height: stripHeight)
    }

    /// Where a tab is drawn relative to its slot while another is dragged:
    /// the dragged tab follows the pointer, and the ones it passes over slide
    /// aside to leave a gap.
    private func xOffset(of id: UInt32, at index: Int, count: Int, pitch: CGFloat) -> CGFloat {
        guard let drag else { return 0 }
        if drag.id == id {
            let low = -CGFloat(drag.from) * pitch
            let high = CGFloat(count - 1 - drag.from) * pitch
            return min(max(drag.dx, low), high)
        }
        let target = targetIndex(drag, count: count, pitch: pitch)
        if drag.from < target, index > drag.from, index <= target { return -pitch }
        if drag.from > target, index >= target, index < drag.from { return pitch }
        return 0
    }

    private func targetIndex(_ drag: TabDrag, count: Int, pitch: CGFloat) -> Int {
        let moved = Int((drag.dx / pitch).rounded())
        return min(max(drag.from + moved, 0), count - 1)
    }

    private func reorderGesture(tab: EditorTab, index: Int, count: Int, pitch: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 4)
            .onChanged { value in
                if drag == nil {
                    // Like Safari, picking a tab up selects it.
                    if !tab.selected {
                        model.selectTab(frameId: frameId, tab: tab.id)
                    }
                    drag = TabDrag(id: tab.id, from: index, dx: 0)
                }
                drag?.dx = value.translation.width
            }
            .onEnded { value in
                guard let current = drag else { return }
                var finished = current
                finished.dx = value.translation.width
                let target = targetIndex(finished, count: count, pitch: pitch)

                // Drop into the new slot with no animation: the tabs are
                // already drawn where they will end up.
                var transaction = Transaction()
                transaction.disablesAnimations = true
                withTransaction(transaction) {
                    if target != current.from {
                        var ids = orderedTabs.map(\.id)
                        ids.insert(ids.remove(at: current.from), at: target)
                        droppedOrder = ids
                        model.moveTab(frameId: frameId, tab: tab.id, to: target)
                        // If the core refuses, fall back to its order.
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                            droppedOrder = nil
                        }
                    }
                    drag = nil
                }
            }
    }

    private var newTabButton: some View {
        Button {
            model.newTab(frameId: frameId)
        } label: {
            Image(systemName: "plus")
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(.primary)
                .frame(width: stripHeight, height: stripHeight)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: Circle())
        .help("New Tab")
    }
}

/// One tab of `NativeTabView`. It's its own view so it can track hover.
private struct TabBarItem: View {
    let tab: EditorTab
    let onSelect: () -> Void
    let onClose: () -> Void

    @State private var hovering = false

    var body: some View {
        ZStack {
            if tab.selected {
                // The raised tab: a brighter pill of glass in the strip.
                Capsule()
                    .fill(Color.clear)
                    .glassEffect(
                        .regular.tint(Color(nsColor: .labelColor).opacity(0.12)),
                        in: Capsule()
                    )
            } else if hovering {
                Capsule().fill(Color(nsColor: .labelColor).opacity(0.06))
            }

            Text(tab.name)
                .font(.system(size: 12))
                .foregroundStyle(tab.selected ? Color.primary : Color.secondary)
                .lineLimit(1)
                .truncationMode(.tail)
                // Room on both sides so the centered title never runs under
                // the close button.
                .padding(.horizontal, 36)

            HStack(spacing: 0) {
                closeButton
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10)
        }
        .contentShape(Capsule())
        .onTapGesture(perform: onSelect)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }

    private var closeButton: some View {
        Button(action: onClose) {
            Image(systemName: "xmark")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(.secondary)
                .frame(width: 16, height: 16)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .opacity(hovering ? 1 : 0)
        .allowsHitTesting(hovering)
        .help("Close tab")
    }
}

/// The app's settings. Nothing here needs a `Scene`: the delegate makes the
/// window.
struct SettingsView: View {
    @AppStorage(EditorModel.optionIsMetaKey) private var optionIsMeta = true

    var body: some View {
        Form {
            Toggle("Use Option as Meta", isOn: $optionIsMeta)
            Text("Off: Option types special characters (é, ü, …); use ESC as Meta.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding()
        .frame(width: 380)
    }
}

/// Give the app the user's shell PATH, the way a terminal-launched one would
/// have it, so plugins can find `rg`, `git` and the like.
private func fixAppPath() {
    var path = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

    // Read the login shell's PATH between markers, so banner text from rc
    // files can't end up in it. Read before waiting (a full pipe would block
    // the child), and give up after two seconds.
    let task = Process()
    let pipe = Pipe()
    task.executableURL = URL(fileURLWithPath: "/bin/zsh")
    task.arguments = ["-l", "-c", "echo __FORGE_PATH__$PATH__FORGE_PATH__"]
    task.standardOutput = pipe
    task.standardError = FileHandle.nullDevice
    task.standardInput = FileHandle.nullDevice

    do {
        try task.run()
        var data = Data()
        let reader = DispatchGroup()
        reader.enter()
        DispatchQueue.global().async {
            data = pipe.fileHandleForReading.readDataToEndOfFile()
            reader.leave()
        }
        if reader.wait(timeout: .now() + 2) == .timedOut {
            task.terminate()
            _ = reader.wait(timeout: .now() + 1)
        } else {
            task.waitUntilExit()
        }
        if let out = String(data: data, encoding: .utf8),
           let start = out.range(of: "__FORGE_PATH__"),
           let end = out.range(of: "__FORGE_PATH__", range: start.upperBound..<out.endIndex)
        {
            let shellPath = String(out[start.upperBound..<end.lowerBound])
            if !shellPath.isEmpty {
                path = shellPath
            }
        }
    } catch {
    }

    setenv("PATH", path, 1)
}
