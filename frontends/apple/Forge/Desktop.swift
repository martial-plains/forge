// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Cross-platform app operations shared by desktop frontends.

import AppKit

struct ForgeDesktopNamespace {
    private let editor: Forge

    init(editor: Forge) {
        self.editor = editor
    }

    @discardableResult
    func open(url: URL) -> Bool {
        editor.openUrl(url: url.absoluteString)
    }
}

extension Forge {
    var desktop: ForgeDesktopNamespace {
        ForgeDesktopNamespace(editor: self)
    }

    var macos: ForgeMacOSNamespace {
        ForgeMacOSNamespace(editor: self)
    }
}
