// SPDX-License-Identifier: MIT OR Apache-2.0
//
//  EditorView.swift
//  Forge
//
//  Draws a Forge frame as a grid of character cells (like the terminal
//  does), and turns keys and the mouse into Forge events. An AppKit view
//  inside SwiftUI, for full control over keys and text drawing.
//

import AppKit
import SwiftUI
import WebKit

struct EditorView: NSViewRepresentable {
    @ObservedObject var model: EditorModel
    /// Which frame this window draws. One window per frame.
    let frameId: UInt32

    func makeNSView(context: Context) -> EditorNSView {
        let view = EditorNSView()
        view.model = model
        view.frameId = frameId
        return view
    }

    func updateNSView(_ view: EditorNSView, context: Context) {
        view.frameId = frameId
        if let f = model.frame(for: frameId), f != view.current {
            // A new picture: repaint just the rows that changed.
            view.show(f)
        } else {
            // Nothing new from the editor (a theme change, say): repaint all.
            view.invalidate()
        }
        view.updateWindowTitle()
        view.updateScrollers()
        view.syncWeb()
    }
}

@MainActor
final class EditorNSView: NSView, NSMenuDelegate {
    weak var model: EditorModel?
    /// The target the context menu's items send their commands to.
    private let contextTarget = ContextMenuTarget()
    /// The frame this view draws, and the one it selects when its window
    /// becomes key, so keys act on the frame under the user's attention.
    var frameId: UInt32 = 0
    var current: Frame?
    private var keyObserver: NSObjectProtocol?
    private var resignObserver: NSObjectProtocol?
    /// The canvas the frame is being painted on (set only inside `paint`).
    private var out: GridCanvas!
    /// The Metal renderer, when the GPU is in use; nil draws with AppKit.
    private var gpu: MetalGridRenderer?
    private var imageOverlay: ImageOverlayView?
    private var gpuTried = false
    /// Catches editor keys before the web view's private content view
    /// swallows them: Control/Option key-downs aren't key equivalents, so
    /// they never reach `ForgeWebView.keyDown` while the page has focus.
    private var webKeyMonitor: Any?

    // MARK: Text size

    static let defaultFontSize: CGFloat = 13
    private static let fontSizeKey = "ForgeFontSize"

