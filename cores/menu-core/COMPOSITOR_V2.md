# Menu Core — Compositor v2 (Scanline-Cached Layered Rendering)

**Status:** Draft v0 (spec, not yet implemented).
**Target hardware:** DE10-Nano (Cyclone V SE 5CSEBA6).
**Companion:** [`PROTOCOL.md`](./PROTOCOL.md) for the existing v1 protocol; this
document defines the additions, replacements, and migration plan for v2. v1
remains the working contract until phase 4 of the rollout below completes.

This document is the design lock-in for what we're building before we touch
RTL. Anything implemented that disagrees with this document should either fix
the code or update the document; "the code is the spec" is not accepted while
this is in flight.

---

## 1. Goals

The v1 protocol treats the FPGA as a host-driven 2D GPU: the host issues blit
commands, the FPGA's blit engine writes pixels into a single framebuffer, and
the MiSTer framework's MISTER_FB scanout reads that framebuffer at HDMI rate.
This works but couples render perf to host perf: a slow JS frame becomes a
slow render frame because the host owns the pixel-production path.

v2 separates the two roles. The host writes **layer source textures** (one per
logical UI region — wallpaper, menu DOM, overlays, notifications); the FPGA
**compositor** walks those layers per scanline and writes the composed result
to the scanout framebuffer; MISTER_FB scanout reads the scanout FB at HDMI rate
as before.

The key wins this enables:

1. **Idle frames cost almost nothing.** A static scene marks zero scanlines
   dirty; the compositor does nothing; the only DDR3 traffic is the constant
   MISTER_FB scanout reads.
2. **Animation cost is proportional to *what changed*, not to scene
   complexity.** Moving a layer by updating its `dst_x` re-composites only the
   scanlines that layer touches. Repainting a small UI element marks only the
   scanlines under that element dirty.
3. **JS perf no longer drags down render perf.** The compositor produces frames
   from the last-known layer state. If the host hasn't updated anything, the
   compositor doesn't run.
4. **Notifications, overlays, modal dialogs become layer-descriptor writes**
   rather than full repaints of whatever's underneath.
5. **Triple-buffered animated backgrounds**: rotate which texture L0 points at
   per frame; the compositor reads whichever is current. Background animation
   is bandwidth-bound by the host's write side, not by the compositor.

What v2 explicitly does **not** try to do:

- Replace the blit_engine. It still exists and is still how the host paints
  into layer source RTs.
- Provide full GPU primitives (vector graphics, shaders, compute). The
  rendering model is still 2D layered raster blitting.
- Support arbitrary 3D transforms. Per-layer scale (nearest-neighbour) and
  panning are in scope; rotation and shear are out.

---

## 2. Architecture overview

```
Host (Rust + JS)
  │  ┌─ blits text glyphs / icons into layer source RTs ──┐
  │  │  (only when that layer's React tree commits)        │
  │  │                                                      ▼
  │  │  ┌─────────────── DDR3 ────────────────────────────────────┐
  │  │  │                                                          │
  │  └─►│  L0 source RT (wallpaper texture)                         │
  │     │  L1 source RT (menu DOM)                                  │
  │     │  L2 source RT (overlay)                                   │
  │     │  L3 source RT (notification)                              │
  │     │                                                            │
  │     │  layer table A / B (descriptors, 256 × 32 B)              │
  │     │                                                            │
  │     │  scanout FB (triple-buffered) ◄── compositor writes here  │
  │     │                                                            │
  │     └────────────────────────────────────────────────────────────┘
  │                ▲                            │
  │                │ reads sources              │ reads scanout FB
  │                │ + writes scanout FB        │ at HDMI rate
  │                │ (per dirty scanline)       ▼
  │           ┌────┴──────┐               ┌──────────────────┐
  └──────────►│ Compositor│               │ MISTER_FB scanout │
   layer      │  (FSM +   │               │ (framework)        │
   descriptor │  arbiter) │               └─────────┬──────────┘
   updates    │           │                         │
   + ops      │  dirty    │                         │
   (INVALIDATE│  bitmask  │                         │
    _RECT etc)│  (1024 b  │                         │
              │  × 2,     │                         │
              │  BRAM)    │                         │
              └───────────┘                         │
                                                    ▼
                                                  ASCAL → HDMI
```

