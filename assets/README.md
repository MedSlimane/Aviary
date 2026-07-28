# Aviary visual asset pack

A self-contained visual foundation for Aviary, a macOS manager for personal AI agents. Every artwork is pure SVG: no linked images, web fonts, scripts, or runtime dependencies.

The texture family is intentionally atmospheric rather than geometric. Each color artwork combines a base sweep, multiple offset radial blooms, blurred highlight paths, blend modes, and procedural film grain. The brand geometry is vector-only and the wordmark contains custom paths rather than font text.

## Texture assets

| Asset | Size | Intended usage |
| --- | ---: | --- |
| `textures/hero-aurora.svg` | 1600 × 400 | Primary onboarding hero, featured agent collection, or optimistic modal top. Violet → blue → pearl with a restrained coral edge. |
| `textures/hero-ember.svg` | 1600 × 400 | Warm announcement, creation flow, upgrade moment, or human-centered feature banner. Peach/coral → amber → pink. |
| `textures/hero-tidal.svg` | 1600 × 400 | Calm status, environment, settings, or successful connection banner. Teal → seafoam → pale gold. |
| `textures/hero-dusk.svg` | 1600 × 400 | Dark-surface hero, high-focus session, advanced tooling, or dramatic modal top. Indigo → magenta → rose. |
| `textures/card-aurora.svg` | 800 × 240 | Compact card header using the Aurora palette with a tighter, higher-contrast composition. |
| `textures/card-ember.svg` | 800 × 240 | Compact warm card header for creation, recent activity, or highlighted prompts. |
| `textures/card-tidal.svg` | 800 × 240 | Compact calm card header for connected services, environments, and healthy states. |
| `textures/card-dusk.svg` | 800 × 240 | Compact moody card header for terminals, active sessions, or advanced controls. |
| `textures/grain-fine.svg` | 512 × 512 | Fine monochrome film-grain overlay at roughly 4% alpha. Composite over gradients with `overlay`, `soft-light`, or normal blending. |
| `textures/grain-coarse.svg` | 512 × 512 | Larger-particle monochrome grain at roughly 8% alpha for empty states, oversized artwork, or subtle tactile emphasis. |
| `textures/dot-grid.svg` | 64 × 64 | Very-low-contrast one-pixel dot grid for near-black canvases, inspectors, and agent workspaces. |
| `textures/mesh-noise.svg` | 1200 × 800 | Desaturated full-window ambient mesh. Use behind dark surfaces at full bleed; avoid stretching far beyond its 3:2 ratio. |

### Tileability

These assets are seamless tiles:

- `textures/grain-fine.svg`
- `textures/grain-coarse.svg`
- `textures/dot-grid.svg`

The hero, card, and mesh artworks are composed canvases and are not tileable. Crop them with `preserveAspectRatio="xMidYMid slice"` or CSS `object-fit: cover`.

## Brand assets

| Asset | Size | Intended usage |
| --- | ---: | --- |
| `brand/aviary-mark.svg` | 128 × 128 | Monochrome mark for navigation, buttons, title bars, and stamps. It uses `fill="currentColor"` and inherits the surrounding text color. |
| `brand/aviary-mark-gradient.svg` | 128 × 128 | Iridescent mark for marketing, onboarding, empty states, and selected/high-emphasis moments. |
| `brand/aviary-wordmark.svg` | 480 × 128 | Full horizontal lockup. Both the spark and the hand-drawn geometric letter paths inherit `currentColor`. No font is required. |
| `brand/aviary-icon-macos.svg` | 1024 × 1024 | macOS app icon master with a dark rounded-square container, subtle material depth, and an iridescent mark occupying about 60% of the canvas. Export platform raster sizes from this master. |

Keep clear space around the standalone mark equal to at least one petal length. Do not place the gradient mark directly over a similarly saturated texture; use the monochrome mark in white or near-black for reliable contrast.

## Design tokens

`tokens.json` contains:

- Five hex stops for each gradient family: Aurora, Ember, Tidal, and Dusk.
- Parallel dark and light neutral ramps using semantic keys.
- Radius and blur scales.
- Structured card, popover, modal, and focus shadows with paste-ready CSS values.

The dark ramp is the default Aviary UI foundation. Use `bg-canvas` for the window, `bg-surface` for persistent panels, and `bg-elevated` for transient or selected surfaces. Keep borders subtle; the artwork should supply atmosphere without becoming interface chrome.

## Implementation notes

- All SVGs include a fixed `viewBox` and explicit pixel dimensions.
- Grain is procedural and may vary slightly across SVG renderers, but remains deterministic because every turbulence filter has a fixed seed.
- The texture SVGs use `mix-blend-mode` in presentation styles. Modern WebKit, Chromium, and macOS SVG renderers support these modes.
- For reduced-transparency contexts, use the SVGs as opaque images rather than placing live translucent content behind them.
- Do not recolor the texture families with CSS filters. Select the nearest family and preserve its carefully balanced highlights.
