//
//  RootView.swift
//  Aviary
//
//  Chat is the root surface; the drawer slides over it.
//
//  Layering note: the aurora is the *screen's* background, applied inside the
//  NavigationStack. Putting it behind the stack instead leaves it visible only
//  in the safe-area insets, because the navigation container paints its own
//  opaque background over the content area.
//

import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    /// Live drag translation. Local view state on purpose — it changes every
    /// frame, so it must never reach the model or the environment.
    @GestureState private var dragTranslation: CGFloat = 0

    private let preferredDrawerWidth: CGFloat = 336

    var body: some View {
        GeometryReader { proxy in
            let width = min(preferredDrawerWidth, proxy.size.width * 0.86)
            let offset = drawerOffset(width: width)
            let progress = width > 0 ? (offset + width) / width : 0

            ZStack(alignment: .leading) {
                // Owns the safe-area insets. A navigation stack's content area
                // excludes them *and* clips, so neither `ignoresSafeArea` nor
                // negative padding on the screen's own layer can reach them —
                // the result is a black bar under the composer. This layer sits
                // outside the stack, where it can. It renders the same field at
                // the same mood, so the boundary is continuous.
                AuroraBackground(mood: model.path.last?.mood ?? .aurora)

                content
                    // Chat slides and recedes as the drawer comes in.
                    .offset(x: progress * width * 0.92)
                    .scaleEffect(1 - progress * 0.04, anchor: .trailing)
                    .clipShape(.rect(cornerRadius: progress > 0.01 ? 38 : 0))
                    .overlay {
                        if progress > 0.01 {
                            Rectangle()
                                .fill(.black.opacity(0.4 * progress))
                                .ignoresSafeArea()
                                .allowsHitTesting(model.isDrawerOpen)
                                .onTapGesture { model.closeDrawer() }
                                .accessibilityLabel("Close menu")
                                .accessibilityAddTraits(.isButton)
                        }
                    }

                DrawerPane(
                    pinned: model.pinned,
                    recents: model.recents,
                    active: model.path.last,
                    onSelect: { model.go(to: $0) },
                    onNewChat: { model.newConversation() }
                )
                .frame(width: width)
                .offset(x: offset)
            }
            .animation(.snappy(duration: 0.34, extraBounce: 0.02), value: model.isDrawerOpen)
            .gesture(drawerDrag(width: width))
        }
        .background(Ink.canvas)
    }

    private var content: some View {
        NavigationStack(path: Binding(get: { model.path }, set: { model.path = $0 })) {
            Screen(mood: .aurora) {
                ChatView()
            }
            .navigationDestination(for: Destination.self) { destination in
                Screen(mood: destination.mood) {
                    destinationView(destination)
                }
            }
        }
    }

    @ViewBuilder
    private func destinationView(_ destination: Destination) -> some View {
        switch destination {
        case .library:
            LibraryView(entries: model.libraryEntries)
        case .servers:
            MCPServersView(servers: model.servers)
        case .bundles, .context, .inspiration:
            ComingSoonView(destination: destination)
        }
    }

    private func drawerOffset(width: CGFloat) -> CGFloat {
        let base: CGFloat = model.isDrawerOpen ? 0 : -width
        return max(-width, min(0, base + dragTranslation))
    }

    /// Edge pull to open, drag anywhere to close.
    private func drawerDrag(width: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 12, coordinateSpace: .local)
            .updating($dragTranslation) { value, state, _ in
                guard abs(value.translation.width) > abs(value.translation.height) else { return }
                if model.isDrawerOpen {
                    state = min(0, value.translation.width)
                } else if value.startLocation.x < 40 {
                    state = max(0, value.translation.width)
                }
            }
            .onEnded { value in
                guard abs(value.translation.width) > abs(value.translation.height) else { return }
                // Predicted end makes a flick feel right.
                let predicted = value.predictedEndTranslation.width
                if model.isDrawerOpen {
                    if predicted < -width * 0.35 { model.closeDrawer() }
                } else if value.startLocation.x < 40, predicted > width * 0.35 {
                    model.openDrawer()
                }
            }
    }
}

// MARK: - Screen background

/// A screen sitting on the animated gradient.
///
/// The gradient is a **ZStack sibling**, not a `.background`. Two constraints
/// forced that:
/// * A navigation stack paints an opaque background over its content area, so a
///   gradient placed *behind* the whole stack survives only in the safe-area
///   insets — you get the content blacked out.
/// * A `.background` is sized to its host, whose bounds stop at the safe area,
///   so `ignoresSafeArea` inside it cannot bleed — you get black bars top and
///   bottom instead.
///
/// A sibling inside the screen can expand into the enclosing insets, which
/// covers both cases with one instance of the shader.
private struct Screen<Content: View>: View {
    let mood: AuroraMood
    @ViewBuilder var content: Content

    var body: some View {
        ZStack {
            // Covers the content area. `RootView` draws the same field behind
            // the navigation stack to cover the insets it cannot reach.
            AuroraBackground(mood: mood)
            content
        }
        .scrollContentBackground(.hidden)
        .toolbarBackgroundVisibility(.hidden, for: .navigationBar)
    }
}

struct ComingSoonView: View {
    let destination: Destination

    var body: some View {
        ContentUnavailableView {
            Label(destination.title, systemImage: destination.systemImage)
        } description: {
            Text("This surface is designed but not built yet.")
        }
        .navigationTitle(destination.title)
    }
}

#Preview {
    RootView()
        .environment(AppModel())
}
