# Menu Core — Host/FPGA Protocol Specification

**Status:** Draft v0
**Target hardware:** DE10-Nano (Cyclone V SE 5CSEBA6)
**Target video mode:** 1920×1080 @ 60 Hz (148.5 MHz pixel clock)

This document specifies the binary contract between the ARM-side host (Rust)
and the FPGA-side menu core (Verilog). Both sides MUST implement this spec
exactly. Any incompatible change requires bumping the protocol version in the
`ID` register and updating this document.

---

## 1. Overview

The menu core is an FPGA bitstream that provides a 2D GPU-style rendering
engine. It is loaded into the FPGA when the 1FPGA firmware is in its main menu,
replacing any running emulator core. The host writes textures and command lists
into a reserved region of HPS DDR3. The FPGA reads those commands and textures,
renders into a triple-buffered framebuffer also in DDR3, and scans the
framebuffer out to HDMI.

The host does not push pixel data over SPI. The SPI channel (MiSTer `user_io`)
is used only for core identification and optional auxiliary signaling. All
bulk data transfer happens through shared DDR3.

```
Host (Rust)                             FPGA (menu core)
-----------                             ----------------
writes textures, commands   ──►    DDR3 (reserved 256 MB)
                                        ▲       │
                                        │       │ reads
LW_H2F write: advance tail ───►  control regs │
                                        │       ▼
                                        │   Command fetcher
                                        │       │
                                        │       ▼
                                        │   Blit engine
                                        │       │
                                        │       ▼
                                        └── Framebuffer
                                                │
                                                ▼
                                           Scanout engine
                                                │
                                                ▼
                                           ADV7513 HDMI
```

---

## 2. Memory layout

The host reserves 256 MB of physically-contiguous HPS DDR3 at a fixed base
address. The base address is discovered at boot time and programmed into the
FPGA via control registers. The default base address is `0x30000000` but the
implementation MUST support any 32-MB-aligned address.

All offsets below are relative to the reserved base.

| Offset (hex) | Size (hex)   | Region                   | Notes                         |
|--------------|--------------|--------------------------|-------------------------------|
| `0x00000000` | `0x00800000` | Framebuffer 0            | 8 MB, holds up to 1920×1080×4 |
| `0x00800000` | `0x00800000` | Framebuffer 1            | 8 MB                          |
| `0x01000000` | `0x00800000` | Framebuffer 2            | 8 MB                          |
| `0x01800000` | `0x00100000` | Command ring buffer      | 1 MB                          |
| `0x01900000` | `0x00020000` | Texture descriptor table | 128 KB — 4096 entries × 32 B  |
| `0x01920000` | `0x006E0000` | Reserved                 | Padding to 32 MB              |
| `0x02000000` | `0x0E000000` | Texture data pool        | 224 MB                        |
| `0x10000000` | —            | End                      | Total 256 MB                  |

### 2.1 Framebuffer format

Each framebuffer holds one image in **BGRA8888 little-endian** byte order
(standard Linux framebuffer layout). Row 0 is the top of the display. Byte
order per pixel in memory: `B, G, R, A`.

Image dimensions are **host-configurable** via the `FB_WIDTH` / `FB_HEIGHT` /
`FB_STRIDE` control registers (§3.1). The host SHOULD read the active HDMI
resolution from `VIDEO_INFO` (§3.1) and program the framebuffer to match,
so the application can render pixel-perfect for the current display rather
than relying on the framework's scaler. Pitch (`FB_STRIDE`) MUST be at
least `width × 4` bytes per row; rounding up to a hardware-friendly stride
is permitted.

The 8 MB slot size caps the framebuffer at `1920 × 1080 × 4 = 8,294,400`
bytes used (with `94,208` bytes of tail padding). Higher resolutions
require a different memory layout and are out of scope for v0.

### 2.2 Command ring

1 MB of DDR3 configured as a contiguous ring buffer. Size MUST be a power of
two. Commands are 4-byte aligned. See §4 for the protocol.

### 2.3 Texture descriptor table

A flat array of fixed-size 32-byte descriptors indexed by texture ID. The
host writes entries directly with ordinary memory stores; the FPGA reads
entries as needed. See §6.

The **default allocation is 4096 entries** (128 KB) which is comfortably
sized for typical menu usage when atlases share a single descriptor
(§6.4). The count is not a protocol limit — `texture_id` in `COPY_RECT`
is 32 bits, and the FPGA honors whatever count the host programs into
`TEX_TABLE_COUNT` (§3.1). If more entries are ever needed, the host
allocates a larger table in DDR3 and programs the new address and
count at startup; no other protocol changes are required.

### 2.4 Texture data pool

224 MB of host-managed texture storage. The host is solely responsible for
allocation, eviction, and fragmentation. The FPGA treats descriptor `data_addr`
values as raw physical addresses into this region and performs no validation
beyond the top-level reserved-region bounds check.

### 2.5 Host CPU mapping

The host SHOULD map the entire reserved region as **write-combining**
(uncached). The ARM side is a pure writer; never read back pixel data through
this mapping. Write-combining avoids the need for explicit cache maintenance
before FPGA reads.

If the host ever needs to read FPGA-written framebuffer data (e.g., for
screenshots), it MUST first issue a FENCE (§4.7) and then invalidate caches for
the target region — but this is out of scope for v0.

---

## 3. Control register map

The FPGA exposes a block of 32-bit control registers through the Lightweight
HPS-to-FPGA bridge (LW_H2F). The host accesses them via mmap of `/dev/mem`
at physical address `0xFF210000` + register offset.

All registers are 32 bits wide. All reserved bits MUST read as zero and MUST
be written as zero for forward compatibility.

### 3.1 Register table