### Comparison with v1

| Aspect | v1 (today) | v2 |
|---|---|---|
| Host pixel target | FB directly | per-layer source RTs |
| Composition | implicit (host paints in z order) | explicit (layer descriptors, painter blends) |
| Wallpaper | painted into FB every frame in damage area | one-shot upload to L0 source; never repainted |
| Per-frame DDR3 from host | ~150 MB/s during nav | ~50 MB/s typical (UI changes only) |
| Scanout reads | 498 MB/s (one FB) | 498 MB/s (one scanout FB) — same |
| Compositor → scanout FB | n/a | dirty-scanline writes, ~50-150 MB/s during nav |
| Idle frame cost | one-FB-pass overhead | zero |
| Animation primitives | per-frame repaint | descriptor update where possible; repaint otherwise |

Net DDR3 traffic is comparable to v1 during nav; idle/static frames are much
cheaper. The structural win is that the host stops being on the per-frame
render critical path.

---

## 3. Memory layout (additions)

v1 layout in `mem.rs` remains valid. v2 adds these regions inside the same
reserved 32 MB block (offsets within the host-side base):

```
+0x0000_0000   FB0   ─┐
+0x0080_0000   FB1   ├── repurposed as the SCANOUT FB triple-buffer
+0x0100_0000   FB2   ─┘    (compositor writes; MISTER_FB reads)
+0x0180_0000   ring buffer
+0x0188_0000   layer table A
+0x018A_0000   layer table B
+0x0200_0000   texture pool (decoded PNGs, glyph atlases — unchanged)
+0x0E00_0000   per-layer source RT pool (NEW)
   each layer source RT is allocated host-side from this region
+0x1000_0000   end of reserved region
```

Notes:

- The three FBs from v1 keep the same physical addresses (`FB0_OFFSET`,
  `FB1_OFFSET`, `FB2_OFFSET`). Triple-buffering still uses fb_swapper. The only
  change is *who writes them*: the compositor instead of the blit engine.
- The "per-layer source RT pool" is a new region. Allocation strategy:
  bump-allocated by the host at startup based on the layer config. Same
  approach as the texture pool.
- The per-scanline dirty bitmask **lives entirely in BRAM, not DDR3** (§6).
  At 1024 bits per buffer × 2 buffers = 256 B total, the BRAM cost is
  negligible (a fraction of one M10K block) and the access pattern is much
  better suited to BRAM than DDR3 — single-cycle reads from the compositor
  FSM each scanline, single-cycle writes from the bus interface on
  `INVALIDATE_*` ops. No DDR3 traffic for the dirty mask at all.
- All physical addresses are 32-byte aligned (matches DDR3 burst boundaries).

---

## 4. Per-layer source RT model

A layer source RT is just a texture in DDR3 in the v1 sense. The host:

1. Allocates an RT at startup via `device.create_render_target(w, h)`.
2. Paints into it using existing blit ops (`SET_RENDER_TARGET`, `COPY_RECT`,
   `FILL_RECT`, etc.).
3. Writes a layer descriptor pointing at it (`tex_id = rt.id`).
4. Re-paints into it whenever the layer's content changes (host owns this
   decision, typically via a React reconciler scoped to that layer).

Common configurations:

| Layer | Source RT | Updated when |
|---|---|---|
| L0 wallpaper (static) | full-size PNG decoded once | never after upload (or rarely, e.g. user changes background) |
| L0 wallpaper (animated) | 1-3 RTs rotated frame-to-frame | host writes one frame per visible animation frame |
| L1 menu DOM | full-size RT | React commits in the menu's reconciler |
| L2 overlay / modal | overlay-sized RT (e.g. 1200×600) | overlay opens, animates, closes |
| L3 notification | small RT (e.g. 360×80) | notification appears or its content changes |

