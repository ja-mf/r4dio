# ASCII Audio Visuals for r4dio

This document proposes an audio-visualization engine for terminal rendering in `r4dio`, using ideas from `devdocs/ascii-rendering.md` and building on the existing VU/bulb/scope stack.

---

## 1) Current state in r4dio (what we can already leverage)

From the current codebase:

- PCM path already exists (`PcmChunk`) for stream/station visuals.
- RMS path already exists (`AudioLevel`) via mpv lavfi.
- `App::update_audio_trackers()` already gives us stable meter features:
  - `audio_level`, `vu_level`, `peak_level`
  - `meter_mean_db`, `meter_spread_db`
- `ScopePanel` already renders waveform from `pcm_ring`.
- Visualization cadence already exists (`MeterTick`, 25 FPS target).
- Linux already has a system output monitor path (`pipewire_viz.rs`), which is ideal for a loopback-first visual sandbox.

So we are not starting from zero; we already have a good signal backbone and render loop discipline.

---

## 2) Key ideas extracted from `ascii-rendering.md`

The most valuable techniques to carry over:

1. **Do not treat characters as pixels.**  
   Character shape should drive selection, not only brightness.

2. **Use high-dimensional shape vectors (6D), not simple 1D density ramps.**  
   This gives directional and structural fidelity (edges, diagonals, asymmetry).

3. **Normalize vectors before lookup** to keep sampling vectors and glyph vectors in comparable ranges.

4. **Global contrast enhancement on normalized vectors** (power/exponent) sharpens boundaries where needed.

5. **Directional contrast enhancement** with external samples can reduce staircasing and improve edge readability.

6. **Widen directional influence mapping** (external-to-internal region mapping) to avoid brittle local-only behavior.

7. **Lookup optimization matters**:
   - brute force is acceptable for prototypes,
   - quantized-cache lookup is likely the best early optimization in Rust terminal workloads,
   - optional k-d tree can be explored later.

8. **Dithering/noise modulation can improve perceived detail** without increasing cell count.

---

## 3) Proposed core: `Audio Visualization Core`

### 3.1 Goals

- Reusable engine with clean boundaries between:
  1. audio features,
  2. visual field synthesis,
  3. ASCII/glyph mapping,
  4. terminal drawing.
- Easy to add new visual modes without rewriting DSP glue.
- Stable defaults and tunable “expressive” modes.

### 3.2 Conceptual pipeline

```text
Audio source (loopback/stream/file)
  -> PCM frame
  -> Feature extraction (VisualSignals)
  -> Field synthesis (scalar/vector fields per cell)
  -> Shape-aware glyph lookup (+ contrast + dithering)
  -> Ratatui cell buffer render
```

### 3.3 Suggested Rust-facing model

```rust
struct VisualSignals {
    // Existing-derived
    rms_db: f32,
    vu_db: f32,
    peak_db: f32,
    mean_db: f32,
    spread_db: f32,

    // Time-domain
    envelope_fast: f32,
    envelope_slow: f32,
    crest: f32,
    zcr: f32,

    // Frequency-domain
    low: f32,
    mid: f32,
    high: f32,
    low_delta: f32,
    mid_delta: f32,
    high_delta: f32,
    spectral_flux: f32,

    // Rhythm-ish modulators
    pulse: f32,
    transient: f32,

    // Utility
    confidence: f32,
}
```

---

## 4) Signal extraction proposals (practical + cheap first)

Start with inexpensive DSP at 25 FPS:

- **Envelope followers**: fast and slow EMA of absolute amplitude.
- **Crest factor proxy**: `peak / rms`.
- **Zero-crossing rate**: cheap brightness/texture modulator.
- **Band energies**: low/mid/high via lightweight filtering or windowed FFT.
- **Spectral flux**: frame-to-frame change in spectral magnitude.
- **Pulse estimate**: smoothed transient detector (not BPM-accurate, but musically useful).

Design intent: keep all outputs normalized to `[0,1]` where possible for direct visual modulation.

---

## 5) Shape-aware ASCII renderer (audio-adapted)

### 5.1 Glyph atlas precompute

Per glyph (for the chosen terminal font assumptions):
- Rasterize in a virtual cell.
- Compute 6D internal shape vector (staggered sampling circles pattern).
- Store normalized vectors for nearest-neighbor lookup.

Possible glyph sets by aesthetic:
- Soft: `" .:-=+*#%@"`
- Mechanical: `" _/\\|()[]{}<>"`
- Organic/noisy: `".,:;irsXA253hMHGS#9B&@"`
- Braille-heavy hybrid set for high detail on modern terminals.

### 5.2 Per-cell sampling vectors

For each output cell:
- Sample from an **audio-driven scalar field** (not image pixels).
- Compute internal vector (6D).
- Optionally compute external directional vector.

### 5.3 Contrast passes

1. **Global contrast enhancement**  
   Normalize by local max, apply exponent, denormalize.

2. **Directional contrast enhancement**  
   Use external vector influence to sharpen edge directionality.

3. **Widened directional mapping**  
   Use an external-to-internal index map (equivalent to `AFFECTING_EXTERNAL_INDICES`) to avoid abrupt transitions.

### 5.4 Dithering layer

Apply before glyph lookup:
- Ordered Bayer dither for stable patterns.
- Low-amplitude blue-ish noise for more “alive” textures.
- Audio-modulated threshold: e.g., high-frequency energy increases dither grain.

---

