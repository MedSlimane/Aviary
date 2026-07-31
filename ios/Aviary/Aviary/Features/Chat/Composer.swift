//
//  Composer.swift
//  Aviary
//

import SwiftUI

/// Glass input pill: attach · field · mic · voice/send.
struct Composer: View {
    @Binding var text: String
    let onSend: () -> Void

    @FocusState private var isFocused: Bool

    private var hasDraft: Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        // Glass cannot sample other glass; the container gives the pill and the
        // voice button one shared sampling region.
        GlassGroup(spacing: 10) {
            pill
        }
    }

    private var pill: some View {
        HStack(spacing: 10) {
            Button("Attach", systemImage: "plus") {
                // Attachment sheet is not wired up yet.
            }
            .labelStyle(.iconOnly)
            .font(.system(size: 20, weight: .medium))
            .foregroundStyle(Ink.primary.opacity(0.85))
            .frame(width: 28, height: 28)

            TextField("Ask Aviary", text: $text, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.body)
                .foregroundStyle(Ink.primary)
                .tint(Ink.violet)
                .lineLimit(1...5)
                .focused($isFocused)
                .submitLabel(.send)
                .onSubmit(onSend)

            if !hasDraft {
                Button("Dictate", systemImage: "mic") {
                    // Dictation is not wired up yet.
                }
                .labelStyle(.iconOnly)
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(Ink.primary.opacity(0.85))
                .frame(width: 26, height: 26)
                .transition(.scale.combined(with: .opacity))
            }

            Button(action: onSend) {
                Image(systemName: hasDraft ? "arrow.up" : "waveform")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(Color(hex: 0x140B22))
                    .frame(width: 40, height: 40)
                    .background(Ink.violet, in: .circle)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(hasDraft ? "Send" : "Voice mode")
        }
        .padding(.leading, 14)
        .padding(.trailing, 8)
        .padding(.vertical, 8)
        .glassSurface(in: .capsule)
        .animation(.snappy(duration: 0.22), value: hasDraft)
        .sensoryFeedback(.impact(weight: .light), trigger: hasDraft)
    }
}

/// `GlassEffectContainer` on iOS 26, plain passthrough below.
struct GlassGroup<Content: View>: View {
    var spacing: CGFloat
    @ViewBuilder var content: Content

    var body: some View {
        if #available(iOS 26, *) {
            GlassEffectContainer(spacing: spacing) { content }
        } else {
            content
        }
    }
}

#Preview {
    @Previewable @State var text = ""

    VStack {
        Spacer()
        Composer(text: $text, onSend: {})
            .padding(16)
    }
    .background { AuroraBackground() }
}