Layer source RTs are NEVER read by MISTER_FB scanout. They are only read by the
compositor during scanline build. This means a layer source RT can be ANY size
≤ the scanout FB; the compositor's `dst_w/dst_h` controls how much of it is
sampled per scanline (with `src_w/src_h` controlling the source rect).

### Allocation API

```
device.create_layer_rt(width: u16, height: u16) -> LayerRtHandle
device.delete_layer_rt(handle: LayerRtHandle)
```

The handle wraps a texture id and a phys address. Same shape as
`TextureHandle` for the existing texture pool, but allocated from the
per-layer source RT pool to keep the texture pool free for glyph atlases /
PNG textures.

(Implementation note: the underlying allocator can be unified between the
texture pool and the layer source RT pool — they're both bump allocators
backed by the same DDR3 region in v1. v2 separates the regions only because
layer source RTs tend to be very large vs the small textures.)

---

## 5. Layer descriptor format v2

The 32-byte descriptor format from v1 has unused bits we now claim. Backward
compatibility with v1 is preserved: a v1-formatted descriptor (with all v2
extensions zero) still produces v1 behaviour.

```
bits      bytes  field       type  notes
─────────────────────────────────────────────────────────────────────────
[15:0]    0-1    flags       u16   bit 0 = enabled (v1)
                                    bit 1 = opaque (v2, see §5.1)
                                    bit 2 = blend_mode high bit (v2, see §5.2)
                                    bit 3 = blend_mode low bit  (v2)
                                    bit 4 = scale_enable (v2, see §5.3)
                                    bits 5-15 reserved (host MUST write 0)
[31:16]   2-3    tex_id      u16   0xFFFF = solid; otherwise layer source RT id
[47:32]   4-5    dst_x       i16   signed (off-screen left ok)
[63:48]   6-7    dst_y       i16   signed
[79:64]   8-9    dst_w       u16
[95:80]   10-11  dst_h       u16
[111:96]  12-13  src_x       u16
[127:112] 14-15  src_y       u16
[143:128] 16-17  src_w       u16   v1: reserved; v2: used iff scale_enable
[159:144] 18-19  src_h       u16   v1: reserved; v2: used iff scale_enable
[191:160] 20-23  color       BGRA  solid fill / A8 tint (unchanged)
[199:192] 24     opacity     u8    v1: reserved; v2: 0=invisible, 255=opaque
[207:200] 25     z_priority  u8    v2: secondary sort key (see §5.4)
                                    v1: reserved (host MUST write 0)
[255:208] 26-31  reserved    -     host MUST write 0
```

### 5.1. `opaque` flag

When set, the painter MUST treat this layer as fully opaque within its
`dst_x..dst_x+dst_w` x-range on every scanline it covers. Lower-z textured
layers contained entirely within that x-range MUST be culled from the active
list for that scanline (the scanout pixel value for those columns comes
entirely from this layer plus higher-z layers).

The host SHOULD set this flag whenever the source RT is known to be fully
opaque in its sampled rect (e.g. a solid-background status bar). Setting it
when the source RT actually has transparent pixels produces incorrect output;
this is the host's responsibility to track.

The `opacity` byte (§5.5) is multiplied AFTER the opacity flag is consulted —
i.e. an `opaque` layer with `opacity = 0x80` is still culled-lower-layers but
output at 50% blend, which produces a "tinted glass over the underlying
empty/black" effect. Unlikely to be useful; the host SHOULD NOT combine these
unless they really want that behaviour.

### 5.2. `blend_mode` field (2 bits)

```
00  SrcAlpha       (v1 behaviour: out = src·a + dst·(1-a))
01  Opaque         (out = src; alpha channel ignored at composition)
10  Additive       (out = src·a + dst, saturating)
11  reserved       (host MUST NOT write this value)
```

The default 00 reproduces v1 behaviour exactly. The Opaque blend mode at
composition time is distinct from the `opaque` *flag* (§5.1): the flag is a
host-side promise enabling z-occlusion; the blend mode controls the actual
math at output. The two can be set independently.

