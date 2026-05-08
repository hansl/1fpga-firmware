// Core-side PLL.
//
// MiSTer's framework expects every core to provide an `rtl/pll/pll.v`
// that produces the system clock from CLK_50M. The framework's
// sys/pll_q17.qip references rtl/pll.qip, which in turn references
// this file. Without it, sys_top's clock-select blocks fail synthesis
// because their `inclk[3]` input must come from a PLL output, not a
// raw input pin (Cyclone V routing constraint, error 15836).
//
// Outputs (integer multipliers — VCO = 800 MHz, well within the
// Cyclone V's 600-1300 MHz range):
//   outclk_0 — 50 MHz system clock for the blit engine, ring fetcher,
//              register file, etc. (= VCO / 16)
//   outclk_1 — 200 MHz video clock for the framework's video_mixer.
//              video_mixer's CLK_VIDEO needs to run at ≥ 4× the actual
//              pixel rate (the comment in sys/video_mixer.sv says
//              "should be multiple by (ce_pix*4)") so ASCAL/HDMI
//              capture stages have the headroom they expect. 200 MHz
//              is 4× our 50 MHz pixel rate; compositor gates itself
//              with a ce_pix that pulses 1-in-4 cycles. (= VCO / 4)

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
	.output_clock_frequency1    ("200.000000 MHz"),
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
