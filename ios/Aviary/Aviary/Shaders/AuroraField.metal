//
//  AuroraField.metal
//  Aviary
//
//  A flowing iridescent gradient field used as the app background.
//
//  Design constraints that shaped this implementation:
//  * It runs on every frame, full screen (up to ~3.2M fragments on a Pro Max at
//    120 Hz), so it deliberately avoids fbm/value-noise. Layered sine fields give
//    the same organic drift for roughly an order of magnitude less ALU work.
//  * Colours come from assets/tokens.json (the Aviary gradient families) so the
//    shader stays in sync with the Figma design system.
//

#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>

using namespace metal;

namespace aviary {

/// Three rotated, counter-drifting sine waves plus a radial term.
/// Returns roughly -1...1 with no sharp features, so blending stays smooth.
static inline float field(float2 p, float t, float phase) {
    float a = sin(p.x * 1.15 + t * 0.62 + phase);
    float b = sin(p.y * 1.63 - t * 0.48 + phase * 1.7);
    float c = sin((p.x + p.y) * 0.87 + t * 0.81 + phase * 0.6);
    float d = sin(length(p) * 1.42 - t * 0.55 + phase * 2.3);
    return (a + b + c + d) * 0.25;
}

/// Smooth 0...1 weight centred on `centre`, used to fade each colour in and out.
static inline float weight(float v, float centre, float spread) {
    float x = (v - centre) / spread;
    return exp(-x * x);
}

} // namespace aviary

/// Dark canvas with an animated glow bleeding up from the bottom edge.
///
/// Composition follows the Gemini/ChatGPT treatment rather than a full-screen
/// wash: the top of the screen stays at the near-black canvas colour so body
/// text sits on flat dark, and the chroma rises from the bottom, peaking around
/// the composer. Because the glow is **additive** over the canvas, the top
/// converges exactly to `bg/canvas` — which means the status-bar inset needs no
/// special handling and no seam is visible.
///
/// - Parameters:
///   - size: view size in points, used to normalise `position`.
///   - time: seconds; drives the drift. Hold it constant to freeze the field.
///   - intensity: scales the glow. 0 leaves a flat canvas.
///   - hueShift: rotates the palette (0 violet/blue, 1 magenta, 2 teal).
[[ stitchable ]] half4 auroraField(float2 position,
                                   half4 color,
                                   float2 size,
                                   float time,
                                   float intensity,
                                   float hueShift) {
    float2 uv = position / max(size, float2(1.0));
    float aspect = size.x / max(size.y, 1.0);

    // Work in a space where y = 0 is the bottom edge, so the glow maths reads
    // in the same direction as the effect.
    float2 p = float2((uv.x - 0.5) * aspect, 1.0 - uv.y);

    float t = time * 0.35;

    // --- vertical bleed -----------------------------------------------------
    // Rises from the bottom edge and is fully extinguished well before the top.
    // `smoothstep` needs edge0 < edge1, so ramp on uv.y (0 = top, 1 = bottom).
    float rise = pow(smoothstep(0.30, 1.0, uv.y), 1.25);

    // --- drifting hot spots -------------------------------------------------
    // Two slow blobs sitting low on the screen, roughly behind the composer.
    float2 c1 = float2(sin(t * 0.51) * 0.34, 0.16);
    float2 c2 = float2(cos(t * 0.37) * 0.46 + 0.18, 0.02);
    float2 d1 = p - c1;
    float2 d2 = p - c2;
    float g1 = exp(-dot(d1, d1) * 3.1);
    float g2 = exp(-dot(d2, d2) * 2.2);

    // Gentle breathing so it never looks like a static gradient.
    float breathe = 0.86 + 0.14 * sin(t * 0.83);

    float glow = rise * (0.26 + 0.62 * g1 + 0.48 * g2) * breathe;

    // --- colour -------------------------------------------------------------
    // A slow field selects along the brand ramp; hueShift rotates the family.
    float sel = aviary::field(p * 1.15, t * 0.9, 0.0) * 0.5 + hueShift;

    const half3 indigo = half3(0.153, 0.133, 0.318); // #272251 dusk.1
    const half3 violet = half3(0.553, 0.478, 0.910); // #8D7AE8
    const half3 skyBlue = half3(0.459, 0.725, 0.941); // #75B9F0
    const half3 magenta = half3(0.702, 0.306, 0.471); // #B34E78
    const half3 teal = half3(0.235, 0.608, 0.604);   // #3C9B9A

    half3 accum = half3(0.0);
    half total = 0.0h;

    struct Stop { half3 rgb; float centre; float spread; };
    const Stop stops[] = {
        { indigo,  -0.85, 0.55 },
        { violet,  -0.25, 0.48 },
        { skyBlue,  0.30, 0.48 },
        { magenta,  0.85, 0.50 },
        { teal,     1.45, 0.55 },
    };
    for (uint i = 0; i < 5; ++i) {
        half w = half(aviary::weight(sel, stops[i].centre, stops[i].spread));
        accum += stops[i].rgb * w;
        total += w;
    }
    half3 tint = accum / max(total, 0.0001h);

    // Keep it saturated while dark, so the bleed reads as colour not haze.
    half luma = dot(tint, half3(0.2126h, 0.7152h, 0.0722h));
    tint = clamp(mix(half3(luma), tint, 1.5h), 0.0h, 1.0h);

    // --- composite ----------------------------------------------------------
    const half3 canvas = half3(0.035, 0.039, 0.051); // #090A0D bg/canvas
    half3 rgb = canvas + tint * half(glow * clamp(intensity, 0.0, 1.0)) * 0.34h;
    rgb = clamp(rgb, 0.0h, 1.0h);

    return half4(rgb * color.a, color.a);
}

