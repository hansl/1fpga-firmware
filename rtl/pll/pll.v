// Core-side PLL.
//
// MiSTer's framework expects every core to provide an `rtl/pll/pll.v`
// that produces the system clock from CLK_50M. The framework's
// sys/pll_q17.qip references rtl/pll.qip, which in turn references
// this file. Without it, sys_top's clock-select blocks fail synthesis
// because their `inclk[3]` input must come from a PLL output, not a
// raw input pin (Cyclone V routing constraint, error 15836).
//
// Two outputs, both from a 600 MHz VCO (= 50 MHz × 12), well within
// the Cyclone V's 600-1300 MHz fractional-PLL range:
//
//   outclk_0 = 50 MHz  → clk_sys. Drives the blit engine, ring
//              fetcher, register file, and layer_dma. Proven stable
//              at 50 MHz with positive slack on all paths.
//   outclk_1 = 90 MHz  → clk_video. Drives the compositor and
//              scanline_filter (native 1080p scanout). Dropped from
//              100 MHz to 90 MHz once Phase 2c step 1 added a second
//              16-deep scan (`other_tex_hit_c`) and the SrcAlpha
//              blend math to the painter — the two simultaneous
//              scans + blend just barely missed 10 ns timing on
//              Cyclone V SE-A6. 11.1 ns is comfortable.
//
// The two are related (same PLL); Quartus times paths between them as
// synchronous unless menu_core.sdc explicitly marks CDC synchronizers
// as false_paths.

module pll(
	input  refclk,
	input  rst,
	output outclk_0,
	output outclk_1,
	output locked
);

altera_pll #(
	.fractional_vco_multiplier("false"),
	.reference_clock_frequency  ("50.0 MHz"),
	.operation_mode             ("normal"),
	.number_of_clocks           (2),
	.output_clock_frequency0    ("50.000000 MHz"),
	.phase_shift0               ("0 ps"),
	.duty_cycle0                (50),
	.output_clock_frequency1    ("90.000000 MHz"),
	.phase_shift1               ("0 ps"),
	.duty_cycle1                (50),
	.output_clock_frequency2    ("0 MHz"),
	.phase_shift2               ("0 ps"),
	.duty_cycle2                (50),
	.output_clock_frequency3    ("0 MHz"),
	.phase_shift3               ("0 ps"),
	.duty_cycle3                (50),
	.output_clock_frequency4    ("0 MHz"),
	.phase_shift4               ("0 ps"),
	.duty_cycle4                (50),
	.output_clock_frequency5    ("0 MHz"),
	.phase_shift5               ("0 ps"),
	.duty_cycle5                (50),
	.output_clock_frequency6    ("0 MHz"),
	.phase_shift6               ("0 ps"),
	.duty_cycle6                (50),
	.output_clock_frequency7    ("0 MHz"),
	.phase_shift7               ("0 ps"),
	.duty_cycle7                (50),
	.output_clock_frequency8    ("0 MHz"),
	.phase_shift8               ("0 ps"),
	.duty_cycle8                (50),
	.pll_type                   ("General"),
	.pll_subtype                ("General")
) altera_pll_i (
	.rst       (rst),
	.refclk    (refclk),
	.outclk    ({outclk_1, outclk_0}),
	.locked    (locked),
	.fboutclk  (),
	.fbclk     (1'b0)
);

endmodule
