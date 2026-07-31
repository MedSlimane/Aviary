//
//  AviaryMark.swift
//  Aviary
//
//  The brand mark — a six-point soft spark, from assets/brand/aviary-mark.svg.
//  Shipped as a template vector asset and filled by `markIridescence`.
//

import SwiftUI

struct AviaryMark: View {
    /// Set false for a flat tint (toolbars, small chrome).
    var animated = true

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        GeometryReader { proxy in
            if animated && !reduceMotion {
                TimelineView(.animation) { context in
                    MarkCanvas(size: proxy.size, time: Self.elapsed(context.date))
                }
            } else {
                MarkCanvas(size: proxy.size, time: Self.frozenTime)
            }
        }
        .accessibilityLabel("Aviary")
    }

    private static let frozenTime: Double = 1.6
    private static let epoch = Date.timeIntervalSinceReferenceDate

    private static func elapsed(_ date: Date) -> Double {
        date.timeIntervalSinceReferenceDate - epoch
    }
}

/// POD view so the per-frame `time` change doesn't invalidate anything above it.
private struct MarkCanvas: View, Equatable {
    let size: CGSize
    let time: Double

    var body: some View {
        Image(.aviaryMark)
            .renderingMode(.template)
            .resizable()
            .scaledToFit()
            .foregroundStyle(.white)
            .frame(width: size.width, height: size.height)
            .colorEffect(
                ShaderLibrary.markIridescence(
                    .float2(Float(size.width), Float(size.height)),
                    .float(Float(time))
                )
            )
    }

    nonisolated static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.size == rhs.size && Int(lhs.time * 60) == Int(rhs.time * 60)
    }
}

#Preview {
    ZStack {
        AuroraBackground()
        AviaryMark()
            .frame(width: 96, height: 96)
    }
}
