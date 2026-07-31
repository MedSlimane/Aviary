//
//  LibraryView.swift
//  Aviary
//

import SwiftUI

struct LibraryView: View {
    let entries: [LibraryEntry]

    @State private var query = ""
    @State private var kindFilter: LibraryEntry.Kind?

    /// Filtering happens here rather than in `body` so the list identity is
    /// stable and no work is repeated per re-render.
    private var groups: [(pack: String, entries: [LibraryEntry])] {
        LibraryEntry.grouped(filtered)
    }

    private var filtered: [LibraryEntry] {
        entries.filter { entry in
            let matchesKind = kindFilter == nil || entry.kind == kindFilter
            let matchesQuery = query.isEmpty
                || entry.name.localizedCaseInsensitiveContains(query)
                || entry.summary.localizedCaseInsensitiveContains(query)
            return matchesKind && matchesQuery
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                Text("\(entries.count) entries · Claude Code and Codex")
                    .font(.subheadline)
                    .foregroundStyle(Ink.secondary)

                SearchField(text: $query, prompt: "Name, description, or path")

                FilterChips(selection: $kindFilter)

                ForEach(groups, id: \.pack) { group in
                    VStack(alignment: .leading, spacing: 8) {
                        Text(group.pack.uppercased())
                            .font(.caption2.weight(.semibold))
                            .kerning(0.7)
                            .foregroundStyle(Ink.tertiary)
                            .padding(.leading, 4)

                        VStack(spacing: 0) {
                            ForEach(Array(group.entries.enumerated()), id: \.element.id) { index, entry in
                                LibraryRow(entry: entry)
                                if index < group.entries.count - 1 {
                                    Divider().overlay(Ink.borderSubtle)
                                        .padding(.leading, 60)
                                }
                            }
                        }
                        .background(Ink.elevated.opacity(0.7), in: .rect(cornerRadius: Radius.lg))
                        .overlay(
                            RoundedRectangle(cornerRadius: Radius.lg)
                                .stroke(Ink.borderSubtle, lineWidth: 1)
                        )
                    }
                }

                if filtered.isEmpty {
                    ContentUnavailableView.search(text: query)
                        .padding(.top, 40)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 32)
        }
        .navigationTitle("Library")
        .navigationBarTitleDisplayMode(.large)
    }
}

struct SearchField: View {
    @Binding var text: String
    let prompt: String

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(Ink.tertiary)
            TextField(prompt, text: $text)
                .textFieldStyle(.plain)
                .font(.body)
                .foregroundStyle(Ink.primary)
                .tint(Ink.violet)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            if !text.isEmpty {
                Button("Clear", systemImage: "xmark.circle.fill") { text = "" }
                    .labelStyle(.iconOnly)
                    .foregroundStyle(Ink.tertiary)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .glassSurface(in: .rect(cornerRadius: 22))
    }
}

private struct FilterChips: View {
    @Binding var selection: LibraryEntry.Kind?

    private static let kinds: [LibraryEntry.Kind?] = [
        nil, .skill, .agent, .prompt, .command,
    ]

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(Array(Self.kinds.enumerated()), id: \.offset) { _, kind in
                    let isSelected = kind == selection
                    Button {
                        selection = kind
                    } label: {
                        Text(kind?.rawValue.appending("s") ?? "All")
                            .font(.caption.weight(.medium))
                            .foregroundStyle(isSelected ? Color(hex: 0x140B22) : Ink.secondary)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 8)
                    }
                    .buttonStyle(.plain)
                    .background {
                        if isSelected {
                            Capsule().fill(Ink.violet)
                        }
                    }
                    .glassSurfaceWhenUnselected(isSelected)
                }
            }
        }
        .scrollIndicators(.hidden)
    }
}

private extension View {
    @ViewBuilder
    func glassSurfaceWhenUnselected(_ isSelected: Bool) -> some View {
        if isSelected {
            self
        } else {
            self.glassSurface(in: .capsule, interactive: true)
        }
    }
}

private struct LibraryRow: View {
    let entry: LibraryEntry

    var body: some View {
        Button {
            // Entry detail/editor is not built yet.
        } label: {
            HStack(spacing: 12) {
                Image(systemName: entry.kind.systemImage)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(entry.kind.tint)
                    .frame(width: 32, height: 32)
                    .background(.white.opacity(0.07), in: .rect(cornerRadius: 10))

                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.name)
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(Ink.primary)
                    Text(entry.summary)
                        .font(.caption)
                        .foregroundStyle(Ink.secondary)
                        .lineLimit(2)
                }

                Spacer(minLength: 0)

                Circle()
                    .fill(entry.runner == .claudeCode ? Ink.claude : Ink.codex)
                    .frame(width: 8, height: 8)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(entry.name), \(entry.kind.rawValue). \(entry.summary). Runner \(entry.runner.rawValue)")
    }
}

#Preview {
    NavigationStack {
        LibraryView(entries: LibraryEntry.sample)
            .background { AuroraBackground(mood: .dusk) }
    }
}