| Offset | R/W | Name                 | Purpose                                        |
|--------|-----|----------------------|------------------------------------------------|
| `0x00` | R   | `ID`                 | Magic + version                                |
| `0x04` | R   | `STATUS`             | Engine status bits                             |
| `0x08` | R/W | `CONTROL`            | Enable / reset / error-clear                   |
| `0x0C` | R   | `ERROR_INFO`         | Last error code + detail                       |
| `0x10` | R   | `VSYNC_COUNT`        | Monotonic vsync counter                        |
| `0x14` | R   | `FRAME_COUNT`        | Monotonic frames presented                     |
| `0x18` | R/W | `VIDEO_MODE`         | Output mode (0 = follow HDMI, reserved for future) |
| `0x1C` | R   | `VIDEO_INFO`         | Active HDMI dims: `{height[15:0], width[15:0]}` |
| `0x20` | R   | `FB_STATE`           | Packed framebuffer state (display/render/ready); see §3.2 |
| `0x24` | R/W | `FB_WIDTH`           | Framebuffer width in pixels                    |
| `0x28` | R/W | `FB_HEIGHT`          | Framebuffer height in pixels                   |
| `0x2C` | R/W | `FB_STRIDE`          | Framebuffer pitch in bytes per row             |
| `0x30` | R/W | `RING_BASE`          | Command ring base address (physical)           |
| `0x34` | R/W | `RING_SIZE`          | Ring size in bytes (power of 2)                |
| `0x38` | R   | `RING_HEAD`          | FPGA's read offset into ring                   |
| `0x3C` | R/W | `RING_TAIL`          | Host's write offset into ring                  |
| `0x40` | W   | `RING_KICK`          | Write any value to wake the fetcher            |
| `0x44` | —   | reserved             |                                                |
| `0x48` | R   | `FENCE_VALUE`        | Most recent FENCE value reached                |
| `0x4C` | —   | reserved             |                                                |
| `0x50` | R/W | `FB0_ADDR`           | Framebuffer 0 physical address                 |
| `0x54` | R/W | `FB1_ADDR`           | Framebuffer 1 physical address                 |
| `0x58` | R/W | `FB2_ADDR`           | Framebuffer 2 physical address                 |
| `0x5C` | —   | reserved             |                                                |
| `0x60` | R/W | `TEX_TABLE_ADDR`     | Texture descriptor table physical address      |
| `0x64` | R/W | `TEX_TABLE_COUNT`    | Number of valid entries in the table (default allocation 4096; see §2.3) |
| `0x68` | R/W | `LAYER_TABLE_BASE`   | Physical address of the 16 KB layer-table region (two 8 KB tables A/B; see §11) |
| `0x6C` | R/W | `LAYER_COMMIT`       | Atomic frame swap: bit 31 = active table (0=A, 1=B); bits 8..0 = valid layer count (see §11) |
| `0x70` | —   | reserved             |                                                |
| `0x80` | R   | `PERF_CYCLES_BUSY`   | Cycles blit engine was busy (debug)            |
| `0x84` | R   | `PERF_CMDS_EXEC`     | Commands executed (debug)                      |
| `0x88` | R   | `PERF_BYTES_READ`    | Bytes read from DDR3 (debug, ÷64)              |
| `0x8C` | R   | `PERF_BYTES_WRITTEN` | Bytes written to DDR3 (debug, ÷64)             |

Offsets `0x100` onward are reserved for future expansion.

### 3.2 Register bit layouts

#### ID (`0x00`)

```
 31          16 15           0
┌──────────────┬──────────────┐
│  MAGIC=0x1FFA│  VERSION     │
└──────────────┴──────────────┘
```

Version for this spec: `0x0001`. Full value: `0x1FFA0001`.

#### STATUS (`0x04`)

```
 31                              4  3  2  1  0
┌─────────────────────────────────┬──┬──┬──┬──┐
│            reserved             │VS│UR│BZ│ER│
└─────────────────────────────────┴──┴──┴──┴──┘
```

- `ER` (bit 0): error latched — see `ERROR_INFO`
- `BZ` (bit 1): blit engine busy
- `UR` (bit 2): scanout underrun occurred since last STATUS read (sticky)
- `VS` (bit 3): vsync has occurred since last read (self-clearing on read)

#### CONTROL (`0x08`)

```
 31                        3  2  1  0
┌───────────────────────────┬──┬──┬──┐
│         reserved          │CE│SE│EN│
└───────────────────────────┴──┴──┴──┘
```

- `EN` (bit 0): master enable. When 0, scanout outputs black and blit engine is
  idle. Transitioning 0→1 begins operation.
- `SE` (bit 1): soft reset (self-clearing). Writing 1 resets all engines, ring
  head/tail, and error state. Does not clear framebuffer contents.
- `CE` (bit 2): clear error (self-clearing). Writing 1 clears the `ER` status
  bit and resumes ring processing from `RING_HEAD`.

#### ERROR_INFO (`0x0C`)

```
 31                     8  7                 0
┌─────────────────────────┬───────────────────┐
│      error_detail       │    error_code     │
└─────────────────────────┴───────────────────┘
```

See §8 for error codes.

#### FB_STATE (`0x20`)

Packed read-only snapshot of all three framebuffer state fields. A single
AXI-Lite read returns a coherent view — the host is guaranteed not to
observe torn state across the three fields.

```
 31                                     6  5       4  3       2  1       0
┌─────────────────────────────────────────┬──────────┬──────────┬──────────┐
│                reserved                 │ FB_READY │ FB_RENDER│FB_DISPLAY│
└─────────────────────────────────────────┴──────────┴──────────┴──────────┘
```

- `FB_DISPLAY` (bits 1:0): which framebuffer is currently scanning out
  (0, 1, or 2). Value 3 is reserved.
- `FB_RENDER` (bits 3:2): which framebuffer the blit engine is currently
  rendering into (0, 1, or 2). Value 3 is reserved.
- `FB_READY` (bits 5:4): framebuffer waiting to be scanned out at next
  vsync (0, 1, or 2). Value 3 = no frame pending (current display stays).
  Set by FPGA when a PRESENT command completes. Reset to 3 by FPGA when
  swap actually occurs at vsync.

Bits 31:6 are reserved and read as zero.

### 3.3 Reset state

After RBF load, before host programs registers:

