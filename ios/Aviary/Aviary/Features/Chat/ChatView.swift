//
//  ChatView.swift
//  Aviary
//

import SwiftUI

struct ChatView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ZStack(alignment: .bottom) {
            if model.conversation.isEmpty {
                ChatEmptyState(suggestions: model.suggestions) { suggestion in
                    model.draft = suggestion.title
                }
            } else {
                ChatTranscript(messages: model.conversation.messages)
            }

            Composer(
                text: Binding(get: { model.draft }, set: { model.draft = $0 }),
                onSend: { model.send() }
            )
            .padding(.horizontal, 16)
            .padding(.bottom, 8)
        }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Menu", systemImage: "line.3.horizontal") {
                    model.openDrawer()
                }
            }
            ToolbarItem(placement: .principal) {
                ConversationTitle(
                    title: model.conversation.runner == .claudeCode ? "Aviary" : model.conversation.runner.rawValue,
                    model: model.conversation.model
                )
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button("New chat", systemImage: "square.and.pencil") {
                    model.newConversation()
                }
            }
        }
    }
}

/// Centre title with the model picker affordance.
private struct ConversationTitle: View {
    let title: String
    let model: String

    var body: some View {
        Button {
            // Model picker is not wired up yet.
        } label: {
            HStack(spacing: 4) {
                Text(title)
                    .font(.headline)
                    .foregroundStyle(Ink.primary)
                Image(systemName: "chevron.down")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Ink.tertiary)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Model: \(model). Change model")
    }
}

private struct ChatTranscript: View {
    let messages: [Message]

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                ForEach(messages) { message in
                    MessageView(message: message)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            // Keeps the last turn clear of the composer.
            .padding(.bottom, 96)
        }
        .scrollEdgeSoftIfAvailable()
    }
}

private extension View {
    /// iOS 26 soft scroll-edge effect; no-op below.
    @ViewBuilder
    func scrollEdgeSoftIfAvailable() -> some View {
        if #available(iOS 26, *) {
            self.scrollEdgeEffectStyle(.soft, for: .top)
        } else {
            self
        }
    }
}

#Preview("Conversation") {
    NavigationStack {
        ChatView()
            .background { AuroraBackground() }
    }
    .environment(AppModel())
}

#Preview("Empty") {
    NavigationStack {
        ChatView()
            .background { AuroraBackground() }
    }
    .environment(AppModel(conversation: Conversation(title: "New chat", messages: [])))
}