    /// The point size of the text (View → Zoom In, Zoom Out, Actual Size).
    private(set) var fontSize: CGFloat = {
        let saved = UserDefaults.standard.double(forKey: EditorNSView.fontSizeKey)
        return saved >= 6 && saved <= 72 ? CGFloat(saved) : EditorNSView.defaultFontSize
    }()
    private(set) var font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    private var boldFont = NSFont.monospacedSystemFont(ofSize: 13, weight: .bold)
    private var italicFont = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    private var cellSize = CGSize(width: 8, height: 16)

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        rebuildFonts()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        rebuildFonts()
    }

    /// Fonts and the cell size for `fontSize`.
    private func rebuildFonts() {
        font = NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        boldFont = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
        italicFont = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask)
        let width = ("M" as NSString).size(withAttributes: [.font: font]).width
        // Keep the fractional glyph advance. Rounding each cell up means the
        // grid drifts farther from the text by a fraction of a point per
        // character, leaving the cursor visibly behind long lines.
        cellSize = CGSize(width: width, height: ceil(font.ascender - font.descender + font.leading))
    }

    /// Change the text size, keeping it between 6 and 72 points, and lay the
    /// grid out again at the new cell size.
    func setFontSize(_ size: CGFloat) {
        let clamped = min(72, max(6, size.rounded()))
        guard clamped != fontSize else { return }
        fontSize = clamped
        UserDefaults.standard.set(Double(clamped), forKey: Self.fontSizeKey)
        rebuildFonts()
        gpu?.atlas.reset()
        updateGridSize()
        updateScrollers()
        syncWeb()
        invalidate()
    }

    @objc func zoomIn(_ sender: Any?) { setFontSize(fontSize + 1) }
    @objc func zoomOut(_ sender: Any?) { setFontSize(fontSize - 1) }
    @objc func zoomReset(_ sender: Any?) { setFontSize(Self.defaultFontSize) }

    /// Trackpad scrolling comes in small steps; one wheel event per row.
    private var scrollAccumulator: CGFloat = 0
    /// Fractional pixels waiting to be sent to the editor in
    /// pixel-scroll-precision-mode.
    private var pixelAccumulator: CGFloat = 0

    // MARK: Native scroll bars (scroll-bar-mode, horizontal-scroll-bar-mode)

    private lazy var vScroller: NSScroller = makeScroller(tag: 0)
    private lazy var hScroller: NSScroller = makeScroller(tag: 1)

    private func makeScroller(tag: Int) -> NSScroller {
        let s = NSScroller()
        s.scrollerStyle = .overlay
        s.knobStyle = .default
        s.controlSize = .regular
        s.target = self
        s.action = #selector(scrollerMoved(_:))
        s.tag = tag
        s.isHidden = true
        addSubview(s)
        return s
    }

    /// The user moved a native scroller: tell the editor where to scroll.
    @objc private func scrollerMoved(_ sender: NSScroller) {
        guard let info = current?.scroll else { return }
        if sender.tag == 0 {
            let span = max(0, Int(info.total) - Int(info.visible))
            let exact = sender.doubleValue * Double(span)
            if current?.pixelScrollPrecision == true {
                let rowHeight = max(1.0, Double(cellSize.height))
                var line = max(0, min(span, Int(floor(exact))))
                var pixels = Int(((exact - Double(line)) * rowHeight).rounded())
                if line >= span {
                    line = span
                    pixels = 0
                }
                pixels = max(0, min(Int(rowHeight) - 1, pixels))
                model?.scrollToLinePixels(UInt32(line), pixels: UInt32(pixels))
            } else {
                let line = Int(exact.rounded())
                model?.scrollToLine(UInt32(max(0, min(span, line))))
            }
        } else {
            let span = max(0, Int(info.lineWidth) - Int(info.width))
            let column = Int((sender.doubleValue * Double(span)).rounded())
            model?.scrollToColumn(UInt32(max(0, min(span, column))))
        }
    }

    /// Put the native scrollers over the selected window's text and set
    /// their thumbs from the editor's scroll position.
    /// Put a new picture on screen. When only text rows, the highlighted
    /// line, mode lines or the cursor moved (an arrow key, typing), only
    /// those rows are repainted; anything else repaints the whole grid.
    /// Repainting every cell for each key made a held-down arrow key crawl.
    func show(_ new: Frame) {
        let old = current
        current = new
        // The GPU repaints the whole grid in one pass; there is nothing to gain from diffing.
        if gpu != nil {
            invalidate()
            return
        }
        guard let old, let dirty = changedRows(from: old, to: new) else {
            invalidate()
            return
        }
        for rect in dirty {
            invalidate(rect)
        }
    }

    /// Ask for a repaint of `rect` (everything when nil). Nothing is drawn
    /// until the system next asks, so an idle editor costs no power.
    func invalidate(_ rect: NSRect? = nil) {
        if let gpu {
            gpu.view.needsDisplay = true
        } else if let rect {
            setNeedsDisplay(rect)
        } else {
            needsDisplay = true
        }
    }

    /// The rectangles that differ between two pictures, or nil when the
    /// difference isn't confined to rows (so everything must be repainted).
    private func changedRows(from old: Frame, to new: Frame) -> [NSRect]? {
        guard old.windows.count == new.windows.count else { return nil }
        // A scaled window's rows are not grid rows: repaint everything.
        guard !old.windows.contains(where: { $0.cellScalePct != 100 }),
              !new.windows.contains(where: { $0.cellScalePct != 100 }) else { return nil }
        // Everything outside the windows' rows must match, bar the cursor.
        var a = old, b = new
        a.windows = []
        b.windows = []
        a.cursorX = 0
        b.cursorX = 0
        a.cursorY = 0
        b.cursorY = 0
        guard a == b else { return nil }

        let height = cellSize.height
        var rects: [NSRect] = []
        func row(_ y: Int) {
            rects.append(NSRect(x: 0, y: CGFloat(y) * height, width: bounds.width, height: height))
        }
        for (ow, nw) in zip(old.windows, new.windows) {
            var x = ow, y = nw
            x.rows = []
            y.rows = []
            x.modeLine = []
            y.modeLine = []
            x.hlRows = []
            y.hlRows = []
            // A pixel-scrolled window shifts all its text; repaint it all.
            guard x == y, nw.scrollYPx == 0, ow.rows.count == nw.rows.count else { return nil }
            for i in 0..<nw.rows.count where ow.rows[i] != nw.rows[i] {
                row(Int(nw.y) + i)
            }
            if ow.modeLine != nw.modeLine {
                row(Int(nw.y) + Int(nw.height) - 1)
            }
            for r in Set(ow.hlRows).symmetricDifference(nw.hlRows) {
                row(Int(nw.y) + Int(r))
            }
        }
        if old.cursorX != new.cursorX || old.cursorY != new.cursorY {
            row(Int(old.cursorY))
            row(Int(new.cursorY))
        }
        return rects
    }

    /// Whether the grid row at `y` overlaps the area being repainted.
    private func rowIsDirty(_ y: Int, _ dirty: NSRect) -> Bool {
        let top = CGFloat(y) * cellSize.height
        return top < dirty.maxY && top + cellSize.height > dirty.minY
    }

    func updateScrollers() {
        guard let f = current, let win = f.windows.first(where: { $0.selected }) else {
            vScroller.isHidden = true
            hScroller.isHidden = true
            return
        }
        let thickness = NSScroller.scrollerWidth(for: .regular, scrollerStyle: .overlay)
        let left = CGFloat(win.x) * cellSize.width
        let top = CGFloat(win.y) * cellSize.height
        let width = CGFloat(win.width) * cellSize.width
        let height = CGFloat(max(1, Int(win.height) - 1)) * cellSize.height

        if f.scrollBar, let info = f.scroll {
            let proportion = info.total > 0 ? CGFloat(info.visible) / CGFloat(info.total) : 1
            vScroller.knobProportion = max(0.05, min(1, proportion))
            let span = max(0, Int(info.total) - Int(info.visible))
            let rowHeight = max(1.0, Double(cellSize.height))
            let position = min(Double(span), Double(info.top) + Double(info.scrollYPx) / rowHeight)
            vScroller.doubleValue = span > 0 ? position / Double(span) : 0
            vScroller.isEnabled = info.total > Int(info.visible)
            let vFrame = NSRect(x: left + width - thickness, y: top,
                                width: thickness, height: height)
            if vScroller.frame != vFrame { vScroller.frame = vFrame }
            vScroller.isHidden = false
        } else {
            vScroller.isHidden = true
        }

        if f.horizontalScrollBar, let info = f.scroll {
            let proportion = info.lineWidth > 0 ? CGFloat(info.width) / CGFloat(info.lineWidth) : 1
            hScroller.knobProportion = max(0.05, min(1, proportion))
            let span = max(0, Int(info.lineWidth) - Int(info.width))
            hScroller.doubleValue = span > 0 ? Double(min(span, Int(info.left))) / Double(span) : 0
            hScroller.isEnabled = info.lineWidth > Int(info.width)
            let hFrame = NSRect(x: left, y: top + height - thickness,
                                width: width, height: thickness)
            if hScroller.frame != hFrame { hScroller.frame = hFrame }
            hScroller.isHidden = false
        } else {
            hScroller.isHidden = true
        }
    }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    // MARK: Web pages (webkit)

    /// One web view per page of `M-x webkit`, made the first time a window
    /// shows it and kept while its buffer lives, so switching buffers
    /// doesn't lose the page.
    private var webs: [UInt32: ForgeWebView] = [:]
    /// The last request from the editor that was done, per page, so each
    /// is done once.
    private var webSeq: [UInt32: UInt64] = [:]
    private var lastWebReport: [UInt32: String] = [:]

    /// Put each page's web view over the text rows of the window that
    /// shows it, do what the editor asked of it, hide the ones nobody
    /// shows, and drop the ones whose page is closed.
    func syncWeb() {
        guard let f = current else { return }
        // Collect first: dropping a view mutates `webs`, which must not
        // happen while it is being iterated.
        let live = Set(f.webPages)
        for id in webs.keys.filter({ !live.contains($0) }) {
            destroyWeb(id)
        }
        var shown = Set<UInt32>()
        for w in f.windows {
            guard let state = w.web else { continue }
            shown.insert(state.page)
            guard let page = webs[state.page] ?? makeWeb(state.page) else { continue }
            page.optionIsMeta = model?.optionIsMeta ?? true
            let textWidth = Int(w.width) - (w.border ? 1 : 0)
            let frame = NSRect(
                x: CGFloat(w.x) * cellSize.width,
                y: CGFloat(w.y) * cellSize.height,
                width: CGFloat(textWidth) * cellSize.width,
                height: CGFloat(max(1, Int(w.height) - 1)) * cellSize.height
            )
            // Fractional cell advances stay fractional: rounding each view
            // would drift it off the grid it covers.
            if page.frame != frame { page.frame = frame }
            if page.isHidden { page.isHidden = false }
            if state.seq != (webSeq[state.page] ?? 0) {
                webSeq[state.page] = state.seq
                perform(state.command, on: page, id: state.page)
            }
        }
        for (id, page) in webs where !shown.contains(id) {
            if !page.isHidden { page.isHidden = true }
            if page.hintsActive {
                page.evaluateJavaScript(
                    "window.__forgeHintsCancel && window.__forgeHintsCancel('dismiss');"
                )
            }
        }
    }

    /// Tear down one page's view: stop it, detach its delegates, handlers
    /// and observers, and forget its bookkeeping.
    private func destroyWeb(_ id: UInt32) {
        guard let page = webs[id] else { return }
        page.stopLoading()
        page.navigationDelegate = nil
        page.uiDelegate = nil
        for o in page.observers { o.invalidate() }
        page.observers = []
        page.hover = nil
        page.onEditorKey = nil
        page.configuration.userContentController.removeScriptMessageHandler(forName: "forgeHover")
        page.configuration.userContentController.removeAllUserScripts()
        page.removeFromSuperview()
        webs[id] = nil
        webSeq[id] = nil
        lastWebReport[id] = nil
        webDelegates[id] = nil
    }

    /// Navigation/task delegates, one per live page. WKWebView holds its
    /// delegates weakly, so the view keeps them alive here.
    private var webDelegates: [UInt32: WebNavDelegate] = [:]

    private func makeWeb(_ id: UInt32) -> ForgeWebView? {
        let config = WKWebViewConfiguration()
        // Tell the editor which link the pointer is over. Mouse-over alone
        // misses keyboard focus and moves between adjacent links, so
        // mouse-move and focus events feed the same de-duplicated sender,
        // and leaving the page always clears it.
        let script = WKUserScript(
            source: """
            (function() {
              var last = "";
              function send(u) {
                if (u === last) return;
                last = u;
                try { window.webkit.messageHandlers.forgeHover.postMessage(u); } catch (e) {}
              }
              function linkFromTarget(t) {
                try {
                  if (!t) return null;
                  if (t.nodeType === 3) t = t.parentElement;
                  if (t && t.closest) return t.closest("a[href]");
                } catch (e) {}
                return null;
              }
              function update(e) {
                var a = (e && e.target) ? linkFromTarget(e.target) : null;
                var href = "";
                try { href = (a && a.href) ? a.href : ""; } catch (err) { href = ""; }
                send(href);
              }
              document.addEventListener("mouseover", update, true);
              document.addEventListener("mousemove", update, true);
              document.addEventListener("focusin", update, true);
              document.addEventListener("mouseout", function(e) {
                var to = null;
                try { to = e.relatedTarget ? linkFromTarget(e.relatedTarget) : null; } catch (err) {}
                if (!e.relatedTarget || !to) update(e.relatedTarget ? {target: e.relatedTarget} : null);
              }, true);
              document.addEventListener("focusout", function() { send(""); }, true);
              document.addEventListener("mouseleave", function() { send(""); }, true);
              window.addEventListener("pagehide", function() { send(""); }, true);
            })();
            """,
            injectionTime: .atDocumentStart,
            forMainFrameOnly: false
        )
        config.userContentController.addUserScript(script)
        let hover = HoverHandler { [weak self] link in
            guard let self, self.webs[id] != nil else { return }
            self.model?.runCommand("webkit--hover \(id)\t\(Self.oneLine(link))")
        }
        config.userContentController.add(hover, name: "forgeHover")
        // The link-hint prompt (webkit-hints) reports what was chosen.
        let hints = HintsHandler { [weak self] kind, url in
            guard let self, let page = self.webs[id] else { return }
            page.hintsActive = false
            // A field keeps the keyboard so you can type into it; anything
            // else gives it back to the editor.
            if kind != "focus" { self.window?.makeFirstResponder(self) }
            self.model?.runCommand(
                "webkit--hinted \(id)\t\(kind)\t\(Self.jsonString(url))"
            )
        }
        config.userContentController.add(hints, name: "forgeHints")
        let page = ForgeWebView(frame: .zero, configuration: config)
        page.hintsHandler = hints
        page.onHintsEscape = { [weak page] in
            page?.evaluateJavaScript("window.__forgeHintsCancel && window.__forgeHintsCancel();")
        }
        page.hover = hover
        page.optionIsMeta = model?.optionIsMeta ?? true
        page.allowsBackForwardNavigationGestures = true
        let delegate = WebNavDelegate(id: id, owner: self)
        page.navigationDelegate = delegate
        page.uiDelegate = delegate
        webDelegates[id] = delegate
        page.onEditorKey = { [weak self] event in
            guard let self else { return }
            // Move focus first: the rest of a prefix (C-x o, M-x,
            // ESC …) is plain keys, which only reach the editor when it
            // is the first responder.
            self.window?.makeFirstResponder(self)
            self.keyDown(with: event)
        }
        // The editor wants the address and title as they change; the
        // navigation delegate also reports commits, finishes and errors,
        // so failed loads are seen even when neither changes.
        page.observers = [
            page.observe(\.url, options: [.new]) { [weak self] page, _ in
                MainActor.assumeIsolated { self?.reportWeb(id, page) }
            },
            page.observe(\.title, options: [.new]) { [weak self] page, _ in
                MainActor.assumeIsolated { self?.reportWeb(id, page) }
            },
        ]
        addSubview(page)
        webs[id] = page
        return page
    }

    private static func oneLine(_ text: String) -> String {
        text.replacingOccurrences(of: "\t", with: " ").replacingOccurrences(of: "\n", with: " ")
    }

    private func reportWeb(_ id: UInt32, _ page: WKWebView) {
        guard webs[id] != nil else { return }
        let url = page.url?.absoluteString ?? ""
        let report = "\(url)\t\(Self.oneLine(page.title ?? ""))"
        guard !url.isEmpty, report != lastWebReport[id] else { return }
        lastWebReport[id] = report
        model?.runCommand("webkit--report \(id)\t\(report)")
    }

    fileprivate func webDidStart(_ id: UInt32, url: String) {
        guard webs[id] != nil, !url.isEmpty else { return }
        model?.runCommand("webkit--loading \(id)\t\(Self.oneLine(url))")
    }

    fileprivate func webDidFinish(_ id: UInt32) {
        guard let page = webs[id] else { return }
        reportWeb(id, page)
    }

    fileprivate func webDidFail(_ id: UInt32, url: String, message: String) {
        guard webs[id] != nil else { return }
        let clean = Self.oneLine(message)
        guard !clean.isEmpty else { return }
        model?.runCommand("webkit--error \(id)\t\(Self.oneLine(url))\t\(clean)")
    }

    private func perform(_ command: String, on page: ForgeWebView, id: UInt32) {
        // The page may have closed between the frame and the request.
        guard webs[id] === page else { return }
        // Link labels belong to the screen they were drawn on: anything the
        // editor asks of the page next (a scroll, a load, going back) takes
        // them away.
        if page.hintsActive, !command.hasPrefix("hints:") {
            page.evaluateJavaScript(
                "window.__forgeHintsCancel && window.__forgeHintsCancel('dismiss');"
            )
        }
        switch command {
        case let c where c.hasPrefix("load:"):
            let text = String(c.dropFirst(5))
            guard let url = URL(string: text),
                  let scheme = url.scheme?.lowercased(),
                  ["http", "https", "file"].contains(scheme)
            else {
                model?.runCommand("webkit--error \(id)\t\tBad address")
                return
            }
            if url.isFileURL {
                page.loadFileURL(url, allowingReadAccessTo: url.deletingLastPathComponent())
            } else {
                page.load(URLRequest(url: url))
            }
        case "back": if page.canGoBack { page.goBack() }
        case "forward": if page.canGoForward { page.goForward() }
        case "reload": page.reload()
        case "stop": page.stopLoading()
        case "focus": window?.makeFirstResponder(page)
        case "zoom:in": page.pageZoom = min(5, page.pageZoom + 0.1)
        case "zoom:out": page.pageZoom = max(0.25, page.pageZoom - 0.1)
        case "zoom:reset": page.pageZoom = 1
        case let c where c.hasPrefix("find:"):
            page.findText = String(c.dropFirst(5))
            find(on: page, id: id, backwards: false)
        case "find-next": find(on: page, id: id, backwards: false)
        case "find-previous": find(on: page, id: id, backwards: true)
        case "selection": copySelection(on: page, id: id)
        case "history": reportHistory(on: page, id: id)
        case let c where c.hasPrefix("history:"):
            if let index = Int(c.dropFirst("history:".count)) {
                goHistory(index, on: page)
            }
        case let c where c.hasPrefix("insert:"):
            insert(String(c.dropFirst("insert:".count)), on: page, id: id)
        case let c where c.hasPrefix("show-element:"):
            showElement(String(c.dropFirst("show-element:".count)), on: page, id: id)
        case let c where c.hasPrefix("hints:"):
            startHints(String(c.dropFirst("hints:".count)), on: page)
        case "focus-input": focusFirstInput(on: page, id: id)
        case let c where c.hasPrefix("link:"):
            followRelatedLink(String(c.dropFirst("link:".count)), on: page, id: id)
        case let c where c.hasPrefix("scroll-"):
            scroll(c, on: page)
        default: break
        }
    }

    private static func jsonString(_ text: String) -> String {
        guard let data = try? JSONSerialization.data(
            withJSONObject: text,
            options: [.fragmentsAllowed]
        ), let out = String(data: data, encoding: .utf8)
        else { return "\"\"" }
        return out
    }

    private static func decodeJSONString(_ text: String) -> String? {
        guard let data = text.data(using: .utf8),
              let value = try? JSONSerialization.jsonObject(
                  with: data,
                  options: [.fragmentsAllowed]
              )
        else { return nil }
        return value as? String
    }

    /// Run a script that answers true or false, and tell the editor which.
    private func reportScript(
        _ script: String,
        on page: ForgeWebView,
        id: UInt32,
        as report: String
    ) {
        page.evaluateJavaScript(script) { [weak self] result, _ in
            let ok = (result as? NSNumber)?.boolValue ?? (result as? Bool) ?? false
            MainActor.assumeIsolated {
                guard let self, self.webs[id] != nil else { return }
                self.model?.runCommand("webkit--\(report) \(id)\t\(ok ? 1 : 0)")
            }
        }
    }

    private func insert(_ json: String, on page: ForgeWebView, id: UInt32) {
        let text = Self.decodeJSONString(json) ?? ""
        let value = Self.jsonString(text)
        reportScript(
            """
            (function(value) {
              var el = document.activeElement;
              if (!el) return false;
              if (typeof el.value !== "undefined") {
                el.value = value;
                el.dispatchEvent(new Event("input", {bubbles: true}));
                el.dispatchEvent(new Event("change", {bubbles: true}));
                return true;
              }
              if (el.isContentEditable) {
                el.textContent = value;
                el.dispatchEvent(new Event("input", {bubbles: true}));
                return true;
              }
              return false;
            })(\(value));
            """,
            on: page,
            id: id,
            as: "inserted"
        )
    }

    private func showElement(_ command: String, on page: ForgeWebView, id: UInt32) {
        guard let colon = command.firstIndex(of: ":") else { return }
        let kind = String(command[..<colon])
        let query = String(command[command.index(after: colon)...])
        guard !query.isEmpty else { return }
        let lookup: String
        switch kind {
        case "id": lookup = "document.getElementById(q)"
        case "name": lookup = "document.getElementsByName(q)[0]"
        case "id-or-name":
            lookup = "document.getElementById(q) || document.getElementsByName(q)[0]"
        default: lookup = "document.querySelector(q)"
        }
        reportScript(
            """
            (function(q) {
              var el = \(lookup);
              if (!el) return false;
              el.scrollIntoView({block: "start"});
              return true;
            })(\(Self.jsonString(query)));
            """,
            on: page,
            id: id,
            as: "shown"
        )
    }

    /// Vim-style link hints: a short label over every visible link and
    /// control; typing a label follows it (`same`), opens it in a new page
    /// (`new`) or copies its address (`copy`). The page runs the prompt,
    /// so it takes the keyboard until a label is chosen or Escape is
    /// pressed.
    private func startHints(_ mode: String, on page: ForgeWebView) {
        page.hintsActive = true
        window?.makeFirstResponder(page)
        page.evaluateJavaScript(Self.hintsScript(mode: Self.jsonString(mode)))
    }

    private static func hintsScript(mode: String) -> String {
        #"""
        (function(mode) {
          if (window.__forgeHintsCancel) window.__forgeHintsCancel(true);
          var chars = "sadfjklewcmpgh";
          function post(kind, url) {
            try {
              window.webkit.messageHandlers.forgeHints.postMessage({kind: kind, url: url || ""});
            } catch (e) {}
          }
          function visible(el) {
            var r = el.getBoundingClientRect();
            if (r.width < 2 || r.height < 2) return false;
            if (r.bottom < 0 || r.right < 0 || r.top > innerHeight || r.left > innerWidth) return false;
            var cs = getComputedStyle(el);
            return cs.visibility !== "hidden" && cs.display !== "none" && cs.opacity !== "0";
          }
          var sel = 'a[href], button, input:not([type=hidden]), select, textarea, summary, ' +
                    '[onclick], [role=button], [role=link], [role=tab], [role=menuitem], ' +
                    '[contenteditable=true], [tabindex]:not([tabindex="-1"])';
          var els = Array.prototype.filter.call(document.querySelectorAll(sel), visible);
          if (els.length === 0) { post("none"); return; }
          var len = 1;
          while (Math.pow(chars.length, len) < els.length) len++;
          function label(n) {
            var s = "";
            for (var i = 0; i < len; i++) { s = chars[n % chars.length] + s; n = Math.floor(n / chars.length); }
            return s;
          }
          var box = document.createElement("div");
          box.style.cssText = "position:absolute;left:0;top:0;z-index:2147483647;pointer-events:none;";
          var items = els.map(function(el, i) {
            var r = el.getBoundingClientRect();
            var tag = document.createElement("span");
            var text = label(i);
            tag.textContent = text.toUpperCase();
            tag.style.cssText = "position:absolute;background:#ffd84a;color:#222;font:bold 11px/1.2 monospace;" +
              "padding:1px 3px;border:1px solid #b8960b;border-radius:3px;box-shadow:0 1px 2px #0006;" +
              "left:" + (r.left + scrollX) + "px;top:" + (r.top + scrollY) + "px;";
            box.appendChild(tag);
            return {el: el, text: text, tag: tag};
          });
          document.documentElement.appendChild(box);
          var typed = "";
          // The labels belong to this moment of this page: anything that
          // changes what is on screen (a scroll, a resize, going to another
          // address, the page being hidden) makes them wrong, so they go.
          var startUrl = location.href;
          var watch = setInterval(function() {
            if (location.href !== startUrl || document.hidden) dismiss();
          }, 250);
          var events = ["scroll", "resize", "pagehide", "popstate", "hashchange", "wheel", "touchmove"];
          function dismiss() { cleanup(); post("dismiss"); }
          function cleanup() {
            window.removeEventListener("keydown", onKey, true);
            events.forEach(function(n) { window.removeEventListener(n, dismiss, true); });
            document.removeEventListener("visibilitychange", dismiss, true);
            clearInterval(watch);
            if (box.parentNode) box.parentNode.removeChild(box);
            window.__forgeHintsCancel = null;
          }
          // true: replaced by a new prompt, say nothing. "dismiss": the
          // editor took the keys elsewhere. Otherwise Escape.
          window.__forgeHintsCancel = function(how) {
            cleanup();
            if (how === true) return;
            post(how || "cancel");
          };
          events.forEach(function(n) { window.addEventListener(n, dismiss, true); });
          document.addEventListener("visibilitychange", dismiss, true);
          function choose(item) {
            cleanup();
            var el = item.el;
            var href = el.href || (el.closest && el.closest("a[href]") ? el.closest("a[href]").href : "");
            if (mode === "copy") { post("copy", href); return; }
            if (mode === "new" && href) { post("new", href); return; }
            var tagName = (el.tagName || "").toLowerCase();
            var type = (el.type || "").toLowerCase();
            var field = tagName === "textarea" || el.isContentEditable ||
              (tagName === "input" && ["button","submit","checkbox","radio","reset","image","file"].indexOf(type) < 0);
            if (field) { el.focus(); post("focus"); return; }
            el.focus && el.focus();
            el.click();
            post("done");
          }
          function onKey(e) {
            if (e.metaKey || e.ctrlKey || e.altKey) return;
            var k = e.key;
            if (k === "Backspace") {
              typed = typed.slice(0, -1);
            } else if (k.length === 1 && chars.indexOf(k.toLowerCase()) >= 0) {
              typed += k.toLowerCase();
            } else {
              return;
            }
            e.preventDefault(); e.stopPropagation();
            var left = 0, last = null;
            items.forEach(function(it) {
              var on = it.text.indexOf(typed) === 0;
              it.tag.style.display = on ? "" : "none";
              if (on) { left++; last = it; }
            });
            if (left === 0) { typed = ""; items.forEach(function(it) { it.tag.style.display = ""; }); return; }
            if (left === 1 && last.text === typed) choose(last);
          }
          window.addEventListener("keydown", onKey, true);
        })(\#(mode));
        """#
    }

    /// Put the cursor in the page's first visible text field and give the
    /// page the keyboard.
    private func focusFirstInput(on page: ForgeWebView, id: UInt32) {
        let script = #"""
        (function() {
          var els = document.querySelectorAll(
            'input:not([type=hidden]):not([type=checkbox]):not([type=radio]):not([type=submit]):not([type=button]), textarea, [contenteditable=true]');
          for (var i = 0; i < els.length; i++) {
            var r = els[i].getBoundingClientRect();
            if (r.width > 0 && r.height > 0 && !els[i].disabled) {
              els[i].scrollIntoView({block: "center"});
              els[i].focus();
              if (els[i].select) els[i].select();
              return true;
            }
          }
          return false;
        })();
        """#
        page.evaluateJavaScript(script) { [weak self] result, _ in
            let ok = (result as? NSNumber)?.boolValue ?? false
            MainActor.assumeIsolated {
                guard let self, self.webs[id] === page else { return }
                if ok {
                    self.window?.makeFirstResponder(page)
                } else {
                    self.model?.runCommand("webkit--inputted \(id)\t0")
                }
            }
        }
    }

    /// Follow the page's "next" or "previous" link: `rel` first, then a
    /// link whose text says so.
    private func followRelatedLink(_ which: String, on page: ForgeWebView, id: UInt32) {
        let next = which == "next"
        let pattern = next
            ? #"^\s*(next|next page|older|older posts|more|›|»|>|→)\s*$"#
            : #"^\s*(prev|previous|previous page|newer|newer posts|‹|«|<|←)\s*$"#
        reportScript(
            """
            (function(rel, re) {
              var el = document.querySelector('link[rel~="' + rel + '"][href], a[rel~="' + rel + '"][href]');
              if (!el) {
                var links = document.querySelectorAll("a[href]");
                for (var i = 0; i < links.length; i++) {
                  var t = (links[i].textContent || "").trim() || links[i].getAttribute("aria-label") || "";
                  if (new RegExp(re, "i").test(t)) { el = links[i]; break; }
                }
              }
              if (!el) return false;
              location.href = el.href;
              return true;
            })(\(Self.jsonString(next ? "next" : "prev")), \(Self.jsonString(pattern)));
            """,
            on: page,
            id: id,
            as: "linked"
        )
    }

    private func scroll(_ command: String, on page: ForgeWebView) {
        let script: String
        switch command {
        case "scroll-up": script = "window.scrollBy(0, window.innerHeight);"
        case "scroll-down": script = "window.scrollBy(0, -window.innerHeight);"
        case "scroll-half-up": script = "window.scrollBy(0, window.innerHeight / 2);"
        case "scroll-half-down": script = "window.scrollBy(0, -window.innerHeight / 2);"
        case "scroll-top": script = "window.scrollTo(pageXOffset, 0);"
        case "scroll-bottom":
            script = "window.scrollTo(pageXOffset, document.documentElement.scrollHeight);"
        case let c where c.hasPrefix("scroll-up-px:"):
            script = "window.scrollBy(0, \(Int(c.dropFirst("scroll-up-px:".count)) ?? 0));"
        case let c where c.hasPrefix("scroll-down-px:"):
            script = "window.scrollBy(0, -(\(Int(c.dropFirst("scroll-down-px:".count)) ?? 0)));"
        case let c where c.hasPrefix("scroll-up-line:"):
            script = """
            (function(n) {
              var lh = parseFloat(getComputedStyle(document.body).lineHeight);
              if (!isFinite(lh)) lh = 20;
              window.scrollBy(0, lh * n);
            })(\(Int(c.dropFirst("scroll-up-line:".count)) ?? 1));
            """
        case let c where c.hasPrefix("scroll-down-line:"):
            script = """
            (function(n) {
              var lh = parseFloat(getComputedStyle(document.body).lineHeight);
              if (!isFinite(lh)) lh = 20;
              window.scrollBy(0, -(lh * n));
            })(\(Int(c.dropFirst("scroll-down-line:".count)) ?? 1));
            """
        case let c where c.hasPrefix("scroll-forward:"):
            script = "window.scrollBy((\(Int(c.dropFirst("scroll-forward:".count)) ?? 1)) * 20, 0);"
        case let c where c.hasPrefix("scroll-backward:"):
            script = "window.scrollBy(-((\(Int(c.dropFirst("scroll-backward:".count)) ?? 1)) * 20), 0);"
        default: return
        }
        page.evaluateJavaScript(script)
    }

    private func copySelection(on page: ForgeWebView, id: UInt32) {
        page.evaluateJavaScript("window.getSelection().toString();") { [weak self] result, _ in
            let text = (result as? String) ?? ""
            MainActor.assumeIsolated {
                guard let self, self.webs[id] != nil else { return }
                self.model?.runCommand(
                    "webkit--selection \(id)\t\(Self.jsonString(text))"
                )
            }
        }
    }

    private func reportHistory(on page: ForgeWebView, id: UInt32) {
        let list = page.backForwardList
        var items = list.backList
        if let current = list.currentItem {
            items.append(current)
        }
        items.append(contentsOf: list.forwardList)
        let current = list.currentItem == nil ? 0 : list.backList.count
        let payload: [String: Any] = [
            "current": current,
            "items": items.map { ["title": $0.title ?? "", "url": $0.url.absoluteString] },
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: payload),
              let json = String(data: data, encoding: .utf8)
        else { return }
        model?.runCommand("webkit--history \(id)\t\(json)")
    }

    private func goHistory(_ index: Int, on page: ForgeWebView) {
        let list = page.backForwardList
        var items = list.backList
        if let current = list.currentItem {
            items.append(current)
        }
        items.append(contentsOf: list.forwardList)
        guard items.indices.contains(index) else { return }
        page.go(to: items[index])
    }

    /// Search the page and scroll to the next (or previous) match,
    /// wrapping round; always tell the editor, so it can report both
    /// hits and misses.
    private func find(on page: ForgeWebView, id: UInt32, backwards: Bool) {
        guard webs[id] === page, !page.findText.isEmpty else { return }
        let config = WKFindConfiguration()
        config.backwards = backwards
        config.wraps = true
        config.caseSensitive = page.findText.contains(where: \.isUppercase)
        page.find(page.findText, configuration: config) { [weak self] result in
            guard let self, self.webs[id] != nil else { return }
            MainActor.assumeIsolated {
                self.model?.runCommand("webkit--found \(id)\t\(result.matchFound ? 1 : 0)")
            }
        }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        registerForDraggedTypes([.fileURL])
        if let o = keyObserver {
            NotificationCenter.default.removeObserver(o)
            keyObserver = nil
        }

        if let o = resignObserver {
            NotificationCenter.default.removeObserver(o)
            resignObserver = nil
        }
        if let m = webKeyMonitor {
            NSEvent.removeMonitor(m)
            webKeyMonitor = nil
        }
        guard let window else { return }
        setUpGPU()
        window.makeFirstResponder(self)
        // Keys act on the editor's selected frame, so this window claims
        // the selection whenever it becomes the key window.
        keyObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeKeyNotification,
            object: window,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.windowBecameKey()
                self?.invalidate()
            }
        }
        // The cursor is a solid box in the key window and an outline elsewhere.
        resignObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didResignKeyNotification,
            object: window,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.invalidate() }
        }
        if window.isKeyWindow {
            // Deferred like the grid size: claiming the selection publishes
            // a `@Published` change, which must not happen mid-update.
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated { self?.windowBecameKey() }
            }
        }
        webKeyMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) {
            [weak self] event in
            guard let self, event.window === self.window,
                  let page = self.focusedWeb, page.isEditorKey(event)
            else { return event }
            // Move focus first, so the rest of a prefix (C-x o,
            // M-x, ESC …) reaches the editor as plain keys.
            self.window?.makeFirstResponder(self)
            self.keyDown(with: event)
            return nil
        }
        updateGridSize()
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        ForgeMacOSNamespace.fileURLs(on: sender.draggingPasteboard).isEmpty ? [] : .copy
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let urls = ForgeMacOSNamespace.fileURLs(on: sender.draggingPasteboard)
        guard !urls.isEmpty else { return false }
        for url in urls where url.isFileURL {
            model?.open(path: url.path)
        }
        return true
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        invalidate()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        model?.setDarkMode(effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua)
        invalidate()
    }

    // MARK: GPU

    private var gpuEnabled: Bool {
        UserDefaults.standard.object(forKey: "ForgeGPURendering") as? Bool ?? true
    }

    /// Put the Metal layer under the grid. If Metal is missing or switched
    /// off (`defaults write <bundle id> ForgeGPURendering -bool NO`),
    /// the view keeps drawing with AppKit.
    private func setUpGPU() {
        guard gpu == nil, !gpuTried, gpuEnabled, let window else { return }
        gpuTried = true
        guard let renderer = MetalGridRenderer(scale: window.backingScaleFactor) else { return }
        renderer.view.frame = bounds
        renderer.view.autoresizingMask = [.width, .height]
        addSubview(renderer.view, positioned: .below, relativeTo: nil)
        let overlay = ImageOverlayView(frame: bounds)
        overlay.owner = self
        overlay.autoresizingMask = [.width, .height]
        addSubview(overlay, positioned: .above, relativeTo: renderer.view)
        renderer.onDraw = { [weak self] in self?.drawGPU() }
        gpu = renderer
        imageOverlay = overlay
        invalidate()
    }

    private func gridMetrics(scale: CGFloat) -> GridMetrics {
        GridMetrics(font: font, boldFont: boldFont, italicFont: italicFont,
                    cellSize: cellSize, scale: scale)
    }

    /// Paint the frame onto the Metal layer. If the glyph atlas fills up
    /// halfway, it is emptied and the frame painted again.
    private func drawGPU() {
        guard let gpu else { return }
        let scale = window?.backingScaleFactor ?? gpu.atlas.scale
        if gpu.atlas.scale != scale {
            gpu.atlas.reset(scale: scale)
        }
        for attempt in 0..<2 {
            let canvas = MetalCanvas(atlas: gpu.atlas, metrics: gridMetrics(scale: scale))
            var clear = SIMD4<Float>(0, 0, 0, 1)
            effectiveAppearance.performAsCurrentDrawingAppearance {
                clear = canvas.resolve(Theme.background)
                paint(canvas, dirty: bounds)
            }
            if canvas.overflowed && attempt == 0 {
                gpu.atlas.reset()
                continue
            }
            gpu.present(canvas, clear: clear)
            imageOverlay?.show(canvas.queuedImages)
            return
        }
    }

    /// Where this frame drew things that sit over the text (completion
    /// popups, the key hints, the busy label). Pictures are painted in a
    /// layer above the Metal one, so they must leave these areas alone or
    /// they would hide them.
    var overlayRects: [NSRect] = []

    /// Draw the pictures `MetalCanvas` set aside, with AppKit, over the Metal layer.
    func paintQueuedImages(_ items: [QueuedImage]) {
        let ctx = NSGraphicsContext.current?.cgContext
        for item in items {
            ctx?.saveGState()
            if !overlayRects.isEmpty {
                let path = CGMutablePath()
                path.addRect(bounds.insetBy(dx: -4000, dy: -4000))
                for r in overlayRects { path.addRect(r) }
                ctx?.addPath(path)
                ctx?.clip(using: .evenOdd)
            }
            if let clip = item.clip {
                ctx?.clip(to: clip)
            }
            ctx?.translateBy(x: 0, y: -item.dy)
            gridScale = item.scale
            gridOrigin = item.origin
            drawImage(item.frame, x: item.x, width: item.width, y: item.y)
            gridScale = 1
            gridOrigin = (0, 0)
            ctx?.restoreGState()
        }
    }

    /// The web page whose view holds the keyboard, if any.
    private var focusedWeb: ForgeWebView? {
        var responder = window?.firstResponder
        while let r = responder {
            if let page = r as? ForgeWebView,
               webs.values.contains(where: { $0 === page })
            {
                return page
            }
            responder = r.nextResponder
        }
        return nil
    }

    deinit {
        if let m = webKeyMonitor {
            NSEvent.removeMonitor(m)
        }
        if let o = resignObserver {
            NotificationCenter.default.removeObserver(o)
        }
    }

    /// This window became key: make its frame the editor's selected frame.
    func windowBecameKey() {
        model?.selectFrame(frameId)
    }

    /// The window title follows this frame's buffer, the way the
    /// terminal's title does.
    func updateWindowTitle() {
        guard let f = current else { return }
        let name = f.title.isEmpty ? "Forge" : f.title
        let title = f.modified ? "\(name) — Edited" : name
        if window?.title != title {
            window?.title = title
        }
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        updateGridSize()
        // Live resizes don't always produce a new frame right away; keep
        // the web views glued to the grid while the drag continues.
        syncWeb()
        updateScrollers()
    }

    override func layout() {
        super.layout()
        syncWeb()
        invalidate()
    }

    /// A size waiting to be handed to the editor: SwiftUI resizes this
    /// view while it updates, and the model's `resize` publishes (a
    /// `@Published` change), which SwiftUI forbids during an update. The
    /// size is deferred one runloop pass, and coalesced, so a live resize
    /// sends the editor the last size once the update is over.
    private var pendingGrid: (columns: UInt16, rows: UInt16)?
    private var gridScheduled = false

    private func updateGridSize() {
        model?.setCellSize(width: Double(cellSize.width), height: Double(cellSize.height))
        model?.setDarkMode(effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua)
        let columns = max(1, Int(bounds.width / cellSize.width))
        let rows = max(2, Int(bounds.height / cellSize.height))
        pendingGrid = (UInt16(columns), UInt16(rows))
        guard !gridScheduled else { return }
        gridScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.gridScheduled = false
            guard let size = self.pendingGrid else { return }
            self.pendingGrid = nil
            self.model?.resize(frameId: self.frameId, columns: size.columns, rows: size.rows)
        }
    }

    /// The cells of the window being drawn, and how much bigger than normal
    /// they are (its buffer's text scale): counted from its top-left cell.
    var gridScale: CGFloat = 1
    var gridOrigin: (x: Int, y: Int) = (0, 0)

    /// Draw the next things on the grid of a window whose cells are `scale`
    /// times the normal size, starting at cell (`x`, `y`).
    private func setGrid(scale: CGFloat, x: Int, y: Int) {
        gridScale = scale
        gridOrigin = (x, y)
        out.gridScale = scale
        out.gridOrigin = (x, y)
    }

    private func cellRect(x: Int, y: Int, width: Int) -> NSRect {
        let s = gridScale
        let ox = CGFloat(gridOrigin.x) * cellSize.width
        let oy = CGFloat(gridOrigin.y) * cellSize.height
        return NSRect(x: ox + CGFloat(x - gridOrigin.x) * cellSize.width * s,
                      y: oy + CGFloat(y - gridOrigin.y) * cellSize.height * s,
                      width: CGFloat(width) * cellSize.width * s, height: cellSize.height * s)
    }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        // With a Metal layer underneath there is nothing for AppKit to draw.
        guard gpu == nil else { return }
        paint(AppKitCanvas(view: self), dirty: dirtyRect)
    }

    /// Paint the current frame onto `canvas`, only the grid rows that
    /// overlap `dirtyRect`.
    private func paint(_ target: GridCanvas, dirty dirtyRect: NSRect) {
        out = target
        defer { out = nil }
        out.fill(bounds, Theme.background)
        guard let f = current else { return }

        var bottom = 0
        var cursorDrawn = false
        for w in f.windows {
            let x = Int(w.x), y = Int(w.y)
            // A buffer's text scale draws its window on bigger (or smaller)
            // cells: the rows, columns and cursor the editor sent are
            // counted in those, from the window's corner. The mode line and
            // the border below are on the normal grid.
            let scalePct = max(1, Int(w.cellScalePct))
            let scale = CGFloat(scalePct) / 100
            let dirtyAll = scalePct != 100
            let realTextWidth = Int(w.width) - (w.border ? 1 : 0)
            let textRows = Int(w.height) - 1
            let gridRows = scalePct == 100 ? textRows : max(1, w.rows.count)
            let gridWidth = scalePct == 100 ? Int(w.width) : max(1, Int(w.width) * 100 / scalePct)
            let textWidth = gridWidth - (w.border ? 1 : 0)
            let headerRows = Int(w.headerRows)
            let bodyY = y + headerRows
            setGrid(scale: scale, x: x, y: y)
            let styleByRow = Dictionary(grouping: w.styleRuns, by: { Int($0.row) })
            // The native horizontal scroller covers the last text row of the
            // selected window; it stays put while the text moves.
            let barRow = (w.selected && f.horizontalScrollBar) ? 1 : 0
            let bodyRows = max(0, gridRows - headerRows - barRow)
            let bodyHeight = scalePct == 100
                ? CGFloat(bodyRows) * cellSize.height
                : CGFloat(max(0, textRows - barRow)) * cellSize.height
                    - CGFloat(headerRows) * cellSize.height * scale
            let px = CGFloat(w.scrollYPx)
            let bodyClip = NSRect(x: CGFloat(x) * cellSize.width,
                                  y: cellRect(x: x, y: bodyY, width: 1).minY,
                                  width: CGFloat(realTextWidth) * cellSize.width,
                                  height: bodyHeight)
            // A shown web page is opaque over the text rows: skip painting
            // them (the mode line and border below still draw). This avoids
            // a full grid redraw flashing from under the page.
            let covered: Bool = {
                guard let web = w.web, let view = webs[web.page] else { return false }
                return !view.isHidden
            }()
            if !covered {
                if let col = w.fillColumn {
                    let indicator = faceStyle(face: "fill-column-indicator", syntax: nil)
                    let lineX = cellRect(x: x + Int(col), y: bodyY, width: 1).minX
                    out.fill(NSRect(x: lineX, y: bodyClip.minY, width: 1,
                                    height: bodyHeight), indicator.foreground)
                }
                // The header (Org's) stays put; only the text below it
                // moves by the fractional pixel offset.
                for (i, row) in w.rows.prefix(headerRows).enumerated()
                where dirtyAll || rowIsDirty(y + i, dirtyRect) {
                    drawRun(row, x: x, y: y + i, runs: styleByRow[i] ?? [])
                }
                if px != 0 {
                    out.pushClip(bodyClip, translateY: px)
                }
                for (i, row) in w.rows.dropFirst(headerRows).enumerated() {
                    let rowIndex = i + headerRows
                    // Rows outside the repaint area stay as they are. (With a
                    // pixel offset everything is repainted, so none are skipped.)
                    if px == 0, !dirtyAll, !rowIsDirty(y + rowIndex, dirtyRect) { continue }
                    let hl = w.hlRows.contains(UInt16(rowIndex))
                    if hl {
                        // A package row draws its own selection surface, so
                        // the current-line fill behind it uses the same soft
                        // colour instead of the brighter system one.
                        let packageRow = w.rows[rowIndex].contains {
                            $0.face.hasPrefix("package-")
                        }
                        out.fill(
                            cellRect(x: x + Int(w.gutter), y: y + rowIndex,
                                     width: textWidth - Int(w.gutter)),
                            packageRow ? Theme.packageSelected : Theme.hlLine
                        )
                    }
                    drawRun(row, x: x, y: y + rowIndex, runs: styleByRow[rowIndex] ?? [])
                }
                for im in w.images where Int(im.row) + Int(im.rows) <= gridRows {
                    out.image(im, x: x + Int(w.gutter), width: textWidth - Int(w.gutter), y: y)
                }
                if px != 0 {
                    out.popClip()
                }
            }
            if w.selected {
                let cy = Int(f.cursorY)
                if cy >= bodyY && cy < bodyY + bodyRows {
                    drawCursor(f, offset: px, clip: bodyClip)
                    cursorDrawn = true
                } else if cy >= y && cy < bodyY {
                    drawCursor(f)
                    cursorDrawn = true
                }
            }
            // The border and the mode line are on the normal grid.
            setGrid(scale: 1, x: 0, y: 0)
            if w.border {
                let bx = CGFloat(x + realTextWidth) * cellSize.width + cellSize.width / 2
                out.fill(NSRect(x: bx, y: CGFloat(y) * cellSize.height, width: 1,
                                height: CGFloat(textRows) * cellSize.height), Theme.fillColumn)
            }
            let modeY = y + textRows
            if rowIsDirty(modeY, dirtyRect) {
                out.fill(cellRect(x: x, y: modeY, width: Int(w.width)),
                         w.selected ? Theme.modeLine : Theme.modeLineInactive)
                drawRun(w.modeLine, x: x, y: modeY)
            }
            bottom = max(bottom, y + Int(w.height))
        }

        // Completion popups (corfu), over the text, and the auxiliary
        // popup beside them (corfu-popupinfo).
        overlayRects = []
        for w in f.windows {
            for (p, face) in [(w.popup, "popup"), (w.popupInfo, "popup-info")] {
                guard let p else { continue }
                if !p.rows.isEmpty {
                    overlayRects.append(
                        cellRect(x: Int(p.x), y: Int(p.y), width: Int(p.width))
                            .union(cellRect(x: Int(p.x), y: Int(p.y) + p.rows.count - 1, width: Int(p.width))))
                }
                for (i, row) in p.rows.enumerated() {
                    if let bg = faceStyle(face: face, syntax: nil).background {
                        out.fill(cellRect(x: Int(p.x), y: Int(p.y) + i, width: Int(p.width)), bg)
                    }
                    drawRun(row, x: Int(p.x), y: Int(p.y) + i)
                }
            }
        }
        // Vertical minibuffer completion (vertico): between the windows
        // and the echo line, which moves down.
        for (i, row) in f.completions.enumerated() {
            drawRun(row, x: 0, y: bottom + i)
        }
        let echoY = bottom + f.completions.count

        if !f.hints.isEmpty {
            drawHints(f, above: bottom)
        }
        drawText(f.echo, style: FaceStyle(), x: 0, y: echoY)
        // The cursor rides with the text it sits in; a cursor outside the
        // selected window's body (the echo area) draws unshifted.
        if !cursorDrawn {
            drawCursor(f)
        }
        if let busy = f.busy {
            let label = " \(busy) is busy… (C-g to stop) "
            let style = FaceStyle(foreground: .white, background: .systemOrange)
            let width = label.count
            let columns = Int(bounds.width / cellSize.width)
            drawText(label, style: style, x: max(0, columns - width), y: 0)
            overlayRects.append(cellRect(x: max(0, columns - width), y: 0, width: width))
        }
        drawHover()
    }

    /// Underline the clickable span under the pointer, so links, checkboxes
    /// and agenda entries read as clickable before they are clicked.
    private func drawHover() {
        guard let h = hovered else { return }
        let r = cellRect(x: Int(h.start), y: Int(h.row), width: Int(h.end) - Int(h.start))
        out.fill(r, NSColor.controlAccentColor.withAlphaComponent(0.18))
        out.fill(NSRect(x: r.minX, y: r.maxY - 1, width: r.width, height: 1),
                 NSColor.controlAccentColor)
    }

    /// Pictures already loaded, by path. Org image links show their file
    /// under the link line; a missing or unreadable file draws nothing.
    private var images: [String: NSImage] = [:]

    /// Draw a picture, scaled down to fit (never up) and kept in its
    /// aspect ratio, in the blank rows the editor reserved for it. The
    /// image descriptor's `:width`, `:height`, `:max-width`,
    /// `:max-height`, `:scale`, `:margin`, `:ascent`, `:rotation` and
    /// `:flip` are applied first.
    func drawImage(_ im: ImageFrame, x: Int, width: Int, y: Int) {
        // A canvas (Bases, graph, drawings) arrives as a scene to draw with
        // CoreGraphics, not as a picture.
        if SceneRenderer.isScene(im.path) {
            guard let scene = SceneRenderer.load(im.path),
                  let ctx = NSGraphicsContext.current?.cgContext else { return }
            let cells = cellRect(x: x, y: y + Int(im.row), width: max(1, width))
            let box = NSRect(x: cells.minX, y: cells.minY, width: cells.width,
                             height: CGFloat(im.rows) * cellSize.height * gridScale)
            SceneRenderer.draw(scene, in: box, context: ctx)
            return
        }
        let image: NSImage
        if let cached = images[im.path] {
            image = cached
        } else if let loaded = NSImage(contentsOfFile: im.path) {
            if images.count > 64 { images = images.filter { $0.key == im.path } }
            images[im.path] = loaded
            image = loaded
        } else {
            return
        }
        let d = im.descriptor
        // Target size: :scale, then :width/:height (aspect-preserving when
        // only one is given), then :max-width/:max-height.
        var w = image.size.width
        var h = image.size.height
        // `slice`: show only part of the picture (pixels, or fractions of it).
        var source = NSRect.zero
        if let s = d?.slice, s.count == 4 {
            let whole = image.size
            let pixelsWide = CGFloat(image.representations.map(\.pixelsWide).max() ?? 0)
            let pixelsHigh = CGFloat(image.representations.map(\.pixelsHigh).max() ?? 0)
            let unitX = pixelsWide > 0 ? whole.width / pixelsWide : 1
            let unitY = pixelsHigh > 0 ? whole.height / pixelsHigh : 1
            func size(_ i: Int, _ total: CGFloat, _ unit: CGFloat) -> CGFloat {
                CGFloat(s[i].value) * (s[i].fraction ? total : unit)
            }
            let sx = min(max(0, size(0, whole.width, unitX)), whole.width - 1)
            let sy = min(max(0, size(1, whole.height, unitY)), whole.height - 1)
            let sw = min(max(1, size(2, whole.width, unitX)), whole.width - sx)
            let sh = min(max(1, size(3, whole.height, unitY)), whole.height - sy)
            // The picture's origin is its bottom-left corner; a slice counts from the top.
            source = NSRect(x: sx, y: whole.height - sy - sh, width: sw, height: sh)
            w = sw
            h = sh
        }
        if let s = d?.scale {
            w *= s
            h *= s
        }
        if let width = d?.width, d?.height == nil {
            h *= width / w
            w = width
        }
        if let height = d?.height, d?.width == nil {
            w *= height / h
            h = height
        }
        if let width = d?.width, let height = d?.height {
            w = width
            h = height
        }
        if let mw = d?.maxWidth, w > mw {
            h *= mw / w
            w = mw
        }
        if let mh = d?.maxHeight, h > mh {
            w *= mh / h
            h = mh
        }
        let mx = CGFloat(d?.marginX ?? 0)
        let my = CGFloat(d?.marginY ?? 0)

        let cells = cellRect(x: x, y: y + Int(im.row), width: max(1, width))
        let box = NSRect(
            x: cells.minX, y: cells.minY, width: cells.width,
            height: CGFloat(im.rows) * cellSize.height * gridScale
        ).insetBy(dx: 0, dy: 2)
        guard box.width > 0, box.height > 0, w > 0, h > 0 else { return }
        // Scale down to fit the box (never up), leaving room for the margin.
        let scale = min(1, (box.width - 2 * mx) / w, (box.height - 2 * my) / h)
        let drawn = NSSize(width: w * scale, height: h * scale)
        // Ascent: where the picture sits in its box (centered by default).
        let ascent = CGFloat(d?.ascent ?? 50) / 100
        // Pictures sit at the left edge of the text, unless a
        // plugin asked for its banner to be centred in the window.
        let left = d?.center == true
            ? box.minX + (box.width - drawn.width) / 2
            : box.minX + mx
        let bottom = box.minY + my + (box.height - 2 * my - drawn.height) * (1 - ascent)
        let target = NSRect(x: left, y: bottom, width: drawn.width, height: drawn.height)

        let ctx = NSGraphicsContext.current?.cgContext
        ctx?.saveGState()
        // Rotation around the picture's center, then the horizontal flip.
        let center = CGPoint(x: target.midX, y: target.midY)
        var transform = CGAffineTransform(translationX: center.x, y: center.y)
        transform = transform.rotated(by: CGFloat(d?.rotation ?? 0) * .pi / 180)
        if d?.flipH == true {
            transform = transform.scaledBy(x: -1, y: 1)
        }
        transform = transform.translatedBy(x: -center.x, y: -center.y)
        ctx?.concatenate(transform)
        image.draw(in: target, from: source, operation: .sourceOver, fraction: 1,
                   respectFlipped: true, hints: nil)
        ctx?.restoreGState()
    }

    func attributes(_ style: FaceStyle) -> [NSAttributedString.Key: Any] {
        var attrs: [NSAttributedString.Key: Any] = [
            .font: style.bold ? boldFont : (style.italic ? italicFont : font),
            .foregroundColor: style.foreground,
        ]
        if let u = style.underline {
            attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue
            attrs[.underlineColor] = u
        }
        if style.strike {
            attrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
            attrs[.strikethroughColor] = style.foreground
        }
        return attrs
    }

    /// `runs`: the stretches of this row with their own height or raise, in
    /// columns from `x` (the row's left edge).
    private func drawRun(_ spans: [StyledText], x: Int, y: Int, runs: [StyleRunFrame] = []) {
        var col = x
        for span in spans {
            let style = faceStyle(face: span.face, syntax: span.syntax)
            let rect = cellRect(x: col, y: y, width: Int(span.cells))
            if let bg = style.background {
                out.fill(rect, bg)
            }
            let end = col + Int(span.cells)
            let touched = runs.contains {
                x + Int($0.col) < end && x + Int($0.col) + Int($0.cells) > col
            }
            if touched {
                drawStyled(span.text, from: col, y: y, style: style, runs: runs, x: x)
            } else {
                out.text(span.text, at: rect.origin, style: style)
            }
            col = end
        }
    }

    /// Draw `text` (starting at column `from`) in pieces, each with the
    /// height and raise of the style run it falls in.
    private func drawStyled(
        _ text: String, from: Int, y: Int, style: FaceStyle, runs: [StyleRunFrame], x: Int
    ) {
        var col = from
        var piece = ""
        var pieceStart = from
        var pieceRun: StyleRunFrame?
        func run(at column: Int) -> StyleRunFrame? {
            runs.first { x + Int($0.col) <= column && column < x + Int($0.col) + Int($0.cells) }
        }
        func flush() {
            guard !piece.isEmpty else { return }
            out.text(
                piece, at: cellRect(x: pieceStart, y: y, width: 1).origin, style: style,
                heightPct: Int(pieceRun?.heightPct ?? 100), raisePct: Int(pieceRun?.raisePct ?? 0))
            piece = ""
        }
        for ch in text {
            let r = run(at: col)
            if r != pieceRun {
                flush()
                pieceRun = r
                pieceStart = col
            }
            piece.append(ch)
            col += MetalCanvas.cells(of: ch)
        }
        flush()
    }

    /// A face style with the theme overrides applied (or the built-in
    /// one without a model to ask).
    private func faceStyle(face: String, syntax: String?) -> FaceStyle {
        if let model {
            return model.faceStyle(face: face, syntax: syntax)
        }
        return Theme.style(face: face, syntax: syntax)
    }

    private func drawText(_ text: String, style: FaceStyle, x: Int, y: Int) {
        let rect = cellRect(x: x, y: y, width: text.count)
        if let bg = style.background {
            out.fill(rect, bg)
        }
        out.text(text, at: rect.origin, style: style)
    }

    /// The keys that can follow a prefix (C-x ...), in columns above the
    /// echo area.
    private func drawHints(_ f: Frame, above bottom: Int) {
        let columns = Int(bounds.width / cellSize.width)
        let keyWidth = f.hints.map { $0.key.count }.max() ?? 1
        let cmdWidth = f.hints.map { $0.command.count }.max() ?? 1
        let colWidth = min(keyWidth + cmdWidth + 6, max(columns, 1))
        let perRow = max(1, columns / colWidth)
        let rows = min((f.hints.count + perRow - 1) / perRow, max(1, bottom * 2 / 3))
        let top = bottom - rows - 1
        let hintArea = cellRect(x: 0, y: top, width: columns).union(cellRect(x: 0, y: bottom - 1, width: columns))
        overlayRects.append(hintArea)
        out.fill(hintArea, Theme.background)
        drawText(" \(f.hintPrefix) ", style: FaceStyle(foreground: .secondaryLabelColor, background: Theme.modeLine), x: 0, y: top)
        for (i, hint) in f.hints.prefix(rows * perRow).enumerated() {
            let row = i % rows, col = i / rows
            let x = col * colWidth
            let key = String(repeating: " ", count: max(0, keyWidth - hint.key.count)) + hint.key
            drawText(key, style: FaceStyle(foreground: .systemTeal, bold: true), x: x, y: top + 1 + row)
            let isPrefix = hint.command == "+prefix"
            drawText(" → " + hint.command, style: FaceStyle(foreground: isPrefix ? .systemPurple : .textColor),
                     x: x + keyWidth, y: top + 1 + row)
        }
    }

    private func drawCursor(_ f: Frame, offset: CGFloat = 0, clip: NSRect? = nil) {
        if let clip {
            out.pushClip(clip, translateY: offset)
        }
        var rect = cellRect(x: Int(f.cursorX), y: Int(f.cursorY), width: 1)
        // Modal editing shows its state with the shape (not in the echo area).
        let inEcho = gridScale == 1
            && f.windows.allSatisfy { Int($0.y) + Int($0.height) <= Int(f.cursorY) }
        let shape = inEcho ? "box" : (f.windows.first { $0.selected }?.cursorShape ?? "box")
        switch shape {
        case "bar": rect.size.width = 2
        case "hbar":
            rect.size.height = 2
            rect.origin.y += cellSize.height * gridScale - 2
        default: break
        }
        if window?.isKeyWindow ?? false {
            out.fill(rect, Theme.cursor)
        } else {
            out.outline(rect, Theme.cursor)
        }
        if clip != nil {
            out.popClip()
        }
    }

    // MARK: Keys

    override func keyDown(with event: NSEvent) {
        guard let model else { return }
        pixelAccumulator = 0
        // Command shortcuts the menus don't carry. s-c is Super,
        // which is Command on macOS; any other Command key is for macOS and
        // goes up the responder chain.
        let flags = event.modifierFlags
        if flags.contains(.command), !flags.contains(.control), !flags.contains(.option),
           let chars = event.charactersIgnoringModifiers?.lowercased(), chars.count == 1
        {
            switch chars {
            case "c":
                model.runCommand("kill-ring-save")
                return
            case "v":
                if !model.pasteFromPasteboard() {
                    model.runCommand("yank")
                }
                return
            case "o":
                model.showOpenPanel()
                return
            case "=", "+":
                zoomIn(nil)
                return
            case "-":
                zoomOut(nil)
                return
            case "0":
                zoomReset(nil)
                return
            default:
                break
            }
        }
        guard let input = KeyTranslator.translate(event, optionIsMeta: model.optionIsMeta) else {
            super.keyDown(with: event)
            return
        }
        if case .char(let c) = input.key {
            if input.control && c == "g" && model.isBusy {
                model.interrupt()
            }
            if !input.control && !input.meta {
                model.text(c)
                return
            }
        }
        // S-F10 opens the context menu at point, like a right-click. The
        // editor opens it; the app then shows its own native menu.
        if case .f(let n) = input.key, n == 10, input.shift,
           !input.control, !input.meta
        {
            model.key(input)
            showContextMenuAtCursor()
            return
        }
        model.key(input)
    }

    // MARK: Mouse

    private func cell(of event: NSEvent) -> (UInt16, UInt16) {
        let p = convert(event.locationInWindow, from: nil)
        var x = max(0, Int(p.x / cellSize.width))
        var y = max(0, Int(p.y / cellSize.height))
        // In a window whose text is scaled, a cell is a bigger square: the
        // editor counts rows and columns in those from the window's corner.
        if let w = current?.windows.first(where: {
            $0.cellScalePct != 100
                && x >= Int($0.x) && x < Int($0.x) + Int($0.width)
                && y >= Int($0.y) && y < Int($0.y) + Int($0.height) - 1
        }) {
            let s = CGFloat(w.cellScalePct) / 100
            x = Int(w.x) + max(0, Int((p.x - CGFloat(w.x) * cellSize.width) / (cellSize.width * s)))
            y = Int(w.y) + max(0, Int((p.y - CGFloat(w.y) * cellSize.height) / (cellSize.height * s)))
        }
        return (UInt16(min(x, Int(UInt16.max))), UInt16(min(y, Int(UInt16.max))))
    }

    // MARK: Hover (tooltip-mode, clickable affordance)

    /// The clickable span under the pointer, if any.
    private var hovered: HoverInfo?
    private var hoverTask: Task<Void, Never>?
    private var hoverCell: (UInt16, UInt16) = (UInt16.max, UInt16.max)

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas where area.owner === self {
            removeTrackingArea(area)
        }
        addTrackingArea(NSTrackingArea(
            rect: bounds,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil))
    }

    override func mouseMoved(with event: NSEvent) {
        let (x, y) = cell(of: event)
        if hoverCell == (x, y) { return }
        hoverCell = (x, y)
        // A pointer still inside the same span keeps its highlight and
        // tooltip; only a move out of it asks the editor again.
        if let h = hovered, h.row == y, x >= h.start, x < h.end { return }
        hoverTask?.cancel()
        hoverTask = Task { @MainActor [weak self] in
            guard let self, let model = self.model else { return }
            let info = await model.hover(column: x, row: y)
            if Task.isCancelled { return }
            self.setHovered(info)
        }
    }

    override func mouseExited(with event: NSEvent) {
        hoverTask?.cancel()
        hoverCell = (UInt16.max, UInt16.max)
        setHovered(nil)
    }

    private func setHovered(_ info: HoverInfo?) {
        let old = hovered
        hovered = info
        // The view's own tool tip shows after the usual rest delay.
        let text = info?.tooltip ?? ""
        toolTip = text.isEmpty ? nil : text
        if old?.row != info?.row || old?.start != info?.start || old?.end != info?.end {
            for h in [old, info].compactMap({ $0 }) {
                invalidate(cellRect(x: 0, y: Int(h.row), width: Int(bounds.width / cellSize.width) + 1))
            }
        }
        window?.invalidateCursorRects(for: self)
    }

    /// A pointing hand over anything a click acts on, the I-beam elsewhere.
    override func resetCursorRects() {
        super.resetCursorRects()
        if let h = hovered {
            addCursorRect(cellRect(x: Int(h.start), y: Int(h.row), width: Int(h.end) - Int(h.start)),
                          cursor: .pointingHand)
        }
    }

    override func mouseDown(with event: NSEvent) {
        pixelAccumulator = 0
        window?.makeFirstResponder(self)
        let (x, y) = cell(of: event)
        model?.mouse(.down, column: x, row: y)
    }

    override func mouseDragged(with event: NSEvent) {
        let (x, y) = cell(of: event)
        model?.mouse(.drag, column: x, row: y)
    }

    override func mouseUp(with event: NSEvent) {
        let (x, y) = cell(of: event)
        model?.mouse(.up, column: x, row: y)
    }

    /// A right-click opens the context menu where the pointer is. The
    /// editor decides what it offers (and whether context-menu-mode is
    /// on); the app pops up a native menu from that answer.
    override func rightMouseDown(with event: NSEvent) {
        pixelAccumulator = 0
        window?.makeFirstResponder(self)
        let (x, y) = cell(of: event)
        model?.mouse(.rightDown, column: x, row: y)
        showContextMenu(at: convert(event.locationInWindow, from: nil))
    }

    // MARK: Context menu

    /// Ask the editor for the context menu it just opened and show it as
    /// a native menu. The mouse or key event was queued first, so the
    /// editor's answer is about the click that asked for it.
    private func showContextMenu(at point: NSPoint) {
        Task { @MainActor [weak self] in
            guard let self, let model = self.model,
                  let menu = await model.contextMenu()
            else { return }
            self.contextTarget.model = model
            let native = self.makeContextMenu(menu)
            native.delegate = self
            native.popUp(positioning: nil, at: point, in: self)
        }
    }

    /// S-F10 opens the menu at point, so pop it up at the cursor cell.
    private func showContextMenuAtCursor() {
        guard let f = current else { return }
        showContextMenu(at: cellRect(x: Int(f.cursorX), y: Int(f.cursorY), width: 1).origin)
    }

    /// The editor's context menu as an AppKit menu. Submenus nest one
    /// level, like the editor's own menu registry and the menu bar's.
    private func makeContextMenu(_ menu: ContextMenuFrame) -> NSMenu {
        let native = NSMenu(title: "Context Menu")
        native.autoenablesItems = false
        let column = contextKeyColumn(
            labels: menu.items.map(\.label),
            keys: menu.items.map(\.keys)
        )
        for entry in menu.items {
            if entry.separator {
                native.addItem(.separator())
            } else if entry.submenu.isEmpty {
                native.addItem(contextMenuItem(entry, keyColumn: column))
            } else {
                let item = NSMenuItem()
                item.attributedTitle = NSAttributedString(string: entry.label)
                item.isEnabled = entry.available
                let submenu = NSMenu(title: entry.label)
                submenu.autoenablesItems = false
                let subColumn = contextKeyColumn(
                    labels: entry.submenu.map(\.label),
                    keys: entry.submenu.map(\.keys)
                )
                for child in entry.submenu {
                    if child.separator {
                        submenu.addItem(.separator())
                    } else {
                        submenu.addItem(contextMenuItem(child, keyColumn: subColumn))
                    }
                }
                item.submenu = submenu
                native.addItem(item)
            }
        }
        return native
    }

    private func contextMenuItem(
        _ entry: any MenuCommand,
        keyColumn: CGFloat
    ) -> NSMenuItem {
        let item = NSMenuItem()
        item.target = contextTarget
        item.action = #selector(ContextMenuTarget.runCommand(_:))
        item.representedObject = entry.command
        item.isEnabled = entry.available
        item.state = entry.checked ? .on : .off
        let text = entry.keys.isEmpty ? entry.label : "\(entry.label)\t\(entry.keys)"
        item.attributedTitle = contextRightAligned(text, keyColumn)
        return item
    }

    /// Where the keys column starts: the widest label, a gap, then the
    /// widest set of keys (the menu bar lays its items out the same way).
    private func contextKeyColumn(labels: [String], keys: [String]) -> CGFloat {
        let label = labels.map(contextWidth(of:)).max() ?? 0
        let keys = keys.map(contextWidth(of:)).max() ?? 0
        return label + 20 + keys + 16
    }

    private func contextWidth(of text: String) -> CGFloat {
        NSAttributedString(
            string: text,
            attributes: [.font: NSFont.menuFont(ofSize: 0)]
        ).size().width
    }

    private func contextRightAligned(
        _ text: String,
        _ column: CGFloat
    ) -> NSAttributedString {
        let style = NSMutableParagraphStyle()
        style.tabStops = [
            NSTextTab(textAlignment: .right, location: column, options: [:])
        ]
        return NSAttributedString(
            string: text,
            attributes: [.paragraphStyle: style]
        )
    }

    /// The native menu closed, whether or not an item was chosen: close
    /// the editor's copy too, so it doesn't keep the keyboard.
    func menuDidClose(_ menu: NSMenu) {
        model?.closeContextMenu()
    }

    override func scrollWheel(with event: NSEvent) {
        let (x, y) = cell(of: event)
        // Pixel scrolling moves the text by the fingers' own amount and
        // keeps the remainder between events. Content follows the fingers:
        // dragging down shows earlier text, so the sign is inverted.
        if current?.pixelScrollPrecision == true, event.hasPreciseScrollingDeltas {
            scrollAccumulator = 0
            pixelAccumulator += event.scrollingDeltaY
            let whole = Int(pixelAccumulator)
            if whole != 0 {
                pixelAccumulator -= CGFloat(whole)
                let rowHeight = UInt32(max(1, cellSize.height.rounded()))
                model?.scrollPixels(delta: Int32(-whole), rowHeight: rowHeight)
            }
            return
        }
        pixelAccumulator = 0
        let step = cellSize.height
        scrollAccumulator += event.hasPreciseScrollingDeltas ? event.scrollingDeltaY : event.scrollingDeltaY * cellSize.height
        while abs(scrollAccumulator) >= step {
            // Content follows the fingers: dragging down shows earlier text.
            model?.mouse(scrollAccumulator > 0 ? .scrollUp : .scrollDown, column: x, row: y)
            scrollAccumulator -= scrollAccumulator > 0 ? step : -step
        }
    }
}

/// The object a context menu's items send their actions to.
@MainActor
private final class ContextMenuTarget: NSObject {
    weak var model: EditorModel?

    @objc func runCommand(_ sender: NSMenuItem) {
        guard let command = sender.representedObject as? String else { return }
        model?.runContextCommand(command)
    }
}

/// The web view of `M-x webkit`. Plain keys (including input-method and
/// dead-key compositions) and Command shortcuts belong to the page; a
/// Control key, an Option key when Option-is-Meta is on, or Escape is the
/// editor's, so C-x o, M-x and the rest keep working from inside a page.
///
/// Routing keeps prefixes working: the first key of a prefix (C-x)
/// moves focus to the editor view before it is delivered, so the plain
/// remainder of the sequence (o) reaches the editor too. `EditorNSView`'s
/// local key monitor does this before WKWebView's private content view can
/// swallow the event; these overrides remain as a fallback. The same event
/// is never delivered twice: `performKeyEquivalent` and `keyDown` can both
/// see it, so the first one to claim it records its number and the other
/// stands down.
@MainActor
final class ForgeWebView: WKWebView {
    var onEditorKey: ((NSEvent) -> Void)?
    /// A link-hint prompt is showing in the page: Escape cancels it
    /// instead of going to the editor.
    var hintsActive = false
    var onHintsEscape: (() -> Void)?
    var hintsHandler: HintsHandler?
    /// Kept alive with the view: the script handler, and the KVO tokens.
    var hover: HoverHandler?
    var observers: [NSKeyValueObservation] = []
    /// The text of the last search, for next and previous.
    var findText = ""
    /// Mirrors Settings > Use Option as Meta. Off, Option types (é, ü, …).
    var optionIsMeta = true
    /// The last event already forwarded to the editor, so the second
    /// entry point (`keyDown` after `performKeyEquivalent`, or reverse)
    /// ignores it.
    private var forwardedEventNumber: Int?

    func isEditorKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection([.command, .control, .option])
        // Command (and Command combinations) stay with the page and the
        // menus: copy, paste, find, back/forward shortcuts.
        if flags.contains(.command) { return false }
        if event.keyCode == 53 { return true } // Escape, bare or modified
        if flags.contains(.control) { return true }
        return optionIsMeta && flags.contains(.option)
    }

    private func forwardOnce(_ event: NSEvent) -> Bool {
        if forwardedEventNumber == event.eventNumber { return false }
        forwardedEventNumber = event.eventNumber
        onEditorKey?(event)
        return true
    }

    override func keyDown(with event: NSEvent) {
        if hintsActive && event.keyCode == 53 {
            onHintsEscape?()
            return
        }
        if isEditorKey(event) {
            // Already claimed via performKeyEquivalent: don't send twice.
            guard forwardOnce(event) else { return }
        } else {
            super.keyDown(with: event)
        }
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if hintsActive && event.keyCode == 53 {
            onHintsEscape?()
            return true
        }
        if isEditorKey(event) {
            guard forwardOnce(event) else { return true }
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
}

/// Loading, titles and errors for one page, plus the policy for where a
/// navigation may go. Weak to the editor view: torn down with the page.
@MainActor
final class WebNavDelegate: NSObject, WKNavigationDelegate, WKUIDelegate {
    private let id: UInt32
    private weak var owner: EditorNSView?

    init(id: UInt32, owner: EditorNSView) {
        self.id = id
        self.owner = owner
    }

    func webView(
        _ webView: WKWebView,
        decidePolicyFor action: WKNavigationAction
    ) async -> WKNavigationActionPolicy {
        guard let url = action.request.url, let scheme = url.scheme?.lowercased() else {
            return .allow // about:blank and friends
        }
        guard ["http", "https", "file", "about", "data", "blob"].contains(scheme) else {
            return .cancel
        }
        // Loading feedback for the main frame only: subframes go through
        // here too, and must not echo a "Loading" per ad slot.
        if action.targetFrame == nil || action.targetFrame?.isMainFrame == true {
            owner?.webDidStart(id, url: url.absoluteString)
        }
        return .allow
    }

    func webView(
        _ webView: WKWebView,
        decidePolicyFor response: WKNavigationResponse
    ) async -> WKNavigationResponsePolicy {
        .allow
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        // Loading is reported from decidePolicyFor (which has the new URL
        // and the target frame); by the time this fires webView.url is
        // still the old page, and it also fires for subframes.
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        owner?.webDidFinish(id)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        owner?.webDidFinish(id)
    }

    private func fail(_ webView: WKWebView, _ error: Error) {
        if Self.isBenignNavigationError(error as NSError) {
            // A stop or a superseded load, not a failure to report.
            return
        }
        let ns = error as NSError
        owner?.webDidFail(
            id, url: webView.url?.absoluteString ?? "",
            message: ns.localizedDescription)
    }

    /// Stops (`NSURLErrorCancelled`) and superseded loads (WebKit's
    /// "frame load interrupted") report errors that aren't failures.
    /// The latter's legacy constant isn't exposed to Swift, so it
    /// compares by domain and code (`WebKitErrorDomain`, 102).
    private static func isBenignNavigationError(_ ns: NSError) -> Bool {
        if ns.domain == NSURLErrorDomain, ns.code == NSURLErrorCancelled {
            return true
        }
        return ns.domain == "WebKitErrorDomain" && ns.code == 102
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        fail(webView, error)
    }

    func webView(
        _ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
        withError error: Error
    ) {
        fail(webView, error)
    }

    func webView(
        _ webView: WKWebView, didReceive challenge: URLAuthenticationChallenge
    ) async -> (URLSession.AuthChallengeDisposition, URLCredential?) {
        (.performDefaultHandling, nil)
    }

    // A link asking for a new window (target=_blank) opens in the same
    // page: there is only one view per page.
    func webView(
        _ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
        for action: WKNavigationAction, windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        if action.targetFrame == nil, let url = action.request.url {
            webView.load(URLRequest(url: url))
        }
        return nil
    }
}

/// Receives the link under the pointer from the page's script.
final class HintsHandler: NSObject, WKScriptMessageHandler {
    private let onChoice: @MainActor (String, String) -> Void

    init(_ onChoice: @escaping @MainActor (String, String) -> Void) {
        self.onChoice = onChoice
    }

    func userContentController(
        _ controller: WKUserContentController, didReceive message: WKScriptMessage
    ) {
        let body = message.body as? [String: Any]
        let kind = body?["kind"] as? String ?? "cancel"
        let url = body?["url"] as? String ?? ""
        MainActor.assumeIsolated { onChoice(kind, url) }
    }
}

@MainActor
final class HoverHandler: NSObject, WKScriptMessageHandler {
    private let onLink: @MainActor (String) -> Void

    init(_ onLink: @escaping @MainActor (String) -> Void) {
        self.onLink = onLink
    }

    func userContentController(
        _ controller: WKUserContentController, didReceive message: WKScriptMessage
    ) {
        let link = message.body as? String ?? ""
        MainActor.assumeIsolated { onLink(link) }
    }
}


/// Draws a frame straight to the view with AppKit: the fallback when Metal
/// is unavailable or switched off.
@MainActor
final class AppKitCanvas: GridCanvas {
    private unowned let view: EditorNSView

    init(view: EditorNSView) {
        self.view = view
    }

    var gridScale: CGFloat {
        get { view.gridScale }
        set { view.gridScale = newValue }
    }
    var gridOrigin: (x: Int, y: Int) {
        get { view.gridOrigin }
        set { view.gridOrigin = newValue }
    }

    func fill(_ rect: NSRect, _ color: NSColor) {
        color.setFill()
        rect.fill()
    }

    func outline(_ rect: NSRect, _ color: NSColor) {
        color.setStroke()
        NSBezierPath(rect: rect.insetBy(dx: 0.5, dy: 0.5)).stroke()
    }

    func text(_ string: String, at origin: NSPoint, style: FaceStyle, heightPct baseHeightPct: Int, raisePct: Int) {
        let heightPct = Int((Double(baseHeightPct) * Double(gridScale)).rounded())
        var attrs = view.attributes(style)
        var at = origin
        if heightPct != 100, let font = attrs[.font] as? NSFont {
            let scaled = font.scaled(by: heightPct)
            attrs[.font] = scaled
            // Keep the baseline where it was, then lift by the raise.
            at.y += font.ascender - scaled.ascender
        }
        at.y -= CGFloat(raisePct) / 100 * view.font.pointSize * CGFloat(heightPct) / 100
        (string as NSString).draw(at: at, withAttributes: attrs)
    }

    func image(_ frame: ImageFrame, x: Int, width: Int, y: Int) {
        view.drawImage(frame, x: x, width: width, y: y)
    }

    func pushClip(_ rect: NSRect, translateY: CGFloat) {
        let ctx = NSGraphicsContext.current?.cgContext
        ctx?.saveGState()
        ctx?.clip(to: rect)
        ctx?.translateBy(x: 0, y: -translateY)
    }

    func popClip() {
        NSGraphicsContext.current?.cgContext.restoreGState()
    }
}