### 5.3. `scale_enable` flag and `src_w`/`src_h`

When clear, the painter samples 1:1 from `src_x, src_y` for `dst_w, dst_h`
output pixels (v1 behaviour; the `src_w/src_h` fields are ignored).

When set, the painter samples `src_w × src_h` pixels from the source and
scales to `dst_w × dst_h` using nearest-neighbour (the same kernel as the
blit engine's scaled COPY_RECT). The painter's address generation gets a
Q16.16 fixed-point step accumulator per axis.

This is the v2 feature with the highest implementation cost; see §11.

### 5.4. `z_priority`

Primary z-order is still slot index (slot 0 = back, slot N = front), as in
v1. `z_priority` is a secondary tie-breaker the painter applies when two
slots claim the same logical layer position. Host MAY use this to swap
which of two overlay layers appears on top without re-laying out the whole
descriptor table. Default 0 = use slot index ordering.

### 5.5. `opacity`

8-bit alpha multiplier applied to the layer's contribution at blend time.
0 = layer invisible (output unchanged), 255 = full strength. Cheap to add
in RTL (one 8×8 multiply per blend stage). v1 left this field unread; v2
wires it through `scanline_filter` to the painter.

---

## 6. Dirty tracking

The per-frame dirty bitmask is 1024 bits (one per scanline; bits 1080-1023
unused). It lives in **on-chip BRAM**, not DDR3 — 128 B per buffer × 2
buffers (A active, B building) = 256 B total, well under the noise floor of
the FPGA's BRAM budget. The host never directly touches the BRAM; bits are
set via ops in the ring-buffer command stream that the FPGA decodes into
single-cycle BRAM writes.

Two reasons for BRAM over DDR3 here:

1. **Access pattern.** The compositor's FSM reads one bit per scanline (1080
   reads per frame). DDR3 reads cost ~10 cycles minimum and waste burst
   capacity on 1-bit fetches. BRAM reads are single-cycle and free.
2. **Size.** 256 B doesn't justify a DDR3 region; it doesn't even fill a
   single M10K block (1280 B).

### 6.1. Writing the dirty bitmask

The host issues `INVALIDATE_RECT(x, y, w, h)` ops (see §8). The FPGA decodes
each into a y-range bit-set on the "building" bitmask. The host can issue
multiple `INVALIDATE_RECT`s per frame; bits accumulate (OR-into-place).

For full-screen invalidations (boot, resolution change, etc.), the
`INVALIDATE_ALL` op sets all 1024 bits in a single bus write. The host
SHOULD NOT issue 1080 individual `INVALIDATE_RECT`s for "all scanlines";
use `INVALIDATE_ALL`.

The host has **no read path** to the bitmask in the normal flow. If we ever
need one for diagnostics, the host can mmap a debug register window that
exposes the active BRAM contents read-only.

### 6.2. Automatic dirty propagation from layer descriptor changes

