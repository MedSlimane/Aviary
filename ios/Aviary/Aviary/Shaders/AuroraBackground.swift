//
//  AuroraBackground.swift
//  Aviary
//

import SwiftUI

/// Which palette family leads the gradient. Maps to the `hueShift` uniform.
enum AuroraMood: Float {
    case aurora = 0      // violet crown, blue body — Chat / Home
    case dusk = 0.55     // magenta + rose — Library
    case tidal = 1.1     // teal + gold — MCP / system surfaces
}

/// Full-bleed animated gradient driven by `AuroraField.metal`.
///
/// The animation is a GPU `colorEffect`, so a frame costs one full-screen
/// fragment pass and **no SwiftUI body re-evaluation** below `AuroraCanvas`.
/// `TimelineView` re-runs only this subtree — keep it as a background layer and
/// never wrap app content in it.
struct AuroraBackground: View {
    var mood: AuroraMood = .aurora
    var intensity: Float = 1

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        GeometryReader { proxy in
            // Paused timeline still renders one frame, so reduce-motion users get
            // the full gradient — just frozen.
            TimelineView(.animation(paused: reduceMotion)) { context in
                AuroraCanvas(
                    size: proxy.size,
                    time: reduceMotion ? Self.frozenTime : Self.elapsed(context.date),
                    mood: mood,
                    intensity: intensity
                )
            }
        }
        .ignoresSafeArea()
        .accessibilityHidden(true)
    }

    /// A pleasant fixed point in the loop for the reduce-motion case.
    private static let frozenTime: Double = 4.2

    /// Reference-date seconds are ~7.9e8 and climbing; `Float` has 24 bits of
    /// mantissa, so passing them straight to the shader quantises `time` into
    /// visible ~60 s steps. Anchor to first use to keep the value small.
    private static let epoch = Date.timeIntervalSinceReferenceDate

    private static func elapsed(_ date: Date) -> Double {
        date.timeIntervalSinceReferenceDate - epoch
    }
}

/// POD view: `memcmp`-fast diffing, and it isolates the per-frame `time` change
/// so nothing above it is invalidated.
private struct AuroraCanvas: View, Equatable {
    let size: CGSize
    let time: Double
    let mood: AuroraMood
    let intensity: Float

    var body: some View {
        Rectangle()
            .fill(.black)
            .colorEffect(
                ShaderLibrary.auroraField(
                    .float2(Float(size.width), Float(size.height)),
                    .float(Float(time)),
                    .float(intensity),
                    .float(mood.rawValue)
                )
            )
    }

    /// Quantise time to ~120 fps. Guards against redundant GPU passes if the
    /// timeline ever fires faster than the display can present.
    ///
    /// `nonisolated` because `View` is `@MainActor` under Swift 6 while
    /// `Equatable.==` is not — SwiftUI may compare views off the main actor.
    nonisolated static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.size == rhs.size
            && lhs.mood == rhs.mood
            && lhs.intensity == rhs.intensity
            && Int(lhs.time * 120) == Int(rhs.time * 120)
    }
}

#Preview("Aurora") {
    AuroraBackground(mood: .aurora)
}

#Preview("Tidal") {
    AuroraBackground(mood: .tidal)
}