- `CONTROL.EN = 0`
- `STATUS = 0`
- `RING_HEAD = RING_TAIL = 0`
- `FB_STATE = 0x34` (FB_DISPLAY=0, FB_RENDER=1, FB_READY=3) — RENDER
  starts at index 1 so the host's first draws don't hit the
  currently-displayed buffer.
- `VSYNC_COUNT`, `FRAME_COUNT` = 0
- HDMI is actively driven with valid 1080p60 timing but all pixels are black.
- All `*_ADDR` registers are 0 (invalid — host MUST program before setting EN).

---

## 4. Command ring protocol

### 4.1 Concept

A single-producer single-consumer (SPSC) ring buffer in DDR3. The host is the
producer, the FPGA is the consumer. Synchronization is via the `RING_HEAD` and
`RING_TAIL` registers exposed over LW_H2F.

- **Empty:** `RING_HEAD == RING_TAIL`
- **Full:** `(RING_TAIL + next_command_bytes) mod RING_SIZE == RING_HEAD`
- Ring size MUST be a power of two so modular arithmetic is a mask.
- Offsets are byte offsets into the ring, 4-byte aligned.

### 4.2 Producer (host) procedure

To submit a command:

1. Read `RING_HEAD`. Compute free bytes:
   `free = (RING_HEAD - RING_TAIL - 4) mod RING_SIZE`
   (Leave one word margin so empty is unambiguous from full.)
2. If the command (including header) does not fit in contiguous bytes between
   `RING_TAIL` and either the ring end or `RING_HEAD`, either:
    - Wait and retry, or
    - If blocked only by ring-end (not by HEAD), emit a NOP-pad to wrap (§4.5).
3. Write the full command (header + args) at offset `RING_TAIL`.
4. Issue a memory barrier to guarantee the writes are visible in DDR3.
   On ARMv7-A: `dsb st` (or `dmb ishst` for writes only).
5. Advance `RING_TAIL` to the byte offset immediately after the command, masked
   to the ring size. Write the new value via LW_H2F.
6. Optionally write any value to `RING_KICK` if the engine may be idle.
   The engine also polls `RING_TAIL` periodically, so this is advisory.

### 4.3 Consumer (FPGA) procedure

The command fetcher:

1. Reads `RING_TAIL`. If `HEAD == TAIL`, idles (with a low-frequency poll or
   wakes on `RING_KICK`).
2. Reads the 4-byte command header at `RING_BASE + RING_HEAD`.
3. Decodes opcode and length. Reads `length_words × 4` additional argument
   bytes from `RING_BASE + RING_HEAD + 4`.
4. Dispatches the command to the appropriate execution unit.
5. When execution completes (for most commands, synchronously at decode time;
   for blit commands, when the blit engine signals done), advances `RING_HEAD`
   by `4 + length_words × 4` bytes, masked to ring size.

The fetcher SHOULD prefetch commands into a small internal FIFO using burst
reads to amortize DDR3 latency.

### 4.4 Memory ordering

The host guarantees that command data is visible in DDR3 before `RING_TAIL` is
updated. The FPGA guarantees that it reads command data only at offsets
strictly less than `RING_TAIL` at the time of reading.

The host's DDR3 writes use write-combining memory, which on Cortex-A9 does not
require explicit cache flushes but DOES require a `dsb` before the register
write to `RING_TAIL` to ensure WC buffers are drained.

### 4.5 Ring wraparound

Commands MUST NOT span the end of the ring. If the next command would not fit
contiguously between `RING_TAIL` and ring end, the host MUST write a NOP
command (§5.1) padding to the end of the ring, then wrap `RING_TAIL` to 0, then
write the real command starting at offset 0.

The FPGA decoder MUST handle NOPs as no-ops and MUST handle any valid NOP
`length_words`, including the case where a NOP pads the ring end.

A convenience: a NOP with `length_words = 0` (size 4 bytes) is legal. To pad
an arbitrary number of bytes to the ring end, use a NOP with the appropriate
`length_words` value.

### 4.6 PRESENT semantics

`PRESENT` (opcode `0x01`) tells the FPGA "the framebuffer currently being
rendered into (`FB_RENDER`) is complete; swap it to display at next vsync".

On PRESENT (immediately, when the fetcher retires the command):

1. Blit engine waits for all prior commands (including their DDR3 writes) to
   retire.
2. `FB_READY` is set to the index of the completed framebuffer (the old
   `FB_RENDER`).
3. `FB_RENDER` is rotated to the third (currently free) buffer index so the
   host can begin issuing draws into the next frame without stalling.
   Subsequent draw commands from the ring apply to the new `FB_RENDER`.

On the next vsync (asynchronously, up to one frame interval later):

4. Scanout atomically switches: `FB_DISPLAY` takes the old value of
   `FB_READY`, and `FB_READY` becomes 3 (empty).
5. `FRAME_COUNT` increments — it counts frames that have *actually been
   shown*, not frames queued by `PRESENT`.

With triple-buffering, the host can issue `PRESENT` without stalling: there is
always a free buffer to render into next. If the host issues two `PRESENT`s
within a single vsync interval, the second overwrites the first in `FB_READY`
— the second scene displays; the first is never seen. This is not an error but
may indicate the host is rendering too fast.

### 4.7 FENCE semantics

`FENCE` (opcode `0x02`) carries a 32-bit user-supplied value. When the command
fetcher retires a FENCE, it writes the value to `FENCE_VALUE`. All prior
commands in ring order have fully completed (all DDR3 writes retired) before
the register update becomes visible to the host.

Host use: monotonically-increasing fence values; host polls `FENCE_VALUE` to
determine the highest fence the FPGA has passed, which implies all draws
before that fence are complete and their source textures may safely be
reused or freed.

---

## 5. Command encoding

### 5.1 Header layout

Every command begins with a 4-byte header:

```
 31       24 23       16 15                    0
┌──────────┬──────────┬────────────────────────┐
│  opcode  │ length_w │        flags           │
└──────────┴──────────┴────────────────────────┘
```

- `opcode` (8 bits): command type (§5.2)
- `length_w` (8 bits): number of 32-bit argument words following the header.
  Total command size in bytes = `4 + length_w × 4`.
