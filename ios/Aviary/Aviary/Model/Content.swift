//
//  Content.swift
//  Aviary
//
//  Library entries and MCP servers.
//

import Foundation
import SwiftUI

struct LibraryEntry: Identifiable, Hashable {
    enum Kind: String, Hashable {
        case skill = "Skill"
        case agent = "Agent"
        case prompt = "Prompt"
        case command = "Command"

        var systemImage: String {
            switch self {
            case .skill: "sparkles"
            case .agent: "circle.dashed"
            case .prompt: "text.bubble"
            case .command: "command"
            }
        }

        var tint: Color {
            switch self {
            case .skill: Ink.violet
            case .agent: Ink.teal
            case .prompt: Ink.peach
            case .command: Ink.blue
            }
        }
    }

    let id: UUID
    let name: String
    let summary: String
    let kind: Kind
    let pack: String
    let runner: Runner

    init(
        id: UUID = UUID(),
        name: String,
        summary: String,
        kind: Kind,
        pack: String,
        runner: Runner
    ) {
        self.id = id
        self.name = name
        self.summary = summary
        self.kind = kind
        self.pack = pack
        self.runner = runner
    }
}

extension LibraryEntry {
    static let sample: [LibraryEntry] = [
        .init(name: "design-taste-frontend", summary: "Visual language rules for dense product UI", kind: .skill, pack: "Personal", runner: .claudeCode),
        .init(name: "brandkit", summary: "Generate a brand system from a single seed", kind: .skill, pack: "Personal", runner: .claudeCode),
        .init(name: "graphify", summary: "Any input → knowledge graph", kind: .skill, pack: "Personal", runner: .claudeCode),
        .init(name: "brainstorming", summary: "Shape a rough idea into a spec", kind: .skill, pack: "Superpowers", runner: .claudeCode),
        .init(name: "Explore", summary: "Read-only fan-out search agent", kind: .agent, pack: "Superpowers", runner: .codex),
        .init(name: "review-checklist", summary: "Frontend review prompt", kind: .prompt, pack: "Superpowers", runner: .codex),
    ]

    /// Packs in stable display order, precomputed so no grouping happens in `body`.
    static func grouped(_ entries: [LibraryEntry]) -> [(pack: String, entries: [LibraryEntry])] {
        var order: [String] = []
        var buckets: [String: [LibraryEntry]] = [:]
        for entry in entries {
            if buckets[entry.pack] == nil { order.append(entry.pack) }
            buckets[entry.pack, default: []].append(entry)
        }
        return order.map { ($0, buckets[$0] ?? []) }
    }
}

struct MCPServer: Identifiable, Hashable {
    enum Health: Hashable {
        case ok(toolCount: Int, transport: String)
        case failed(reason: String)
        case disabled

        var tint: Color {
            switch self {
            case .ok: Ink.ok
            case .failed: Ink.error
            case .disabled: Ink.warn
            }
        }

        var detail: String {
            switch self {
            case let .ok(toolCount, transport): "\(toolCount) tools · \(transport)"
            case let .failed(reason): reason
            case .disabled: "disabled for this runner"
            }
        }
    }

    let id: UUID
    let name: String
    let health: Health
    var isEnabled: Bool

    init(id: UUID = UUID(), name: String, health: Health, isEnabled: Bool) {
        self.id = id
        self.name = name
        self.health = health
        self.isEnabled = isEnabled
    }
}

extension MCPServer {
    static let sample: [MCPServer] = [
        .init(name: "figma", health: .ok(toolCount: 24, transport: "stdio"), isEnabled: true),
        .init(name: "playwright", health: .ok(toolCount: 12, transport: "stdio"), isEnabled: true),
        .init(name: "github", health: .ok(toolCount: 31, transport: "http"), isEnabled: true),
        .init(name: "postgres", health: .failed(reason: "handshake failed"), isEnabled: false),
        .init(name: "linear", health: .ok(toolCount: 18, transport: "http"), isEnabled: true),
        .init(name: "filesystem", health: .disabled, isEnabled: false),
    ]
}
