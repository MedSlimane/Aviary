//
//  AppModel.swift
//  Aviary
//

import Foundation
import Observation

/// Destinations reachable from the drawer. Chat is not a destination — it is the
/// root surface the drawer slides over.
enum Destination: Hashable, CaseIterable, Identifiable {
    case library
    case bundles
    case servers
    case context
    case inspiration

    var id: Self { self }

    var title: String {
        switch self {
        case .library: "Library"
        case .bundles: "Bundles"
        case .servers: "MCP Servers"
        case .context: "Context"
        case .inspiration: "Inspiration"
        }
    }

    var systemImage: String {
        switch self {
        case .library: "square.stack.3d.up"
        case .bundles: "cube"
        case .servers: "server.rack"
        case .context: "clock"
        case .inspiration: "sparkles"
        }
    }

    var mood: AuroraMood {
        switch self {
        case .library, .bundles: .dusk
        case .servers, .context: .tidal
        case .inspiration: .aurora
        }
    }
}

@MainActor
@Observable
final class AppModel {
    // MARK: Navigation

    /// Drawer offset is owned by the view (it changes every frame while dragging);
    /// only the settled state lives here.
    var isDrawerOpen = false
    var path: [Destination] = []

    // MARK: Chat

    var conversation: Conversation
    var draft = ""

    var pinned: [Conversation]
    var recents: [Conversation]

    // MARK: Content

    let libraryEntries: [LibraryEntry]
    let servers: [MCPServer]

    init(
        conversation: Conversation = .sample,
        pinned: [Conversation] = Conversation.samplePinned,
        recents: [Conversation] = Conversation.sampleRecents,
        libraryEntries: [LibraryEntry] = LibraryEntry.sample,
        servers: [MCPServer] = MCPServer.sample
    ) {
        self.conversation = conversation
        self.pinned = pinned
        self.recents = recents
        self.libraryEntries = libraryEntries
        self.servers = servers
    }

    // MARK: Intents

    func openDrawer() { isDrawerOpen = true }
    func closeDrawer() { isDrawerOpen = false }

    func go(to destination: Destination) {
        isDrawerOpen = false
        path = [destination]
    }

    func newConversation() {
        isDrawerOpen = false
        path.removeAll()
        conversation = Conversation(title: "New chat", messages: [])
        draft = ""
    }

    /// Appends the draft as a user turn. No backend yet — the assistant reply is
    /// left to the caller so this stays synchronous and testable.
    func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        conversation.messages.append(Message(role: .user, text: text))
        draft = ""
    }

    var suggestions: [Suggestion] {
        [
            Suggestion(title: "Search my library", systemImage: "magnifyingglass"),
            Suggestion(title: "Run a bundle", systemImage: "cube"),
            Suggestion(title: "Check MCP health", systemImage: "server.rack"),
        ]
    }
}

struct Suggestion: Identifiable, Hashable {
    let title: String
    let systemImage: String
    var id: String { title }
}

#if DEBUG
extension AppModel {
    /// Lets a UI state be driven from the command line, for screenshots and UI
    /// tests that would otherwise need to synthesise taps:
    ///
    ///     xcrun simctl launch <device> dev.aviary.ios -uiState drawer
    ///
    /// Recognised values: `empty`, `drawer`, `library`, `servers`.
    static func fromLaunchArguments() -> AppModel {
        let arguments = ProcessInfo.processInfo.arguments
        guard
            let flag = arguments.firstIndex(of: "-uiState"),
            let state = arguments[safe: flag + 1]
        else {
            return AppModel()
        }

        let model = AppModel()
        switch state {
        case "empty": model.conversation = Conversation(title: "New chat", messages: [])
        case "drawer": model.isDrawerOpen = true
        case "library": model.path = [.library]
        case "servers": model.path = [.servers]
        default: break
        }
        return model
    }
}

private extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
#endif
