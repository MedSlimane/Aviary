//
//  Chat.swift
//  Aviary
//

import Foundation

enum Runner: String, Hashable, CaseIterable {
    case claudeCode = "Claude Code"
    case codex = "Codex"
}

struct Conversation: Identifiable, Hashable {
    let id: UUID
    var title: String
    var messages: [Message]
    var runner: Runner
    var model: String

    init(
        id: UUID = UUID(),
        title: String,
        messages: [Message],
        runner: Runner = .claudeCode,
        model: String = "Sonnet 5"
    ) {
        self.id = id
        self.title = title
        self.messages = messages
        self.runner = runner
        self.model = model
    }

    var isEmpty: Bool { messages.isEmpty }
}

struct Message: Identifiable, Hashable {
    enum Role: Hashable {
        case user
        case assistant
    }

    let id: UUID
    let role: Role
    var text: String
    /// Streaming "thinking" steps shown above an assistant turn.
    var steps: [ThinkingStep]
    /// Footer such as "Used 3 tools · gmail, calendar, memory".
    var toolSummary: String?

    init(
        id: UUID = UUID(),
        role: Role,
        text: String,
        steps: [ThinkingStep] = [],
        toolSummary: String? = nil
    ) {
        self.id = id
        self.role = role
        self.text = text
        self.steps = steps
        self.toolSummary = toolSummary
    }
}

struct ThinkingStep: Identifiable, Hashable {
    enum Tone: Hashable {
        case violet, peach, coral, teal
    }

    let id: UUID
    let label: String
    let tone: Tone

    init(id: UUID = UUID(), label: String, tone: Tone) {
        self.id = id
        self.label = label
        self.tone = tone
    }
}

// MARK: - Sample data

extension Conversation {
    static let sample = Conversation(
        title: "Frontend Review",
        messages: [
            Message(role: .user, text: "Review my inbox and highlight anything that needs my attention today."),
            Message(
                role: .assistant,
                text: """
                Three things need you today. The Vercel invoice failed to charge and \
                retries stop Friday. Maya is blocked on the auth migration decision — \
                she asked twice. And the design review you moved twice is now \
                double-booked against the board prep.
                """,
                steps: [
                    ThinkingStep(label: "Thinking…", tone: .violet),
                    ThinkingStep(label: "Understanding your inbox…", tone: .peach),
                    ThinkingStep(label: "Performing actions…", tone: .coral),
                ],
                toolSummary: "Used 3 tools · gmail, calendar, memory"
            ),
        ]
    )

    static let samplePinned = [
        Conversation(title: "Frontend Review", messages: []),
        Conversation(title: "Auth migration", messages: []),
    ]

    static let sampleRecents = [
        Conversation(title: "Inbox triage for today", messages: []),
        Conversation(title: "Why did Codex skip the tests?", messages: [], runner: .codex, model: "gpt-5"),
        Conversation(title: "Draft the billing runbook", messages: []),
        Conversation(title: "Compare Sonnet vs Opus cost", messages: []),
    ]
}
