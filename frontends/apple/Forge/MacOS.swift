// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Native macOS services used by Forge and other app integrations.

import AppKit
import QuickLookUI
import UserNotifications

struct ForgeMacOSNamespace {
    private let editor: Forge

    init(editor: Forge) {
        self.editor = editor
    }

    var textPasteboardTypes: [String] {
        editor.macosPasteboardTypes()
    }

    func readPasteboardText(_ pasteboard: NSPasteboard = .general) -> String? {
        Self.readPasteboardText(pasteboard)
    }

    @discardableResult
    func writePasteboardText(_ text: String, to pasteboard: NSPasteboard = .general) -> Bool {
        Self.writePasteboardText(text, to: pasteboard)
    }

    func revealInFinder(_ url: URL) {
        Self.revealInFinder(url)
    }

    @MainActor
    func quickLook(_ url: URL) {
        Self.quickLook(url)
    }

    @MainActor
    @discardableResult
    func notify(title: String, body: String) async -> Bool {
        await Self.notify(title: title, body: body)
    }

    static func revealInFinder(_ url: URL) {
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    @MainActor
    static func quickLook(_ url: URL) {
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        ForgeQuickLook.shared.show(url)
    }

    @MainActor
    @discardableResult
    static func notify(title: String, body: String) async -> Bool {
        let center = UNUserNotificationCenter.current()
        do {
            let settings = await center.notificationSettings()
            if settings.authorizationStatus == .notDetermined {
                guard try await center.requestAuthorization(options: [.alert, .sound]) else {
                    return false
                }
            } else if settings.authorizationStatus != .authorized
                        && settings.authorizationStatus != .provisional {
                return false
            }
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            try await center.add(
                UNNotificationRequest(
                    identifier: UUID().uuidString,
                    content: content,
                    trigger: nil
                )
            )
            return true
        } catch {
            NSLog("Forge notification unavailable: %@", String(describing: error))
            return false
        }
    }

    static func readPasteboardText(_ pasteboard: NSPasteboard = .general) -> String? {
        if let text = pasteboard.string(forType: .string) {
            return text
        }
        for type in [NSPasteboard.PasteboardType.rtf, .html] {
            guard let data = pasteboard.data(forType: type) else { continue }
            let documentType: NSAttributedString.DocumentType = type == .rtf ? .rtf : .html
            if let text = try? NSAttributedString(
                data: data,
                options: [.documentType: documentType],
                documentAttributes: nil
            ).string {
                return text
            }
        }
        return nil
    }

    @discardableResult
    static func writePasteboardText(_ text: String, to pasteboard: NSPasteboard = .general) -> Bool {
        let value = NSAttributedString(string: text)
        let item = NSPasteboardItem()
        item.setString(text, forType: .string)
        for (type, documentType) in [
            (NSPasteboard.PasteboardType.rtf, NSAttributedString.DocumentType.rtf),
            (.html, .html),
        ] {
            if let data = try? value.data(
                from: NSRange(location: 0, length: value.length),
                documentAttributes: [.documentType: documentType]
            ) {
                item.setData(data, forType: type)
            }
        }
        pasteboard.clearContents()
        return pasteboard.writeObjects([item])
    }

    static func fileURLs(on pasteboard: NSPasteboard) -> [URL] {
        pasteboard.readObjects(
            forClasses: [NSURL.self],
            options: [.urlReadingFileURLsOnly: true]
        ) as? [URL] ?? []
    }
}

@MainActor
private final class ForgeQuickLook: NSObject, QLPreviewPanelDataSource {
    static let shared = ForgeQuickLook()
    private var item: ForgePreviewItem?

    func show(_ url: URL) {
        item = ForgePreviewItem(url: url)
        guard let panel = QLPreviewPanel.shared() else { return }
        panel.dataSource = self
        panel.reloadData()
        panel.makeKeyAndOrderFront(nil)
    }

    func numberOfPreviewItems(in panel: QLPreviewPanel) -> Int {
        item == nil ? 0 : 1
    }

    func previewPanel(_ panel: QLPreviewPanel, previewItemAt index: Int) -> QLPreviewItem {
        item!
    }
}

private final class ForgePreviewItem: NSObject, QLPreviewItem {
    let url: URL

    init(url: URL) {
        self.url = url
    }

    var previewItemURL: URL? { url }
    var previewItemTitle: String? { url.lastPathComponent }
}

@MainActor
final class ForgeServices: NSObject {
    private let openText: (String) -> Void

    init(openText: @escaping (String) -> Void) {
        self.openText = openText
    }

    @objc func forgeOpenSelection(
        _ pasteboard: NSPasteboard,
        userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString?>
    ) {
        guard let text = ForgeMacOSNamespace.readPasteboardText(pasteboard), !text.isEmpty else {
            error.pointee = "No text selection was provided"
            return
        }
        openText(text)
    }
}
