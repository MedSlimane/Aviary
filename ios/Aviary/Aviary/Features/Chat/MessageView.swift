//
//  MessageView.swift
//  Aviary
//

import SwiftUI

struct MessageView: View {
    let message: Message

    var body: some View {
        switch message.role {
        case .user:
            HStack {
                Spacer(minLength: 40)
                Text(message.text)
                    .font(.subheadline)
                    .foregroundStyle(Color(hex: 0x0C0917))
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
                    .background(Ink.violet.opacity(0.92), in: .rect(cornerRadius: 22))
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("You said: \(message.text)")

        case .assistant:
            VStack(alignment: .leading, spacing: 12) {
                ForEach(message.steps) { step in
                    ThinkingPill(step: step)
                }

                if !message.text.isEmpty {
                    Text(message.text)
                        .font(.subheadline)
                        .foregroundStyle(Ink.primary.opacity(0.92))
                        .fixedSize(horizontal: false, vertical: true)
                }

                if let summary = message.toolSummary {
                    Text(summary)
                        .font(.caption)
                        .foregroundStyle(Ink.tertiary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
        }
    }
}

/// Glass pill with a breathing dot, shown while the agent works.
struct ThinkingPill: View {
    let step: ThinkingStep

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isBreathing = false

    var body: some View {
        HStack(spacing: 9) {
            Circle()
                .fill(tint)
                .frame(width: 14, height: 14)
                .scaleEffect(isBreathing ? 1 : 0.78)
                .opacity(isBreathing ? 1 : 0.65)

            Text(step.label)
                .font(.subheadline.weight(.medium))
                .foregroundStyle(Ink.primary)
        }
        .padding(.leading, 10)
        .padding(.trailing, 16)
        .padding(.vertical, 9)
        .glassSurface(in: .capsule)
        .animation(
            reduceMotion ? nil : .easeInOut(duration: 1.1).repeatForever(autoreverses: true),
            value: isBreathing
        )
        .onAppear {
            guard !reduceMotion else { return }
            isBreathing = true
        }
        .accessibilityLabel(step.label)
    }

    private var tint: Color {
        switch step.tone {
        case .violet: Ink.violet
        case .peach: Ink.peach
        case .coral: Ink.coral
        case .teal: Ink.teal
        }
    }
}

#Preview {
    VStack(alignment: .leading, spacing: 16) {
        ForEach(Conversation.sample.messages) { MessageView(message: $0) }
    }
    .padding()
    .background { AuroraBackground() }
}
