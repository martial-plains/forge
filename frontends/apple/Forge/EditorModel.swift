// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  EditorModel.swift
//  Forge
//
//  Owns the editor (forge-ffi's `Forge`), sends it keys and mouse events,
//  and keeps the latest frame for the view to draw. The editor starts off
//  the main thread (its launch waits for config and plugins to load), and
//  the getters that need its answer are awaited, so the main thread never
//  waits for the editor or a plugin.
//

import AppKit
import Combine

@MainActor
final class EditorModel: ObservableObject {
    static let optionIsMetaKey = "optionIsMeta"

    /// The selected frame's latest picture (the key window's).
    @Published private(set) var frame: Frame?
    /// All frames, each backed by its own window in the app.
    @Published private(set) var frames: [EditorFrame] = []
    /// Each frame's latest picture, by frame id, so a background window
    /// can draw itself while another frame has focus.
    @Published private(set) var frameCache: [UInt32: Frame] = [:]
    @Published private(set) var title = "Forge"
    @Published private(set) var startError: String?
    /// The editor thread's panic, if it stopped: the app shows this
    /// instead of a frozen editor. The process stays up so the message
    /// (and the log) can be read and reported.
    @Published private(set) var crash: String?
    /// Where Forge writes its log, so the crash view can point at it.
    @Published private(set) var logPath = ""
    /// The native menu bar's contents, from the editor.
    @Published private(set) var menus: [MenuGroup] = []
    /// The theme overrides, by face name, from the editor.
    @Published private(set) var theme: [String: ThemeEntry] = [:] {
        didSet { styleCache.removeAll() }
    }

    /// Called when a plugin adds or removes a menu, so the bar can be rebuilt.
    var onMenusChanged: (() -> Void)?

    private var forge: Forge?
    /// True once the editor has started: files opened before then are queued.
    var isReady: Bool { forge != nil }
    private var columns: UInt16 = 80
    private var rows: UInt16 = 24
    private var selected: UInt32 = 0
    /// Each window's size in cells, so a background frame can be laid out
    /// at the size of the window that shows it.
    private var sizes: [UInt32: (columns: UInt16, rows: UInt16)] = [:]
    private var timer: Timer?
    /// The kill ring's top already put on the system clipboard.
    private var lastClip: String?
    /// Files dropped or opened before the editor was ready.
    private var pendingOpens: [String] = []
    private var pendingURLs: [URL] = []
    /// One refresh at a time, with at most one more queued: a stream of
    /// keys can't stack up waits on the editor thread.
    private var refreshing = false
    private var refreshPending = false
    private var refreshPendingFull = false
    /// Frames a refresh is already fetching, so nothing fetches one twice.
    private var ensuring = Set<UInt32>()

    /// Option works as Meta (M-x is Option-x). Settings can turn it off.
    var optionIsMeta: Bool {
        UserDefaults.standard.object(forKey: Self.optionIsMetaKey) as? Bool ?? true
    }

    var isBusy: Bool { frame?.busy != nil }

