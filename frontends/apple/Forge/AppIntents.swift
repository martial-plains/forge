// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Shortcuts actions use App Intents and require no signing capability.

import AppIntents
import AppKit

struct ForgeOpenDocumentIntent: AppIntent {
    static let title: LocalizedStringResource = "Open Document in Forge"
    static var openAppWhenRun = true

    @Parameter(title: "Document")
    var file: IntentFile

    func perform() async throws -> some IntentResult {
        guard let fileURL = file.fileURL else {
            throw URLError(.badURL)
        }
        var components = URLComponents()
        components.scheme = "forge"
        components.host = "open"
        components.queryItems = [URLQueryItem(name: "file", value: fileURL.path)]
        guard let url = components.url else { throw URLError(.badURL) }
        let opened = await MainActor.run { NSWorkspace.shared.open(url) }
        guard opened else { throw URLError(.cannotOpenFile) }
        return .result()
    }
}

struct ForgeAppShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: ForgeOpenDocumentIntent(),
            phrases: ["Open a document in \(.applicationName)"],
            shortTitle: "Open Document",
            systemImageName: "doc.text"
        )
    }
}