## 6) “Map engine” concept (audio-reactive world model)

Instead of drawing waveform only, generate an evolving 2D map/state:

- A grid of cells with persistent state:
  - height
  - flow vector
  - heat
  - age/decay
  - local turbulence
- Audio signals write into this map:
  - **low** drives large-scale deformation / waves / terrain uplift,
  - **mid** drives advection and contour movement,
  - **high** drives local texture, sparks, dithering, micro-fractures.
- The ASCII renderer then reads this map and converts to glyphs with shape-aware lookup.

This is the best path to visuals that feel “alive” and not just metering.

---

## 7) Creative visualization presets (engine-ready)

These are mode ideas designed around available signals:

1. **Tectonic Terrain**
   - Low band pushes broad contour ridges.
   - Mid rotates contour flow.
   - High adds grain and erosion flicker.

2. **Liquid Marble**
   - Field advection with slow swirl velocity.
   - Transients inject local vortices.
   - Directional contrast gives crisp marble veins.

3. **Storm Cells**
   - Grid behaves like a reaction-diffusion-lite system.
   - Pulse/transients spawn “cells” that split and decay.

4. **Neon Drift**
   - Smooth gradients + subtle ordered dither.
   - Great for ambient/chill content.

5. **Shatter Map**
   - Transients trigger radial fracture lines.
   - High frequencies intensify crack-edge contrast.

6. **Topographic Radar**
   - Concentric scan sweeps.
   - Band energies determine contour density and scan echo brightness.

7. **Flocking Glyphs**
   - Vector field steers glyph clusters.
   - Low frequencies influence centroid drift; highs increase jitter.

8. **Pulse Garden**
   - Cellular blobs bloom on beats/transients.
   - Slow decay creates ghost trails.

9. **ASCII Plasma**
   - Oscillatory field equation modulated by spectral flux.
   - Dither and directional contrast prevent muddy gradients.

10. **Granular Rain**
    - High band creates rain particles.
    - Mid steers wind direction.
    - Low controls puddle/ripple accumulation.

---

## 8) Scope-tui improvements and variations (requested)

These can be implemented as scope modes or as bridge modes into the new engine:

1. **Persistence Scope (phosphor)**
   - Keep decay buffer of prior traces.
   - Improves perceived continuity at terminal frame limits.

2. **Dual Envelope Scope**
   - Overlay fast/slow envelopes on waveform.
   - Very readable for dynamics.

3. **Adaptive Trigger Scope**
   - Trigger threshold follows transient statistics.
   - More stable traces across different loudness/mastering styles.

4. **Band-Split Scope**
   - Simultaneous low/mid/high traces with distinct glyph/color channels.
   - Teaches users what each band contributes.

5. **Phase Garden (Lissajous+)**
   - Use delayed/self channels for XY path even in mono contexts.
   - Optional persistence creates “flower” structures.

6. **Glyph Scope**
   - Replace line plotting with shape-aware glyph tiles.
   - Lets scope itself benefit from the ASCII engine.

7. **Topo Scope (hybrid)**
   - Scope trace acts as force emitter in map engine.
   - Produces a direct waveform-to-landscape connection.

8. **Dither Scope**
   - Scope body fills with dynamic dither thresholding by energy.
   - Adds texture without heavy compute.

9. **Edge-Enhanced Scope**
   - Apply directional contrast to local waveform neighborhood.
   - Cleaner visual boundaries in dense passages.

10. **Beat-Latched Scope**
    - On strong pulse: momentary hold/rewind or strobe blend.
    - Gives rhythmic “locks” that look intentional.

---

## 9) Mini app (`viz_lab`) strategy for rapid iteration

Confirmed direction: **system output loopback first**.

### 9.1 Why this first
- Closest to “real listening context”.
- Fast feedback loop for visual quality tuning.
- Existing Linux monitor capture path reduces bootstrap cost.

### 9.2 Suggested input priority

1. Loopback capture (primary)
2. Fallback in-app playback PCM adapter
3. Optional synthetic signal generator for deterministic testing

### 9.3 Runtime controls

- Mode switch (scope, map, hybrid)
- Contrast mode (off/global/directional/hybrid)
- Dither mode (off/ordered/noise)
- Glyph set preset
- Sensitivity/band gains
- Decay/persistence knobs
- Freeze/frame-step for screenshot tuning

---

## 10) Integration path back into main TUI

1. Keep current VU + existing scope behavior as stable default.
2. Add new visualization pane mode only after `viz_lab` presets are usable.
3. Gate advanced modes in config (`[viz]`) to avoid regressions for minimal setups.
4. Reuse existing `MeterTick` initially; revisit adaptive frame pacing only if needed.

---

## 11) Performance and robustness notes

- Limit cell resolution before adding expensive DSP.
- Cache glyph lookups with quantized keys (big win for repeated vectors).
- Keep all heavy allocations out of per-frame hot path.
- Provide graceful degraded modes:
  - no directional contrast
  - lower sampling density
  - reduced glyph set
- Remember terminal/font variance: allow profile presets per terminal family.

---

## 12) Recommended first implementation slice

If we want a high-value first milestone:

1. `viz_lab` loopback capture + `VisualSignals` extraction
2. One map-based ASCII mode (`Tectonic Terrain`)
3. One improved scope mode (`Persistence Scope`)
4. One hybrid mode (`Topo Scope`)
5. Knobs for contrast + dither + glyph preset

This gives immediate creative output while validating the engine architecture.