- `flags` (16 bits): command-specific bits. Reserved bits MUST be zero.

### 5.2 Opcode table

Opcodes are organized by category in the upper nibble (`opcode[7:4]`),
with the lower nibble (`opcode[3:0]`) selecting within a category. This
lets the FPGA route a command to the right execution unit with a single
4-bit compare before the full 8-bit decode.

| Opcode | Mnemonic   | Length (words) | Description                                             |
|--------|------------|----------------|---------------------------------------------------------|
| `0x00` | NOP        | 0..255         | No-op, used for ring-end padding                        |
| `0x01` | PRESENT    | 0              | Commit current render FB; swap at next vsync            |
| `0x02` | FENCE      | 1              | Synchronization marker; writes value to FENCE_VALUE     |
| `0x03` | SET_CLIP   | 2              | Set clipping rectangle                                  |
| `0x04` | CLEAR_CLIP | 0              | Remove clipping                                         |
| `0x05` | SET_RENDER_TARGET | 1       | Redirect subsequent draws into a texture (§5.7)         |
| `0x10` | FILL_RECT  | 3              | Solid color rectangle (auto-clamped to framebuffer)     |
| `0x11` | COPY_RECT  | 5 or 6         | Textured rectangle (with optional scaling, tint, blend). `length_w = 6` iff `tint_en = 1` |
| `0xFF` | EXTENDED   | —              | Reserved for future protocol extension (see §10); MUST raise `ERR_BAD_OPCODE` in v0 |

Category ranges (upper nibble):

| Range         | Category                                          |
|---------------|---------------------------------------------------|
| `0x0X`        | Control / synchronization                         |
| `0x1X`        | Basic rectangle drawing                           |
| `0x2X`        | Reserved — primitives (lines, circles, polygons) |
| `0x3X`        | Reserved — gradients                              |
| `0x4X`        | Reserved — effects (blur, shadow, color matrix)   |
| `0x5X`        | Reserved — paths / Bezier                         |
| `0x6X`        | Reserved — text / glyph runs                      |
| `0x7X`–`0xEX` | Reserved                                          |
| `0xFX`        | Reserved; `0xFF` is the extension escape          |

All opcodes not listed in the table above are reserved and MUST
produce error code `ERR_BAD_OPCODE` (§8) if encountered by the FPGA.
Opcode `0xFF` is additionally reserved as a future extension escape
(§10.1) and MUST NOT be repurposed for any other command in this
protocol line.

### 5.3 Per-command argument layouts

All coordinates are pixel coordinates in the framebuffer or texture. Origin
is top-left. Coordinates and sizes use unsigned 16-bit values packed two per
32-bit word as `(high_field << 16) | low_field`.

Commands are documented below in category order
(`0x0X` control/flow, then `0x1X` basic drawing).

#### NOP (`0x00`)

Header `flags` ignored. `length_w` specifies padding size. Arguments are
ignored.

#### PRESENT (`0x01`)

No arguments. See §4.6.

#### FENCE (`0x02`)

```
length_w = 1, flags = 0
Word 0: 32-bit fence value (arbitrary host-assigned)
```

See §4.7.

#### SET_CLIP (`0x03`)

```
length_w = 2, flags = 0
Word 0: x (high 16) | y (low 16)
Word 1: w (high 16) | h (low 16)
```

Restricts all subsequent drawing to this rectangle until CLEAR_CLIP or next
SET_CLIP. The effective clip is the intersection of the supplied rectangle
with the framebuffer bounds (§5.5); the host MAY pass a rectangle that
extends beyond the framebuffer and the FPGA will clamp it silently. A
clip rect whose intersection with the framebuffer is empty causes all
subsequent non-`ignore_clip` draws to be no-ops.

#### CLEAR_CLIP (`0x04`)

No arguments. Drawing is unrestricted (equivalent to clip rect equal to
the full framebuffer).

#### SET_RENDER_TARGET (`0x05`)

```
length_w = 1, flags = 0
Word 0: tex_id (low 16) | reserved (high 16, must be 0)
```

The reserved sentinel `tex_id = 0xFFFF` selects the framebuffer (the
default target). Any other `tex_id` selects a texture in the descriptor
table (§6); the FPGA reads that descriptor to obtain `data_addr`,
`width`, `height`, and `pitch_bytes`, and routes subsequent
`FILL_RECT` / `COPY_RECT` writes there. See §5.7.

#### FILL_RECT (`0x10`)

```
length_w = 3
flags    = { reserved[15:3], ignore_clip[2], blend[1:0] }
Word 0:  dx (high 16) | dy (low 16)
Word 1:  dw (high 16) | dh (low 16)
Word 2:  RGBA color
```

- `blend` — 0 = opaque, 1 = src_alpha, 2 = additive, 3 = reserved.
- `ignore_clip` — 0 = honor the current clip rect (default); 1 =
  bypass the user clip rect for this command. The framebuffer-bounds
  clip (§5.5) is NEVER bypassed.

Behavior:

- Destination rect is silently clamped to framebuffer bounds per
  §5.5. Idiom for full-screen fill: `dx=0, dy=0, dw=0xFFFF, dh=0xFFFF`.
- If `dw == 0 || dh == 0`, or the dest rect intersection with the
  effective clip is empty, the command is a no-op.
- Blend modes (§7) apply. Opaque is the fast path (pure write, no
  dest read-modify-write); the FPGA MAY further specialize the case
  of full-framebuffer dest + opaque + `ignore_clip = 1` as a fast
  clear.

#### COPY_RECT (`0x11`)

COPY_RECT uses the variable-length argument convention described in
§5.4. The header is followed by five always-present base words, then
zero or more optional words in canonical order as enabled by flag
bits.

Flag field:
```
flags = { reserved[15:5], tint_en[4], filter[3:2], blend[1:0] }
```

Base arguments (always present, 5 words):
```
Word 0: texture_id (32-bit)
Word 1: sx (high 16) | sy (low 16)
Word 2: sw (high 16) | sh (low 16)
Word 3: dx (high 16) | dy (low 16)
Word 4: dw (high 16) | dh (low 16)
```

