//
//  Theme.swift
//  Aviary
//
//  Values mirror assets/tokens.json (dark ramp) and the Figma variable
//  collection `Aviary/Color`. Keep the three in sync.
//

import SwiftUI

extension Color {
    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }
}

/// Aviary semantic colours, dark ramp.
enum Ink {
    static let canvas = Color(hex: 0x090A0D)
    static let surface = Color(hex: 0x101116)
    static let elevated = Color(hex: 0x181A21)

    static let borderSubtle = Color(hex: 0x242630)
    static let borderStrong = Color(hex: 0x383B48)

    static let primary = Color(hex: 0xF4F4F6)
    static let secondary = Color(hex: 0xB5B7C0)
    static let tertiary = Color(hex: 0x7B7E8A)

    static let violet = Color(hex: 0x8D7AE8)
    static let blue = Color(hex: 0x75B9F0)
    static let teal = Color(hex: 0x5EEAD4)
    static let peach = Color(hex: 0xF6A75D)
    static let coral = Color(hex: 0xE66B66)
    static let gold = Color(hex: 0xFFD68F)

    static let ok = Color(hex: 0x4ADE80)
    static let warn = Color(hex: 0xFBBF24)
    static let error = Color(hex: 0xF87171)

    /// Runner identity colours.
    static let claude = Color(hex: 0xD97757)
    static let codex = Color(hex: 0xE8E8EA)
}

/// Corner radii from tokens.json.
enum Radius {
    static let xs: CGFloat = 4
    static let sm: CGFloat = 8
    static let md: CGFloat = 12
    static let lg: CGFloat = 16
    static let xl: CGFloat = 24
    static let xxl: CGFloat = 32
}

// MARK: - Glass

extension View {
    /// Liquid Glass on iOS 26+, material + hairline on iOS 18–25.
    ///
    /// `Glass` itself is iOS 26-only, so it never appears in this signature —
    /// callers stay version-agnostic.
    @ViewBuilder
    func glassSurface(
        in shape: some Shape,
        tint: Color? = nil,
        interactive: Bool = false
    ) -> some View {
        if #available(iOS 26, *) {
            self.glassEffect(.glass(tint: tint, interactive: interactive), in: shape)
        } else {
            self
                .background(.ultraThinMaterial, in: shape)
                .overlay(shape.stroke(.white.opacity(0.16), lineWidth: 1))
        }
    }
}

@available(iOS 26, *)
extension Glass {
    static func glass(tint: Color?, interactive: Bool) -> Glass {
        var glass = Glass.regular
        if let tint { glass = glass.tint(tint) }
        if interactive { glass = glass.interactive() }
        return glass
    }
}

/// Circular 44pt glass control used for the drawer, compose and back buttons.
struct GlassCircleButton: View {
    let systemImage: String
    let accessibilityLabel: LocalizedStringKey
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(Ink.primary)
                .frame(width: 44, height: 44)
                .contentShape(.circle)
        }
        .buttonStyle(.plain)
        .glassSurface(in: .circle, interactive: true)
        .accessibilityLabel(accessibilityLabel)
    }
}
