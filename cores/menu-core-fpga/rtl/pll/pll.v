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
//   outclk_1 = 100 MHz → clk_video. Drives the compositor and
//              scanline_filter (native 1080p scanout). VCO = 600 MHz
//              with M=12 gives clean integer dividers for both
//              outputs (50 = 600/12, 100 = 600/6). Tried 90 MHz
//              briefly when Phase 2c step 1's painter was over
//              budget at 100 MHz, but no in-range VCO produces both
//              50 and 90 with integer dividers — Quartus synthesised
//              fractional and the resulting jitter broke timing on
//              clocks we didn't touch (FPGA_CLK1_50, clk_sys). The
//              real fix lived in the painter: cut MAX_ACTIVE 16->8
//              and removed the second 16-deep scan.
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
	.output_clock_frequency1    ("100.000000 MHz"),
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