Optional arguments, in canonical order (each appended only when its
flag is set):

| Slot | Gate flag | Length | Payload | Status |
|------|-----------|--------|---------|--------|
| 1    | `tint_en` (bit 4) | 1 word | tint RGBA | v0 |
| 2..  | reserved (bits 5..15) | — | — | future (rotation, skew, color matrix, …) |

Field semantics:

- `blend` — 0 = opaque, 1 = src_alpha, 2 = additive, 3 = reserved.
- `filter` — 0 = nearest (only mode supported in v0). Other values
  reserved for future bilinear and similar.
- `tint_en` — when 1, the next word after the base is a tint RGBA
  value, and the source texel is multiplied by `tint` before
  blending. When 0, RGBA8888 texels are used unmodified; A8 texels
  render with an implicit white tint (`0xFFFFFFFF`, see §7.3).

Other behavior:

- If `sw == dw && sh == dh`, no scaling (fast path). Otherwise
  nearest-neighbor scaling per §7.4.
- Destination rect is clamped to framebuffer bounds; source rect is
  clamped proportionally to preserve texture alignment. See §5.5.
- Clipped by the current user clip rect.
- `length_w` MUST equal `5 + Σ enabled_optional_lengths`; a mismatch
  produces `ERR_BAD_LENGTH` per §5.4.

### 5.4 Optional arguments and extensibility

Some commands accept optional argument words that are present only
when a specific flag bit in the header is set. `COPY_RECT` is the
first such command; future commands are expected to use the same
pattern for rotation, skew, color matrix, shadow, and other
orthogonal extensions.

**Rules for optional arguments:**

- **Base arguments always present.** The required fixed arguments
  appear immediately after the header.
- **Optional arguments follow in canonical order.** Each optional
  argument (or argument group) is gated by a specific flag bit
  defined in the command's spec. When enabled, optional arguments
  appear after the base arguments in the order declared by the
  command — *not* in the numeric order of the flag bits, and *not*
  in an order chosen by the host.
- **`length_w` MUST match the flag-implied length.** The host
  computes:
  ```
  length_w = base_length + Σ (flag_i_set ? optional_i_length : 0)
  ```
  The FPGA computes the same sum independently from the flag bits
  and compares against the host-supplied `length_w`. A mismatch
  MUST produce `ERR_BAD_LENGTH` (§8).
- **Reserved flag bits MUST be zero in v0.** They are reserved for
  future optional arguments. When a future protocol version defines
  a new optional argument, the corresponding flag bit becomes
  meaningful and a new slot is appended to the canonical order
  *after* all existing slots. Existing flag bits and slot positions
  never move.

**Why this encoding:**

- Forward-compatible: adding a new optional argument is a
  backwards-compatible spec change (§10) — existing host code
  leaves the new flag bit at zero and the FPGA produces the same
  result as before.
- Minimal on-wire overhead: no tag bytes per optional argument.
  Cost is one bit of header flag space per option.
- Simple FPGA decode: a small adder tree sums the enabled optional
  lengths, producing the expected `length_w` for validation. No
  TLV parser needed.

**Limit:** the 16-bit flag field bounds the maximum number of
independently-gated optional arguments per command to 16 minus the
bits used for non-optional purposes (blend mode, filter, etc.). If a
command ever needs more than this, it should define a sub-opcode in
the same category range rather than crowding the flag field.

### 5.5 Drawing bounds and clipping

All drawing commands that write to the framebuffer (`FILL_RECT`,
`COPY_RECT`) are implicitly clamped to the active framebuffer bounds
`[0, fb_w) × [0, fb_h)`. The FPGA writes nothing outside the render
target regardless of command arguments or clip-rect state. This is
an unconditional hardware-enforced boundary and cannot be disabled.

Two layers of clipping apply in order:

1. **Framebuffer bounds** (outer, always active). Source of truth for
   `fb_w` / `fb_h` is the current video mode. Host does not need to
   know these values to stay within them.
2. **User clip rect** (inner, via `SET_CLIP`/`CLEAR_CLIP`). The
   effective inner clip is the intersection of the SET_CLIP rectangle
   with the framebuffer bounds. `ignore_clip = 1` on a drawing command
   disables this inner layer for that command only; the outer
   framebuffer-bounds layer still applies.

Effective draw region =
  `framebuffer_bounds ∩ (ignore_clip ? ALL : user_clip)`.

**Consequence for the host:** any destination rectangle may be
specified with oversized dimensions. The idiom
```
FILL_RECT(dx=0, dy=0, dw=0xFFFF, dh=0xFFFF, color, blend=opaque, ignore_clip=1)
```
fills the entire framebuffer at any video mode, without the host
needing to read back the current resolution. Partially off-screen
rectangles (e.g., a UI element dragged near the edge) work naturally.

**Consequence for COPY_RECT with scaling:** when the destination rect
is clamped, the source rect is clamped proportionally to preserve
texture alignment. For each edge where `n` destination pixels are
cut, the corresponding source coordinate shifts by
`n * source_extent / destination_extent` on that axis. Degenerates to
"shift source by the same amount" for non-scaled (`sw == dw,
sh == dh`) copies.

**Consequence for SET_CLIP:** the clip rectangle may be any
value — the FPGA intersects it with the framebuffer bounds. There is
no error for an out-of-bounds clip. A clip whose intersection with
the framebuffer is empty causes subsequent non-`ignore_clip` draws to
be no-ops.

### 5.6 Render-to-texture (`SET_RENDER_TARGET`)

The default render target is the framebuffer (the back-buffer being
prepared for the next `PRESENT`). `SET_RENDER_TARGET tex_id` redirects
subsequent draws into a texture's pixel data:

```
SET_RENDER_TARGET tex_id_a       -- subsequent draws write into tex A
FILL_RECT ...
COPY_RECT ...
SET_RENDER_TARGET 0xFFFF         -- back to framebuffer
COPY_RECT tex_id_a, ...          -- now read from tex A
```

**Constraints (v1):**

- The target texture's `format` MUST be `RGBA8888` (`0`); A8 RTT is not
  supported.