    init() {
        do {
            let launch = try Self.launchArguments(Array(CommandLine.arguments.dropFirst()))
            if let directory = launch.directory,
               !FileManager.default.changeCurrentDirectoryPath(directory) {
                throw LaunchError.invalidDirectory(directory)
            }
            startEditor(launch)
        } catch {
            startError = "\(error)"
        }
        timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.refresh(full: true)
            }
        }
    }

    /// Start the editor off the main thread: `Forge.init` waits for the
    /// editor thread to load its config and plugins, and that wait must
    /// not freeze the app's launch (macOS flags a main thread waiting on
    /// a lower-QoS thread as a priority inversion, and the dock and menu
    /// bar stay responsive while it runs).
    private func startEditor(_ launch: LaunchArguments) {
        Task.detached(priority: .userInitiated) { [weak self] in
            do {
                let editor = try Forge(files: launch.files, noConfig: launch.noConfig)
                let logPath = editor.logPath()
                if let (line, column) = launch.position {
                    editor.gotoPosition(line: line - 1, column: column)
                }
                await self?.started(editor, logPath: logPath)
            } catch {
                await self?.startFailed("\(error)")
            }
        }
    }

    /// The editor couldn't start: the app shows this instead of it.
    private func startFailed(_ message: String) {
        startError = message
    }

    /// The editor is up: take it, open what was queued, and draw.
    private func started(_ editor: Forge, logPath: String) {
        forge = editor
        editor.setNativeScenes(on: true)
        self.logPath = logPath
        let queued = pendingOpens
        pendingOpens.removeAll()
        for path in queued {
            editor.openFile(path: path)
        }
        let queuedURLs = pendingURLs
        pendingURLs.removeAll()
        for url in queuedURLs {
            if !editor.desktop.open(url: url) {
                NSLog("Forge ignored an unsupported open URL: %@", url.absoluteString)
            }
        }
        refresh(full: true)
    }

    // MARK: - Refresh

    /// Redraw from the editor, now if nothing is refreshing. A refresh
    /// asked for while one is running runs once more when it finishes
    /// (with `full` if any of them wanted it), so a burst of keys results
    /// in one extra redraw, not one redraw each.
    func refresh(full: Bool = false) {
        requestRefresh(full: full)
        guard !refreshing else { return }
        refreshing = true
        Task { @MainActor [weak self] in
            await self?.drainRefreshes()
            self?.refreshing = false
        }
    }

    private func requestRefresh(full: Bool) {
        refreshPending = true
        if full { refreshPendingFull = true }
    }

    private func drainRefreshes() async {
        while refreshPending {
            refreshPending = false
            let full = refreshPendingFull
            refreshPendingFull = false
            await runRefresh(full: full)
        }
    }

    private func runRefresh(full: Bool) async {
        guard let forge else { return }
        if full { styleCache.removeAll() }
        if let message = forge.crash() {
            setCrash(message)
            return
        }
        await refreshFrames()
        let picture = await forge.frame(width: columns, height: rows)
        await updateSelected(picture, full: full)
    }

    /// Menus and theme are refreshed only on the idle tick (`full`) and
    /// only when the editor isn't busy, since each waits for the editor
    /// and would slow typing down behind a slow plugin.
    private func updateSelected(_ picture: Frame, full: Bool) async {
        if picture != frame {
            frame = picture
        }
        if frameCache[selected] != picture {
            frameCache[selected] = picture
        }
        updateTitle(picture)
        await syncClipboard(busy: picture.busy != nil)
        if picture.quit {
            NSApp.terminate(nil)
        }
        if full && picture.busy == nil {
            await refreshOtherFrames()
            await refreshMenus()
            await refreshTheme()
        }
    }

    /// Remember that the editor thread stopped, so the crash view can
    /// replace the editor. Called from every refresh until the app quits;
    /// the message is set once.
    private func setCrash(_ message: String) {
        if crash != message {
            crash = message
        }
        if logPath.isEmpty, let forge {
            logPath = forge.logPath()
        }
    }

    /// Make sure a frame has a picture to draw, fetching it if the window
    /// was just made and nothing has refreshed it yet.
    func ensureFrame(_ id: UInt32) {
        if frameCache[id] == nil, !ensuring.contains(id) {
            ensuring.insert(id)
            Task { @MainActor [weak self] in
                await self?.refreshFrame(id)
                self?.ensuring.remove(id)
            }
        }
    }

    /// Only what changed since the last redraw, for this one frame.
    func frame(for id: UInt32) -> Frame? { frameCache[id] }

    func refreshFrames() async {
        guard let forge else { return }
        let all = await forge.frames()
        // The editor always has at least one frame: an empty list means
        // the getter couldn't reach the editor, not that the frames are
        // gone. Treating it as real would close every window and quit.
        guard !all.isEmpty else { return }
        for f in all where sizes[f.id] == nil {
            sizes[f.id] = (columns, rows)
        }
        if let sel = all.first(where: { $0.selected }) {
            selected = sel.id
            if let size = sizes[sel.id] {
                columns = size.columns
                rows = size.rows
            }
        }
        if all != frames {
            frames = all
        }
    }

    /// Redraw one frame (a window's size changed, or it became key).
    func refreshFrame(_ id: UInt32) async {
        guard let forge else { return }
        if let message = forge.crash() {
            setCrash(message)
            return
        }
        let (width, height) = sizes[id] ?? (columns, rows)
        let picture = await forge.frameAt(id: id, width: width, height: height)
        if frameCache[id] != picture {
            frameCache[id] = picture
        }
        if id == selected {
            await updateSelected(picture, full: false)
        }
    }

    /// Redraw every frame other than the selected one, at its window's
    /// size, so background frames keep up without slowing the focused one.
    func refreshOtherFrames() async {
        guard let forge else { return }
        for f in frames where f.id != selected {
            let (width, height) = sizes[f.id] ?? (columns, rows)
            let picture = await forge.frameAt(id: f.id, width: width, height: height)
            if frameCache[f.id] != picture {
                frameCache[f.id] = picture
            }
        }
    }

    private func updateTitle(_ picture: Frame) {
        let name = picture.title.isEmpty ? "Forge" : picture.title
        let newTitle = picture.modified ? "\(name) — Edited" : name
        if newTitle != title {
            title = newTitle
        }
    }

    /// Copies (M-w and friends) reach the system clipboard. Skipped while a
    /// plugin is busy: the editor thread can't answer, and the wait isn't
    /// worth it for a picture that's about to change again.
    private func syncClipboard(busy: Bool) async {
        guard let forge, !busy else { return }
        let top = await forge.killTop()
        guard top != lastClip else { return }
        lastClip = top
        guard let top, !top.isEmpty else { return }
        if !ForgeMacOSNamespace(editor: forge).writePasteboardText(top) {
            NSLog("Forge could not update the pasteboard")
        }
    }

    // MARK: - Sending events

    private func send(_ body: (Forge) -> Void) {
        guard let forge else { return }
        body(forge)
        refresh()
    }

    func key(_ input: KeyInput) {
        send { $0.key(input: input) }
    }

    func text(_ text: String) {
        send { $0.text(text: text) }
    }

    @discardableResult
    func pasteFromPasteboard() -> Bool {
        guard let text = ForgeMacOSNamespace.readPasteboardText() else { return false }
        if !text.isEmpty {
            self.text(text)
        }
        return true
    }

    func mouse(_ kind: MouseKind, column: UInt16, row: UInt16) {
        send { $0.mouse(kind: kind, column: column, row: row) }
    }

    /// Tell the editor how big a text cell is, in points, so pictures sized
    /// in pixels (`#+attr_org: :width 250`) get the right number of rows.
    func setCellSize(width: Double, height: Double) {
        forge?.setCellSize(width: width, height: height)
    }

    /// Tell the editor whether the system is in dark mode, so canvases,
    /// Bases and graphs are drawn in colours that match.
    func setDarkMode(_ dark: Bool) {
        forge?.setDarkMode(dark: dark)
    }

    /// Canvases arrive as scenes for `SceneRenderer` to draw, not as PNGs.
    func useNativeScenes() {
        forge?.setNativeScenes(on: true)
    }

    func scrollToLine(_ line: UInt32) {
        send { $0.scrollToLine(line: line) }
    }

    func scrollToLinePixels(_ line: UInt32, pixels: UInt32) {
        send { $0.scrollToLinePixels(line: line, pixels: pixels) }
    }

    func scrollPixels(delta: Int32, rowHeight: UInt32) {
        send { $0.scrollPixels(delta: delta, rowHeight: rowHeight) }
    }

    func scrollToColumn(_ column: UInt32) {
        send { $0.scrollToColumn(column: column) }
    }

    /// One window's size changed: remember it and redraw that frame.
    func resize(frameId: UInt32, columns: UInt16, rows: UInt16) {
        sizes[frameId] = (columns, rows)
        if frameId == selected {
            self.columns = columns
            self.rows = rows
        }
        Task { @MainActor [weak self] in
            await self?.refreshFrame(frameId)
        }
    }

    // MARK: - Frames and tabs

    func selectFrame(_ id: UInt32) {
        guard id != selected else { return }
        forge?.selectFrame(id: id)
        selected = id
        if let size = sizes[id] {
            columns = size.columns
            rows = size.rows
        }
        refresh()
    }

    /// Switch the frame to another of its tabs. The editor selects the
    /// frame too, the way a click on its window does.
    func selectTab(frameId: UInt32, tab: UInt32) {
        send { $0.selectTab(frame: frameId, tab: tab) }
    }

    /// Click a tool bar button. The editor runs its command, then the
    /// menus refresh because a command can enable or disable others.
    func clickToolBarItem(_ index: UInt32) {
        send { $0.toolBarItem(index: index) }
        Task { @MainActor [weak self] in
            await self?.refreshMenus()
        }
    }

    /// Click a modifier bar button: it waits for the next key.
    func clickModifierButton(_ index: UInt32) {
        send { $0.modifierButton(index: index) }
    }

    /// Make a new tab in the frame and switch to it.
    func newTab(frameId: UInt32) {
        send { $0.newTab(frame: frameId) }
    }

    /// Move one of the frame's tabs to place `to` in its order (a drag in
    /// the tab strip).
    func moveTab(frameId: UInt32, tab: UInt32, to: Int) {
        send { $0.moveTab(frame: frameId, tab: tab, to: UInt32(to)) }
    }

    /// Close one of the frame's tabs. The editor refuses the last one.
    func closeTab(frameId: UInt32, tab: UInt32) {
        Task { @MainActor [weak self] in
            guard let self, let forge = self.forge else { return }
            _ = await forge.closeTab(frame: frameId, tab: tab)
            self.refresh()
        }
    }

    /// Delete a frame (its window closed). False when it was the last
    /// one, which the editor refuses to close.
    func closeFrame(_ id: UInt32) async -> Bool {
        guard let forge else { return false }
        return await forge.closeFrame(id: id)
    }

    // MARK: - Commands and files

    /// Run a command by name, as a menu item does.
    func runCommand(_ name: String) {
        guard let forge, !isBusy else { return }
        forge.runCommand(name: name)
        refresh()
        Task { @MainActor [weak self] in
            await self?.refreshMenus()
        }
    }

    /// The context menu while the editor has one open (right-click, or
    /// S-F10), or nil when it is off or closed. The app turns it into its
    /// own native menu.
    func contextMenu() async -> ContextMenuFrame? {
        guard let forge else { return nil }
        return await forge.contextMenu()
    }

    /// What a click would do at a screen cell (its text span and tooltip),
    /// or nil when nothing clickable is there.
    func hover(column: UInt16, row: UInt16) async -> HoverInfo? {
        guard let forge else { return nil }
        return await forge.hover(column: column, row: row)
    }

    /// Dismiss the editor's context menu, as closing the native popup does.
    func closeContextMenu() {
        forge?.closeContextMenu()
    }

    /// Run an item from the context menu: close the editor's copy of the
    /// menu first (the command still knows where the click was), then run
    /// it as any other menu item would.
    func runContextCommand(_ name: String) {
        guard let forge, !isBusy else { return }
        forge.closeContextMenu()
        forge.runCommand(name: name)
        refresh()
        Task { @MainActor [weak self] in
            await self?.refreshMenus()
        }
    }

    /// C-g while a plugin or init.rhai is busy.
    func interrupt() {
        forge?.interrupt()
    }

    func open(path: String) {
        guard let forge else {
            // The editor is still starting: open it when it's up.
            pendingOpens.append(path)
            return
        }
        forge.openFile(path: path)
        refresh()
    }

    func open(url: URL) {
        guard let forge else {
            pendingURLs.append(url)
            return
        }
        if !forge.desktop.open(url: url) {
            NSLog("Forge ignored an unsupported open URL: %@", url.absoluteString)
            return
        }
        refresh()
    }

    func openServiceText(_ text: String) {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("Forge-Service-\(UUID().uuidString).txt")
        do {
            try text.write(to: url, atomically: true, encoding: .utf8)
            open(path: url.path)
        } catch {
            NSLog("Forge could not create a text document for Services: %@", String(describing: error))
        }
    }

    func showOpenPanel() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        if panel.runModal() == .OK {
            for url in panel.urls {
                open(path: url.path)
            }
        }
    }

    /// Remember places, recent files and history now (the app is quitting).
    /// Synchronous on purpose: the process is about to exit.
    func saveSession() {
        forge?.saveSession()
    }

    // MARK: - Menus and theme

    /// The menu bar's contents: the same table the terminal's
    /// menu-bar-mode draws, so both offer the same commands.
    func refreshMenus() async {
        guard let forge else { return }
        let groups = await forge.menuBar()
        // An empty table means the request failed or timed out (the editor
        // always has menus), not that every menu went away. Taking it would
        // rebuild the bar as just the app menu and View, and the next
        // refresh would swap the real one back: the bar flickering between
        // the two.
        guard !groups.isEmpty, groups != menus else { return }
        // A plugin that defines a menu of its own adds a title to the bar,
        // not just items to one, so the whole bar is rebuilt when the set
        // of titles changes. Items changing is cheaper: each menu refills
        // itself when it opens.
        let titles = groups.map(\.title)
        let changedTitles = titles != menus.map(\.title)
        menus = groups
        if changedTitles {
            onMenusChanged?()
        }
    }

    /// The theme overrides: the same table the terminal paints with, so
    /// both frontends agree on text colors.
    func refreshTheme() async {
        guard let forge else { return }
        let table = Dictionary(
            uniqueKeysWithValues: await forge.theme().map { ($0.face, $0) }
        )
        if table != theme {
            theme = table
        }
    }

    /// A face style with the theme overrides applied: what the editor's
    /// own style says, with every set field replaced.
    func faceStyle(face: String, syntax: String?) -> FaceStyle {
        // Every span of every row asks for its style on every redraw, and
        // working one out (colour blending, parsing theme hex strings) is
        // far dearer than looking it up, so redraws while a key is held
        // down (arrow keys) would crawl. The cache is cleared when the
        // theme changes and on each idle refresh (see `refresh`), which is
        // when a light/dark switch is picked up.
        let key = syntax.map { face + "\u{1F}" + $0 } ?? face
        if let hit = styleCache[key] { return hit }
        let style = computeFaceStyle(face: face, syntax: syntax)
        styleCache[key] = style
        return style
    }

    private var styleCache: [String: FaceStyle] = [:]

    private func computeFaceStyle(face: String, syntax: String?) -> FaceStyle {
        var style = Theme.style(face: face, syntax: syntax)
        guard let override = theme[face] else { return style }
        if let fg = override.fg.flatMap(NSColor.init(hex:)) {
            style.foreground = fg
        }
        if let bg = override.bg.flatMap(NSColor.init(hex:)) {
            style.background = bg
        }
        if let bold = override.bold {
            style.bold = bold
        }
        if let italic = override.italic {
            style.italic = italic
        }
        switch override.underline {
        case true where style.underline == nil:
            style.underline = style.foreground
        case false:
            style.underline = nil
        default:
            break
        }
        return style
    }

    // MARK: - Launch arguments

    private struct LaunchArguments {
        var files: [String] = []
        var position: (line: UInt32, column: UInt32)?
        var pendingPosition: (line: UInt32, column: UInt32)?
        var directory: String?
        var noConfig = false
    }

    private enum LaunchError: LocalizedError {
        case invalidLocation(String)
        case invalidDirectory(String)

        var errorDescription: String? {
            switch self {
            case .invalidLocation(let value):
                "Invalid file location '\(value)'; expected +LINE[:COLUMN]."
            case .invalidDirectory(let path):
                "Cannot change directory to \(path)."
            }
        }
    }

    private static func launchArguments(_ args: [String]) throws -> LaunchArguments {
        var result = LaunchArguments()
        var i = 0
        var positionalOnly = false
        while i < args.count {
            let arg = args[i]
            i += 1
            if !positionalOnly, arg == "--" {
                positionalOnly = true
                continue
            }
            if !positionalOnly, arg == "-q" || arg == "--no-config" {
                result.noConfig = true
                continue
            }
            if !positionalOnly,
               ["--file", "--find-file", "--visit", "--chdir", "--plugin"].contains(arg) {
                guard i < args.count else { continue }
                let value = args[i]
                i += 1
                switch arg {
                case "--chdir": result.directory = value
                case "--plugin": break
                default:
                    result.files.append(value)
                    result.position = result.pendingPosition
                    result.pendingPosition = nil
                }
                continue
            }
            if !positionalOnly, arg.hasPrefix("+") {
                let location = String(arg.dropFirst())
                let parts = location.split(separator: ":", omittingEmptySubsequences: false)
                guard parts.count <= 2, let line = UInt32(parts[0]), line > 0 else {
                    throw LaunchError.invalidLocation(arg)
                }
                let column: UInt32
                if parts.count == 2 {
                    guard let parsed = UInt32(parts[1]) else {
                        throw LaunchError.invalidLocation(arg)
                    }
                    column = parsed
                } else {
                    column = 0
                }
                result.pendingPosition = (line, column)
                continue
            }
            if !positionalOnly, arg.hasPrefix("-") {
                continue
            }
            result.files.append(arg)
            result.position = result.pendingPosition
            result.pendingPosition = nil
        }
        return result
    }
}