/// Iridescent fill for the Aviary mark.
///
/// Applied with `colorEffect` to a **template** image, so `color.a` is the
/// glyph's coverage mask: the shader paints only the mark and leaves the
/// surrounding pixels clear. Deliberately more saturated and faster-moving than
/// `auroraField` — it is a 56pt logo, not a full-screen wash.
[[ stitchable ]] half4 markIridescence(float2 position,
                                       half4 color,
                                       float2 size,
                                       float time) {
    float2 uv = position / max(size, float2(1.0));

    // Sweep diagonally across the glyph, with a slow counter-rotation so the
    // six petals don't all peak at once.
    float t = time * 0.55;
    float sweep = uv.x * 0.75 + uv.y * 0.55;
    float wobble = sin((uv.x - uv.y) * 5.5 + t * 1.35) * 0.14;
    float phase = fract(sweep + t * 0.22 + wobble);

    // Vivid brand ramp, wrapping so the loop is seamless.
    const half3 violet = half3(0.553, 0.478, 0.910); // #8D7AE8
    const half3 blue    = half3(0.459, 0.725, 0.941); // #75B9F0
    const half3 teal    = half3(0.369, 0.918, 0.831); // #5EEAD4
    const half3 gold    = half3(1.000, 0.839, 0.561); // #FFD68F
    const half3 rose    = half3(0.902, 0.420, 0.520); // #E66B85

    const half3 ramp[6] = { violet, blue, teal, gold, rose, violet };

    float scaled = phase * 5.0;
    uint index = uint(clamp(scaled, 0.0, 4.999));
    half localT = half(fract(scaled));
    half3 rgb = mix(ramp[index], ramp[index + 1], smoothstep(0.0h, 1.0h, localT));

    // Specular lift along the sweep keeps it feeling like light, not paint.
    half spec = half(pow(max(sin(phase * 6.2831853), 0.0), 6.0)) * 0.35h;
    rgb = clamp(rgb + spec, 0.0h, 1.0h);

    return half4(rgb * color.a, color.a);
}
