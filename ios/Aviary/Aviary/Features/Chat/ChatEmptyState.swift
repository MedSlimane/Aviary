//
//  ChatEmptyState.swift
//  Aviary
//

import SwiftUI

struct ChatEmptyState: View {
    let suggestions: [Suggestion]
    let onPick: (Suggestion) -> Void

    var body: some View {
        VStack(spacing: 0) {
            Spacer()

            AviaryMark()
                .frame(width: 76, height: 76)

            Text("What should we shape today?")
                .font(.title2.weight(.bold))
                .foregroundStyle(Ink.primary)
                .multilineTextAlignment(.center)
                .padding(.top, 20)
                .padding(.horizontal, 32)

            Spacer()

            VStack(alignment: .leading, spacing: 4) {
                ForEach(suggestions) { suggestion in
                    Button {
                        onPick(suggestion)
                    } label: {
                        HStack(spacing: 14) {
                            Image(systemName: suggestion.systemImage)
                                .font(.system(size: 17, weight: .regular))
                                .frame(width: 22)
                            Text(suggestion.title)
                                .font(.body)
                            Spacer(minLength: 0)
                        }
                        .foregroundStyle(Ink.secondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 12)
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 16)
            // Clears the composer.
            .padding(.bottom, 88)
        }
    }
}

#Preview {
    ChatEmptyState(
        suggestions: [
            Suggestion(title: "Search my library", systemImage: "magnifyingglass"),
            Suggestion(title: "Run a bundle", systemImage: "cube"),
            Suggestion(title: "Check MCP health", systemImage: "server.rack"),
        ],
        onPick: { _ in }
    )
    .background { AuroraBackground() }
}