- The target texture's `pitch_bytes` MUST equal `width * 4` (no
  sub-region RTT into a larger atlas).
- `SET_CLIP` continues to apply with target dimensions in place of the
  framebuffer dimensions. The clip's intersection with the target
  bounds is what gets enforced. `CLEAR_CLIP` reverts to the full
  current target.
- `PRESENT` MUST be issued with the framebuffer as the active target.
  Issuing `PRESENT` while another target is active is a host bug;
  behaviour is unspecified (the FPGA may silently retire it without
  swap, or raise `ERR_BAD_OPCODE` — implementations choose).

**Synchronization:**

A texture written via RTT in one batch and sampled via `COPY_RECT` in
another batch MUST have a `FENCE` retired between them. Within the
same submitted batch, write-then-read ordering on the same texture is
implementation-defined and SHOULD be avoided.

**Default target on reset:**

`CONTROL.SE = 1` (soft reset) and `CONTROL.CE = 1` (clear error) both
reset the active target to the framebuffer. `CONTROL.E = 1` (enable)
leaves the active target untouched.

---

## 6. Texture descriptors

The texture descriptor table is a flat array of fixed 32-byte entries stored
in DDR3. The host writes entries directly; there is no "upload texture"
command. The FPGA reads the descriptor when it needs it (i.e., during
COPY_RECT execution).

### 6.1 Descriptor layout (32 bytes)

```
Offset Size Field
────── ──── ─────
0x00    4   data_addr       Physical address of pixel data in DDR3
0x04    4   pitch_bytes     Bytes per row (may exceed width * bpp for padding)
0x08    2   width           Width in pixels (1..65535)
0x0A    2   height          Height in pixels (1..65535)
0x0C    1   format          0 = RGBA8888, 1 = A8
0x0D    1   flags           Reserved, MUST be 0
0x0E    2   reserved        MUST be 0
0x10    16  reserved        MUST be 0, available for future fields
```

Texture ID 0 is reserved as "null texture" and MUST NOT be used in COPY_RECT.

### 6.2 Format codes

| Code | Name     | Bytes/pixel | Description                                           |
|------|----------|-------------|-------------------------------------------------------|
| `0`  | RGBA8888 | 4           | Standard color + alpha, BGRA byte order in memory     |
| `1`  | A8       | 1           | Alpha only, RGB implied as (255, 255, 255). See §7.3. |

Other format codes are reserved for future use (RGB565, RGBA4444, paletted,
etc.) and MUST produce `ERR_BAD_FORMAT` if encountered.

### 6.3 Byte order for RGBA8888 textures

Like the framebuffer: in memory, each pixel is `B, G, R, A` in ascending
addresses. This matches the framebuffer layout and avoids byte swapping in
the blit engine for opaque copies.

### 6.4 Pitch and atlas usage

`pitch_bytes` MUST be ≥ `width * bytes_per_pixel` and MAY be larger to
describe rows within a packed atlas.

#### Recommended: one descriptor per atlas, sub-region via `COPY_RECT`

The preferred way to use a sprite sheet or font atlas is to store the
whole atlas as **a single descriptor** covering the full image, then
select regions at draw time via `COPY_RECT`'s `sx / sy / sw / sh`
source-rectangle parameters (§5.3). The host keeps a side table of
sub-region metadata (glyph metrics, sprite frames, etc.) — those
tuples live in ordinary host memory, not in FPGA-visible descriptors.

This pattern keeps descriptor-table usage proportional to the number of
**atlases** rather than the number of renderable elements, which is
typically 1–2 orders of magnitude smaller. A UI with thousands of
glyphs across several fonts and sizes consumes only a handful of
descriptor slots.

#### Alternative: per-region descriptors

A descriptor may also point into the middle of a larger atlas by
setting `data_addr` to the region's top-left pixel and `pitch_bytes`
to the atlas stride. The descriptor's `width` / `height` describe only
the sub-region. Use this when a sub-region needs persistent metadata
(e.g., a pre-clipped animation frame used with many different tint
colors), but avoid it for routine glyph or tile rendering — it
consumes descriptor slots that the atlas pattern above would save.

### 6.5 Descriptor writes and cache coherency

The host writes descriptors through the write-combining mapping. Before
issuing any COPY_RECT that references a newly-written descriptor, the host
MUST issue a `dsb` to ensure the descriptor bytes are visible in DDR3. This
is typically done implicitly by the `dsb` before `RING_TAIL` advance (§4.2).

The FPGA MAY cache the N most recently used descriptors in on-chip RAM. If
the host modifies an in-use descriptor, the host MUST either wait for a
FENCE to confirm no command references it, or use a new texture ID.

---

## 7. Pixel formats and blending

### 7.1 Framebuffer and color representation

All colors in commands use RGBA8888 format with A in the most significant
byte:

```
 31      24 23      16 15       8 7        0
┌──────────┬──────────┬──────────┬──────────┐
│    A     │    R     │    G     │    B     │
└──────────┴──────────┴──────────┴──────────┘
```

In memory (framebuffer and RGBA8888 textures), pixels are stored little-endian
such that the byte at the lowest address is B, then G, R, A. A 32-bit word
loaded from memory will have the layout above when interpreted on the ARM
(little-endian). This makes host command construction and memory storage
consistent.

### 7.2 Blend modes

With `src` the (possibly tinted) source pixel and `dst` the destination pixel
currently in the framebuffer:

| Mode      | `blend[1:0]` | Formula                                                                                                                 |
|-----------|--------------|-------------------------------------------------------------------------------------------------------------------------|
| Opaque    | 0            | `dst = src` (alpha written through)                                                                                     |
| Src alpha | 1            | `dst.rgb = src.rgb * src.a + dst.rgb * (255 - src.a)` (÷255); `dst.a = src.a + dst.a * (255 - src.a)` (÷255, saturated) |
| Additive  | 2            | `dst.rgb = min(src.rgb + dst.rgb, 255)`; `dst.a = min(src.a + dst.a, 255)`                                              |

