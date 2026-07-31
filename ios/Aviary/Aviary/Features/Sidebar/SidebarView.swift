//
//  SidebarView.swift
//  Aviary
//
//  The drawer. Structure is a native sidebar `List` so row metrics, section
//  headers, selection highlight, swipe actions and Dynamic Type all come from
//  UIKit rather than being re-approximated with stacks.
//

import SwiftUI

/// Glass pane + sidebar list. The sliding container lives in `RootView`.
struct DrawerPane: View {
    let pinned: [Conversation]
    let recents: [Conversation]
    let active: Destination?
    let onSelect: (Destination) -> Void
    let onNewChat: () -> Void

    var body: some View {
        SidebarView(
            pinned: pinned,
            recents: recents,
            active: active,
            onSelect: onSelect,
            onNewChat: onNewChat
        )
        .drawerGlass()
    }
}

private extension View {
    /// Liquid Glass pane on iOS 26; material on iOS 18–25. The pane samples the
    /// chat's aurora sitting behind it, which is the whole point of the effect.
    @ViewBuilder
    func drawerGlass() -> some View {
        if #available(iOS 26, *) {
            self.background {
                Rectangle()
                    .fill(.clear)
                    .glassEffect(.regular, in: .rect)
                    .ignoresSafeArea()
            }
        } else {
            self.background {
                Rectangle()
                    .fill(.ultraThinMaterial)
                    .ignoresSafeArea()
            }
        }
    }
}

struct SidebarView: View {
    let pinned: [Conversation]
    let recents: [Conversation]
    let active: Destination?
    let onSelect: (Destination) -> Void
    let onNewChat: () -> Void

    @State private var query = ""

    private var filteredRecents: [Conversation] {
        guard !query.isEmpty else { return recents }
        return recents.filter { $0.title.localizedCaseInsensitiveContains(query) }
    }

    private var filteredPinned: [Conversation] {
        guard !query.isEmpty else { return pinned }
        return pinned.filter { $0.title.localizedCaseInsensitiveContains(query) }
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(Destination.allCases) { destination in
                        Button {
                            onSelect(destination)
                        } label: {
                            Label(destination.title, systemImage: destination.systemImage)
                        }
                        .listRowBackground(rowBackground(isActive: active == destination))
                    }
                }

                if !filteredPinned.isEmpty {
                    Section("Pinned") {
                        ForEach(filteredPinned) { conversation in
                            Button {
                                // Opening a stored conversation needs a store.
                            } label: {
                                Label(conversation.title, systemImage: "folder")
                            }
                            .listRowBackground(rowBackground(isActive: false))
                        }
                    }
                }

                if !filteredRecents.isEmpty {
                    Section("Recents") {
                        ForEach(filteredRecents) { conversation in
                            Button {
                                // Opening a stored conversation needs a store.
                            } label: {
                                Text(conversation.title)
                                    .foregroundStyle(Ink.secondary)
                            }
                            .listRowBackground(rowBackground(isActive: false))
                        }
                    }
                }
            }
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)
            .navigationTitle("Aviary")
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search chats")
            .toolbarBackgroundVisibility(.hidden, for: .navigationBar)
            .safeAreaInset(edge: .bottom) { footer }
        }
        .tint(Ink.violet)
    }

    /// Transparent rows so the glass pane reads through; active gets a tint.
    private func rowBackground(isActive: Bool) -> some View {
        Group {
            if isActive {
                RoundedRectangle(cornerRadius: 12)
                    .fill(Ink.violet.opacity(0.22))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
            } else {
                Color.clear
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 12) {
            Button(action: onNewChat) {
                Label("New chat", systemImage: "square.and.pencil")
                    .font(.subheadline.weight(.semibold))
            }
            .buttonStyle(.prominentGlassIfAvailable)

            Spacer()

            Button("Settings", systemImage: "gearshape") {
                // Settings screen is not built yet.
            }
            .labelStyle(.iconOnly)
            .font(.system(size: 18))
            .buttonStyle(.glassIfAvailable)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        // The list scrolls behind this bar, so it needs its own surface —
        // otherwise rows read straight through the buttons. Kept as a solid
        // scrim rather than glass: the pane behind is already glass, and glass
        // cannot sample glass.
        //
        // Must be a plain `background(_:)`. Wrapping an expanding `Color` in a
        // stack here asks for infinite height inside the `safeAreaInset`
        // measurement, which is unresolvable and collapses the whole screen.
        .background(Ink.canvas.opacity(0.88))
        .overlay(alignment: .top) {
            Divider().overlay(Ink.borderSubtle)
        }
    }
}

// MARK: - Glass button styles with fallbacks

extension PrimitiveButtonStyle where Self == GlassFallbackButtonStyle {
    static var glassIfAvailable: GlassFallbackButtonStyle { .init(prominent: false) }
    static var prominentGlassIfAvailable: GlassFallbackButtonStyle { .init(prominent: true) }
}

/// Routes to `.glass` / `.glassProminent` on iOS 26, `.bordered` variants below.
struct GlassFallbackButtonStyle: PrimitiveButtonStyle {
    let prominent: Bool

    func makeBody(configuration: Configuration) -> some View {
        if #available(iOS 26, *) {
            if prominent {
                Button(configuration).buttonStyle(.glassProminent).controlSize(.large)
            } else {
                Button(configuration).buttonStyle(.glass).controlSize(.large)
            }
        } else {
            if prominent {
                Button(configuration).buttonStyle(.borderedProminent).controlSize(.large)
            } else {
                Button(configuration).buttonStyle(.bordered).controlSize(.large)
            }
        }
    }
}

#Preview {
    ZStack(alignment: .leading) {
        AuroraBackground()
        DrawerPane(
            pinned: Conversation.samplePinned,
            recents: Conversation.sampleRecents,
            active: .library,
            onSelect: { _ in },
            onNewChat: {}
        )
        .frame(width: 336)
    }
}