Any time the host writes a new layer table (via `LAYER_COMMIT`), the FPGA
MUST compare the new descriptor against the old one for each slot and mark
the union of the old and new `dst_y..dst_y+dst_h` ranges dirty. This makes
the common case (host moves a layer, doesn't touch the bitmask explicitly)
just work.

Specifically dirty-mark conditions per slot diff:

- New descriptor has `enabled` set and old didn't: mark `[new_y, new_y+h)`.
- Old descriptor had `enabled` and new doesn't: mark `[old_y, old_y+h)`.
- Both enabled and any of {`dst_*`, `src_*`, `tex_id`, `color`, `flags`,
  `opacity`, `z_priority`} differ: mark union of both ranges.

This is computed inside `layer_dma` when it DMAs the new table; it's cheap
(per-slot field comparison + a y-range OR into the bitmask).

### 6.3. First-frame and full-screen invalidations

On boot, after `START`, the entire bitmask is set to 1 so the first frame
fully composites. Host MAY issue `INVALIDATE_ALL` to force a full
recomposition (e.g. after changing render resolution).

### 6.4. Limits and granularity

- Scanline granularity, not pixel-rect granularity. If 1 pixel changes on
  scanline 500, the entire scanline 500 gets recomposed. Acceptable: the
  per-scanline cost is small relative to per-frame fixed overheads.
- The compositor processes dirty scanlines in y order, not in random-access
  order. Out-of-order processing offers no DDR3 benefit and complicates the
  FSM.

---

## 7. New register additions

The v1 register file (`menu_core_regs.sv`) gains these fields. v1 indices
unchanged; v2 indices added in the previously-reserved range.

```
0x00  ID                   (existing)
...
0x60  LAYER_COUNT          (existing)
0x68  LAYER_TABLE_BASE     (existing)
0x6C  LAYER_COMMIT         (existing; semantics extended — see §8)
0x84  COMPOSITOR_CONTROL   (new) bit 0 = enable; bit 1 = pause; bit 2 = use_compositor_scanout
0x88  COMPOSITOR_STATUS    (new) bit 0 = busy; bit 1..2 = current_buffer (0/1/2)
0x8C  SCANOUT_FB_SELECT    (new) bit 0 = 0:use MISTER_FB direct (v1) / 1:use compositor output
0x90  FB_BASE              (new) base for the scanout FB triple-buffer (replaces hardcoded address)
0x94  COMPOSITE_FENCE      (new) host reads → "compositor has completed all frames up through fence value X"
```

### 7.1. `COMPOSITOR_CONTROL`

- bit 0 `enable`: when 0, the compositor is idle (no scanline processing).
  When 1, it processes the dirty bitmask each vsync.
- bit 1 `pause`: when 1, compositor finishes any in-flight scanline and
  stops. Useful for atomic descriptor mutations.
- bit 2 `use_compositor_scanout`: replaces the hardcoded `FB_EN = 1'b1`
  with a runtime toggle. When 0: MISTER_FB scans whatever's at
  `scanout_fb_base` (this could be host-written or compositor-written
  depending on bit 0). When 1: framework consumes `VGA_*` from the
  compositor's pixel path (the v1 path with FB_EN=0 we discussed).

### 7.2. `SCANOUT_FB_SELECT`

For the staged rollout (§11), this lets us flip atomically between:

- 0: MISTER_FB scans the host-written FB (v1 path — host's blit_engine
  writes, MISTER_FB reads)
- 1: MISTER_FB scans the compositor-written FB (v2 path — compositor
  writes, MISTER_FB reads)

The two FB regions can be the same physical address; what changes is who's
the writer.

### 7.3. `COMPOSITE_FENCE`

Analogue to v1's `FENCE_VALUE`. After processing a frame, the compositor
writes the frame index here. The host uses it to know when a particular
descriptor commit has fully taken effect.

---

## 8. New ops in the ring buffer command stream

v1 ops remain. v2 adds:

```
0x10  INVALIDATE_RECT      args: x16, y16, w16, h16   (8 bytes)
0x11  INVALIDATE_ALL       no args                    (0 bytes)
0x12  MASK_COMMIT          no args                    (0 bytes)
0x13  SET_LAYER_RT_BASE    args: rt_id16, phys_addr32 (6 bytes)
```

### 8.1. `INVALIDATE_RECT(x, y, w, h)`

Mark scanlines `y..y+h` dirty in the next-frame bitmask (the inactive
buffer). The x/w fields are ignored by the compositor today — granularity
is per-scanline — but reserved for future per-rect dirty tracking.

### 8.2. `INVALIDATE_ALL`

Equivalent to `INVALIDATE_RECT(0, 0, fb_w, fb_h)`. Fast path that sets all
bits to 1 in one register write.

### 8.3. `MASK_COMMIT`

Atomically swap the active BRAM-resident dirty bitmask buffer with the
inactive one. The host typically issues this at end-of-frame after all
`INVALIDATE_*` ops have been queued.

`LAYER_COMMIT` implicitly also commits the bitmask: when the FPGA processes
a `LAYER_COMMIT` it swaps both the layer table active/inactive index *and*
the bitmask active/building index in a single cycle. Hosts that don't need
separate timing for the two MAY just call `LAYER_COMMIT` and skip
`MASK_COMMIT`; hosts that want to commit dirty bits without changing
descriptors (rare) use `MASK_COMMIT` standalone.

### 8.4. `SET_LAYER_RT_BASE(rt_id, phys_addr)`

Host writes a (texture id → phys addr) mapping. The compositor's
texture_unit uses this to translate `tex_id` in a layer descriptor to a DDR3
base address.

(This is already the model for the v1 texture upload path; v2 just extends
the same id space to include layer source RTs.)

---

## 9. Compositor execution model

### 9.1. When does it run

The compositor's frame begins on each HDMI vsync rising edge (same trigger
as v1's `layer_dma` kick). On vsync:

1. `layer_dma` re-reads the active layer table into on-chip cache.
2. Layer-vs-prev-layer diff fires for each slot; dirty bitmask is updated
   with the unions of changed dst_y ranges (§6.2).
3. The compositor begins processing scanlines y=0..1079, in order.
4. For each scanline whose dirty bit is 1:
   - scanline_filter walks the layer cache, builds the active list.
   - texture_unit fetches each active textured layer's row from DDR.
   - Painter blends pixels, writes the composed scanline to the back-buffer
     scanout FB.
   - Clear the scanline's dirty bit.
5. For each scanline whose dirty bit is 0: skip entirely; the back-buffer
   already has the correct pixels from the previous frame (assuming we
   double-buffer the scanout FB — see §10).
6. When all dirty scanlines have been processed, the compositor fires a
   "frame done" pulse → fb_swapper rotates `ready ← back`.
7. On the next vsync, fb_swapper does `display ← ready`; MISTER_FB now
   scans the just-composited FB.

### 9.2. Timing budget

At clk_video = 100 MHz:

- Per-scanline cost ≈ 1920 pixels × 1 cycle/px = 19200 cycles ≈ 0.19 ms.
- Plus per-scanline HBlank fetch ≈ depends on `MAX_TEXTURED`; for 4 active
  textured layers at full width, ~1500 cycles ≈ 0.015 ms additional setup.
- Worst case (all 1080 dirty): 1080 × 0.2 ms ≈ 216 ms. Exceeds vsync.
- Typical (20% dirty): 216 × 0.20 ≈ 43 ms. Exceeds 16.67 ms vsync; ~3
  frames to settle.

This is a real concern. Two paths:

- **Accept transient frame drops** when the host paints a large UI change.
  The compositor needs ~3 frames to finish a 20%-dirty update. During those
  frames the displayed image lags by 1-2 frames.
- **Run the compositor at higher than HDMI rate.** clk_video can plausibly
  be doubled if the painter's combinational depth allows. Roughly halves
  the worst-case time.

For phase 1, accept the frame drops. Revisit if user-visible.

### 9.3. Concurrency with the host

The host can write layer source RTs while the compositor is reading them
because they target different scanlines. Specifically: while the compositor
is processing scanline N, the host MUST NOT write to source RT pixels that
the compositor will read for scanline N (i.e. for that layer, scanlines
that the layer's `dst_y..dst_y+dst_h` maps into the current compositor y).

Practically, the host's reconciler commits one full layer's source RT
between vsyncs, then the compositor processes the resulting frame. Mid-flight
mutation is forbidden; the existing `LAYER_COMMIT` two-table swap pattern
applies to layer source RTs as well (host paints into the inactive RT,
swaps via descriptor `tex_id` change at commit).

---

## 10. Scanout FB management

The scanout FB is triple-buffered using the existing FB0/FB1/FB2 addresses
and fb_swapper. Behaviour:

- **Compositor** writes to FB[render_idx] (the one fb_swapper currently
  considers RENDER).
- **MISTER_FB** reads from FB[display_idx].
- **fb_swapper** rotates render/ready/display on PRESENT (compositor-frame-
  done) + vsync, exactly as in v1.

The catch: with scanline damage tracking, the compositor only writes the
*dirty* scanlines to FB[render_idx]. The clean scanlines must already have
the correct pixels — but FB[render_idx] is the "third buffer" that wasn't
on screen during the previous frame, so its content is stale.

Three options:

### 10.1. Option A: clean-scanline copy from FB[ready_idx]

Before the compositor starts, copy the clean scanlines from FB[ready_idx]
(the buffer just promoted off READY → DISPLAY) to FB[render_idx]. Cost:
(1 - dirty_fraction) × 8 MB read + write per frame. For 20% dirty: 6.4 MB
copy per frame × 60 Hz = 384 MB/s. Pulls a lot of DDR3.

### 10.2. Option B: keep a single "true" FB, accept tearing

Compositor writes directly to the FB MISTER_FB is reading. Risk: visible
tearing if a scanline is mid-composite when scanout reads it. Mitigated by
running compositor "ahead of" scanout (compositor processes scanline N
during HBlank N-1). Not bulletproof; some scanlines could still tear under
pathological cases.

### 10.3. Option C: dual-buffer, full-overwrite

Compositor processes ALL scanlines every frame (whether dirty or not) but
reads from the dirty bitmask to decide whether to use the layer-fetched
pixel or to re-output the value already at FB[render_idx]. Same DDR3 reads
as A but the writes are still to FB[render_idx], no copy. Adds a per-pixel
read-modify-write on clean scanlines (FB[render_idx][pixel] → FB[render_idx]
[pixel]).

**Provisional decision:** option B with the "scanline-ahead-of-scanout"
guarantee. Easiest to implement; we accept the small tearing risk and
revisit if visible.

(Open question for review: option A may actually be reasonable on this
architecture if DDR3 has headroom. Measure phase 1 bandwidth first.)

---

## 11. Phasing — staged rollout

Five phases, each independently reviewable and reverable. The
`SCANOUT_FB_SELECT` toggle lets us A/B between v1 and v2 paths on the same
RBF for direct comparison.

### Phase 1 — Compositor writes to DDR (no dirty tracking yet)

- Add Avalon-MM write master to `compositor.sv`.
- Compositor outputs pixels to FB[render_idx] instead of `VGA_*`.
- `SCANOUT_FB_SELECT = 1` routes MISTER_FB to read the compositor's FB.
- Compositor regenerates every scanline every frame.
- Per-layer alpha (§5.5) and per-layer blend modes (§5.2) wired through.
- Host work: minimal — just configure `SCANOUT_FB_SELECT` and exercise the
  compositor with the existing v1 layer-descriptor protocol.

**Validation:** pixel-identical output to v1 when given equivalent layer
descriptors. Bandwidth measurement: should be ~750 MB/s steady state
(498 scan + ~250 compositor write).

**Cost estimate:** ~1 week of FPGA + ~2 days of host (just wiring).

### Phase 2 — Per-scanline dirty tracking

- Add `DIRTY_BITMASK_BASE` register and the bitmask region.
- Add `INVALIDATE_RECT`, `INVALIDATE_ALL`, `MASK_COMMIT` ops.
- `layer_dma` auto-marks dirty scanlines on descriptor changes.
- Compositor skips clean scanlines per §9.1.
- Implement scanout FB management option B (§10.2).

**Validation:** static scene = compositor idle after first frame; dynamic
scene = bandwidth proportional to dirty area.

**Cost estimate:** ~1 week of FPGA + ~3 days of host (new ops, dirty
tracking).

### Phase 3 — Per-layer features

- Per-layer scale (§5.3, the big one). May not fit timing on first pass;
  worst-case defer.
- Per-layer `z_priority` (§5.4, small).

**Validation:** unit-test each feature with a small example.

**Cost estimate:** ~1 week, mostly the scale feature. Could parallelise
with phase 2.

### Phase 4 — Host-side migration

- Define per-layer RT model in the host runtime.
- Multiple React reconciler roots (one per layer).
- Damage tracking per layer.
- Animation system maps tween targets to descriptor fields where possible.
- Reuse the existing PNG/font loading; layer source RT allocation is the
  only new host side.

**Cost estimate:** ~2-3 weeks of focused host + JS work. Significant React
reconciler config change.

### Phase 5 — Cleanup

- Delete v1's "host paints into FB" code path once nothing uses it.
- `assign FB_EN` removed from `menu_core.sv`; controlled by register.
- PROTOCOL.md updated to mark v1 paint path as removed; PROTOCOL.md v2 = this
  doc once it's all implemented.

**Cost estimate:** ~2 days.

**Total realistic estimate:** 5-7 weeks of focused work, with FPGA and
host work proceeding in parallel where possible.

---

## 12. Risks and open questions

1. **Worst-case repaint exceeds vsync (§9.2).** Provisional answer: accept
   1-2 frame drops on large UI changes. Revisit if user-visible.

2. **Scanout FB tearing under option B (§10.2).** Mitigated by running
   compositor ahead of scanout; risk is a single-scanline tear at the
   crossover boundary. Acceptable for UI; not for video playback.

3. **Per-layer scale (§5.3) may not fit timing.** Roughly the same cost as
   the blit_engine's scaled COPY_RECT; it fits there. But the painter has
   additional pipeline depth from MAX_TEXTURED stages, so scale might bust
   the 10 ns budget at clk_video = 100 MHz. Fallback: drop scale to phase 6
   or implement it at half rate.

4. **Bandwidth math for full-screen dynamic L1.** Even with dirty tracking,
   if 100% of L1's scanlines are dirty every frame (e.g. animated wallpaper
   below a transparent menu DOM), we pay full per-frame layer-source bandwidth.
   Acceptable; the architecture's win is for static / partial-change frames.

5. **DDR3 arbiter priority.** Add compositor as a master; priority
   `blit > layer_dma > tex_unit > compositor > scanout_fetcher`. Compositor
   can tolerate stalls; scanout can't.

6. **Per-scanline dirty granularity vs per-rect.** Per-scanline is
   architecturally simpler and tracks well with the compositor's scanline-
   sequential processing. Per-rect would save bandwidth on cases like
   "the right half of scanline N changed but the left didn't" — but those
   savings are likely <5% in practice. Decision: per-scanline for v2.

7. **Backward compat for v1 layer descriptors.** All v2 flag extensions
   default to v1 behaviour when the bit is 0. A v1 host writing v1-only
   descriptors produces v1 output (subject to the new SrcAlpha-only blend
   path). Migration is per-layer; layers that use v2 features and layers
   that don't can coexist.

8. **What about the existing `staging_rt` mental model?** It's gone. The
   v2 scanout FB serves the same role (a target the compositor writes to
   that MISTER_FB reads from), but it's owned by the compositor, not the
   host. The host doesn't directly target the scanout FB anymore.

---

## 13. Out of scope for v2 (deferred to v3+)

- 3D rotation, shear, arbitrary affine.
- Per-pixel shaders / programmable blend.
- Linear-light blending (sRGB-correct compositing).
- HDR / >8 bpc output.
- Streaming video as a layer (would need a different layer source pump).
- Cross-layer effects (blur of one layer informed by another).

Any of these are fair v3 candidates if the v2 architecture turns out to land
cleanly.

---

## 14. Reference implementations / prior art

- iOS CALayer / Core Animation: same separation of "host owns layer source
  bitmaps; compositor owns scanout."
- Wayland subsurfaces + the linux DRM atomic plane API: same model at the
  driver level.
- The Sega Genesis VDP (with sprite/plane layers + scrolling registers) is
  the spiritual ancestor of this whole architecture; we're closer to it
  than to a modern GPU.

---

## 15. Document changelog

- v0 (initial draft, this file): describes the architecture and phases for
  scanline-cached layered rendering. Pre-implementation.
- v0.1: dirty bitmask moved from DDR3 to BRAM (256 B total, fits in a
  fraction of one M10K). `DIRTY_BITMASK_BASE` register removed; the bitmask
  is no longer host-mappable. `MASK_COMMIT` semantics clarified (BRAM
  active/building index swap). Memory layout, register table, and §6 rewritten
  accordingly.