"÷255" in practice is implemented as `(x * 257 + 32768) >> 16` or
`(x + (x>>8) + 1) >> 8` — both are exact for the 0..255×255 range. The
implementation MUST be deterministic; any choice is acceptable as long as it
is documented and tested with a reference implementation.

For FILL_RECT, `src` is the specified color.
For COPY_RECT with RGBA source, `src` is the sampled texel (possibly
modulated by tint).
For COPY_RECT with A8 source, `src` is the tint color with `src.a = texel * tint.a / 255`.

### 7.3 A8 textures

A8 textures are used primarily for font glyphs and masks. The single byte per
pixel represents alpha. When sampled:

- If `tint_en = 1` (tint word present): `src.rgb = tint.rgb`,
  `src.a = texel × tint.a ÷ 255`.
- If `tint_en = 0` (no tint word): an implicit white tint of
  `0xFFFFFFFF` is used, giving `src.rgb = (255, 255, 255)` and
  `src.a = texel`. This produces a white glyph — useful when the
  desired color is white or when the output is being composited
  against a single-color background.

Use with `blend = src_alpha` for anti-aliased text rendering. With
`tint_en = 1`, `tint.rgb` is the text color and the blend produces
properly-blended subpixel-quality edges.

### 7.4 Scaling (nearest neighbor)

When `sw != dw || sh != dh`, the blit engine samples the source at:

```
src_x = sx + floor((i + 0.5) * sw / dw)    for i in 0..dw
src_y = sy + floor((j + 0.5) * sh / dh)    for j in 0..dh
```

Implementation: fixed-point accumulator per axis (no divider needed). Upscaling
and downscaling are both supported. Filter mode `filter != 0` is reserved
for future bilinear support.

### 7.5 Tinting

When `tint_en = 1` (and format is RGBA8888):

```
src.r = texel.r * tint.r / 255
src.g = texel.g * tint.g / 255
src.b = texel.b * tint.b / 255
src.a = texel.a * tint.a / 255
```

Tinting is applied before blending.

---

## 8. Error handling

### 8.1 Error codes

| Code   | Name                   | Detail field                                       |
|--------|------------------------|----------------------------------------------------|
| `0x00` | `ERR_NONE`             | 0                                                  |
| `0x01` | `ERR_BAD_OPCODE`       | The unknown opcode value                           |
| `0x02` | `ERR_BAD_LENGTH`       | Expected vs. actual length (packed: exp<<8 \| got) |
| `0x03` | `ERR_BAD_TEXTURE`      | Texture ID that was out of range or invalid        |
| `0x04` | `ERR_BAD_FORMAT`       | Unrecognized format code                           |
| `0x05` | reserved               | Previously `ERR_BAD_CLIP`; freed by §5.5 auto-clamping |
| `0x06` | `ERR_AXI`              | AXI response code (SLVERR=2, DECERR=3)             |
| `0x07` | `ERR_SCANOUT_UNDERRUN` | Line number where underrun occurred                |
| `0x08` | `ERR_RING_OVERRUN`     | FPGA saw TAIL move backwards unexpectedly          |

### 8.2 Error semantics

When the FPGA detects an error:

1. It stops advancing `RING_HEAD` immediately after the offending command.
2. It writes the error code and detail into `ERROR_INFO`.
3. It sets `STATUS.ER = 1`.
4. The blit engine completes any in-flight pixel writes and idles.
5. Scanout continues normally (errors do not blank the display).

Recovery:

1. Host reads `ERROR_INFO` and logs it.
2. Host may forcibly advance `RING_HEAD` by writing `RING_HEAD` directly —
   allowed only when `STATUS.ER = 1`. Otherwise `RING_HEAD` is read-only.
3. Host writes `CONTROL.CE = 1` to clear the error state. The fetcher
   resumes from the current `RING_HEAD`.

Alternately, the host can issue a soft reset (`CONTROL.SE = 1`), which clears
all state including ring pointers and error.

### 8.3 Scanout underruns

If the scanout engine's line buffer underflows (because the DDR3 read
bandwidth was insufficient), `STATUS.UR` is set sticky. The display will show
corruption for that line. `ERR_SCANOUT_UNDERRUN` is not latched into
`ERROR_INFO` unless it repeats — transient underruns just bump the sticky bit.
Persistent underruns (e.g., 16 consecutive frames) DO latch the error and halt.

---

## 9. Startup sequence

### 9.1 FPGA side

After RBF load:

1. All PLLs lock, clocks stable.
2. Video timing generator starts producing 1080p60 timing. Scanout outputs
   black because `EN = 0`.
3. ADV7513 is configured by Linux (outside this spec; MiSTer sys_top handles
   the I2C init).
4. Control registers at reset values (§3.3).
5. Fetcher and blit engine idle.

### 9.2 Host side

Before any drawing can occur:

1. Open `/dev/mem` with `O_RDWR | O_SYNC`, then `mmap` the reserved DDR3
   region (offset = base physical address, length = 256 MB) with `PROT_READ
   | PROT_WRITE` and `MAP_SHARED`. The MiSTer kernel is already configured
   with `mem=511M memmap=513M$511M`, reserving `[0x1FF00000, 0x40000000)`;
   the 256 MB carve-out at `0x30000000` fits inside that region and
   requires no kernel changes.
2. `O_SYNC` produces a device-uncached mapping (not true write-combining).
   Each ARM store becomes a full DDR3 transaction, yielding ~40–80 MB/s
   write throughput. This is acceptable for command submission and
   descriptor updates. For faster bulk texture uploads, a future `uio`
   kernel module using `pgprot_writecombine` is planned but not required
   for v0.
3. `memset` framebuffers 0, 1, 2 to zero (black).
4. `memset` texture descriptor table to zero.
5. `mmap` LW_H2F control region at `0xFF210000` (size 4 KB is sufficient).
6. Read `ID`; verify magic `0x1FFA` and supported version.
7. Write `FB0_ADDR`, `FB1_ADDR`, `FB2_ADDR` with the framebuffer physical
   addresses.
