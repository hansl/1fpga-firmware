# rtl/ — core-specific RTL

This directory holds menu-core-specific Verilog/SystemVerilog. The
top-level glue module (`emu`) lives one level up in
`../menu_core.sv`; everything it instantiates should live here.

## Current contents

Empty — the M1 milestone (black screen) needs nothing beyond the glue
in `../menu_core.sv`. The system clock is a passthrough of `CLK_50M`,
and scanout is entirely handled by the framework's MISTER_FB path.

## Planned contents (v1 onward)

| File / dir            | Purpose                                              |
|-----------------------|------------------------------------------------------|
| `pll/`                | Altera PLL megafunction wrappers (blit clock, etc.)  |
| `control_regs.sv`     | LW_H2F Avalon-MM slave exposing §3 registers         |
| `ring_fetcher.sv`     | SPSC ring buffer consumer, prefetches commands       |
| `cmd_decoder.sv`      | Opcode dispatch, flag/length validation              |
| `blit_engine.sv`      | FILL_RECT / COPY_RECT execution, blending, clipping  |
| `tex_descriptor.sv`   | Reads 32-byte descriptors from DDR3                  |
| `fb_state.sv`         | Tracks FB_DISPLAY / FB_RENDER / FB_READY per §3.2    |

The framework's MISTER_FB scanout remains the output path; our blit
engine writes pixels into whichever framebuffer slot is currently
`FB_RENDER`, and we hand off to `FB_DISPLAY` on PRESENT at vsync.

## Adding files

Files must be added to `../files.qip` explicitly. Quartus's IDE-driven
file addition mangles the `.qsf` — do it manually in `files.qip`.