8. Write `RING_BASE` and `RING_SIZE` (must be power of 2).
9. Write `TEX_TABLE_ADDR` and `TEX_TABLE_COUNT`.
10. Write `CONTROL = 0x01` (`EN = 1`).
11. Begin submitting commands. The first frame MUST start with a
    full-screen `FILL_RECT` (`dx=0, dy=0, dw=0xFFFF, dh=0xFFFF`,
    `ignore_clip=1`, `blend=opaque`) because FB_RENDER contents are
    undefined after reset.

### 9.3 Shutdown

Before unloading the menu core (to switch to an emulator core):

1. Stop submitting commands.
2. Wait until `RING_HEAD == RING_TAIL` (engine idle).
3. Optionally issue a final full-screen `FILL_RECT` (black, opaque,
   `ignore_clip = 1`) + `PRESENT` to clear the display.
4. Write `CONTROL = 0x00` (`EN = 0`).
5. Unmap memory regions.
6. Trigger MiSTer core swap via existing firmware mechanisms.

---

## 10. Versioning and evolution

The `ID` register's low 16 bits hold the protocol version. This document is
version `0x0001` (1).

Rules for backwards-compatible changes (no version bump):

- Adding new opcodes (with opcodes that were previously reserved).
- Adding new flag bits (previously reserved-zero).
- Adding new format codes.
- Adding new control registers at previously-reserved offsets.
- Adding new reserved fields to descriptors (in existing 16-byte reserved
  space).

Rules requiring a version bump:

- Changing existing command argument layouts.
- Changing existing register layouts.
- Changing blend math.
- Changing byte order or pixel format of existing formats.
- Adding new required setup steps.

Host code MUST read the `ID` register at startup and fail cleanly if the
version is not one it supports.

### 10.1 Opcode `0xFF` — extension escape

Opcode `0xFF` is reserved across all versions of this protocol as an
extension escape. A future protocol version MAY define `0xFF` such that
the first argument word following the header carries a 32-bit extended
opcode, effectively providing a 2³²-wide future opcode space without
altering the v0 header layout. Implementations MUST NOT repurpose
`0xFF` for any other command while this protocol line is in effect. In
v0, encountering `0xFF` MUST raise `ERR_BAD_OPCODE`.

---

## 11. Layer compositor (Phase 2+)

The compositor scanout path renders the active framebuffer pixel-by-pixel
from a host-managed table of layer descriptors instead of a pre-rendered
buffer. It runs in parallel with the existing FB-scanout path and is
selected by which control registers the host populates: programming
`LAYER_TABLE_BASE` and committing through `LAYER_COMMIT` (§3.1) is
sufficient to engage the compositor.

### 11.1 Layer-table region

The host reserves a 16 KB region in the DDR3 carve-out (default offset
`0x01B0_0000`, see §2). The region holds two back-to-back tables:

| Region   | Offset within region | Size  | Slots |
|----------|---------------------|-------|-------|
| Table A  | `+0x0000`           | 8 KB  | 256   |
| Table B  | `+0x2000`           | 8 KB  | 256   |

Each slot is a 32-byte layer descriptor (see `protocol::LayerDescriptor`
in the host crate for the byte-level layout). Slot index encodes z-order:
slot 0 paints first (back), slot 255 last (front).

### 11.2 Atomic commit

The compositor reads from whichever of A/B is currently active. The host
prepares a frame by:

1. Writing every changed slot in the **inactive** table.
2. Writing `LAYER_COMMIT` with `bit 31` flipped and `bits 8..0` set to the
   number of populated slots.

`LAYER_COMMIT` is read by the FPGA atomically — the active-table flip
and the valid-count update are observed simultaneously, so a scanline in
flight cannot see a torn commit.

### 11.3 Solid-colour layers (Phase 2a)

A layer with `tex_id == 0xFFFF` is treated as solid: the compositor
paints `color` (BGRA) over the layer's `dst` rectangle and never
references a texture. Phase 2a supports only solid layers; textured
layers (`tex_id < 0xFFFF`) are reserved for Phase 2b and ignored by the
compositor until then.

---

## Appendix A — Sizes at a glance

| Item                        | Value                            |
|-----------------------------|----------------------------------|
| Reserved DDR3 region        | 256 MB                           |
| Framebuffer (each)          | 8 MB (1920×1080×4 + padding)     |
| Framebuffers total          | 24 MB (3×)                       |
| Command ring                | 1 MB                             |
| Texture descriptor table    | 128 KB (4096 × 32 B)             |
| Texture data pool           | 224 MB                           |
| Pixel clock                 | 148.5 MHz                        |
| Peak scanout bandwidth      | ~475 MB/s (1920×1080×4×60)       |
| Target blit throughput (v0) | 2 px/cycle @ 150 MHz = 300 Mpx/s |

## Appendix B — Quick command reference

```
Header: [ opcode:8 | length_words:8 | flags:16 ]

Category 0x0X — control / synchronization
NOP          00 LL 0000  [LL×4 bytes ignored]
PRESENT      01 00 0000
FENCE        02 01 0000  [value]
SET_CLIP     03 02 0000  [x|y] [w|h]
CLEAR_CLIP   04 00 0000

Category 0x1X — basic rectangle drawing
FILL_RECT    10 03 00CB  [dx|dy] [dw|dh] [rgba]               C=ignore_clip (bit 2)
                                                              B=blend (bits 1:0)
COPY_RECT    11 05 00FB  [tex_id] [sx|sy] [sw|sh]             (tint_en=0)
                         [dx|dy] [dw|dh]                      F=filter (bits 3:2)
                                                              B=blend (bits 1:0)
COPY_RECT    11 06 TTFB  [tex_id] [sx|sy] [sw|sh]             (tint_en=1)
                         [dx|dy] [dw|dh] [tint_rgba]          TT=tint_en<<4
                                                              F=filter<<2, B=blend

Full-screen clear idiom:
FILL_RECT    10 03 0004  [0|0] [FFFF|FFFF] [rgba]             ignore_clip=1, opaque

Extension
EXTENDED     FF  — reserved; MUST raise ERR_BAD_OPCODE in v0
```
