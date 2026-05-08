// Core-side PLL.
//
// MiSTer's framework expects every core to provide an `rtl/pll/pll.v`
// that produces the system clock from CLK_50M. The framework's
// sys/pll_q17.qip references rtl/pll.qip, which in turn references
// this file. Without it, sys_top's clock-select blocks fail synthesis
// because their `inclk[3]` input must come from a PLL output, not a
// raw input pin (Cyclone V routing constraint, error 15836).
//
// 50 MHz integer pass-through. The compositor scanout (Phase 1+) uses
// the same clock as both pixel clock and system clock; the framework's
// ASCAL handles upscaling our compositor output to the HDMI mode the
// user has configured. A fractional VCO would allow generating a
// 1080p60-native 148.5 MHz pixel clock here, but the (49.5, 148.5) MHz
// pair pushes the VCO past Cyclone V's ~1300 MHz upper limit and the
// PLL fails to lock at runtime — staying with integer multiplication
// is reliable and the perf hit is tiny (compositor doesn't need to
// run that fast for our use case).

module pll(
	input  refclk,
	input  rst,
	output outclk_0,
	output locked
);

altera_pll #(
	.fractional_vco_multiplier("false"),
	.reference_clock_frequency  ("50.0 MHz"),
	.operation_mode             ("normal"),
	.number_of_clocks           (1),
	.output_clock_frequency0    ("50.000000 MHz"),
	.phase_shift0               ("0 ps"),
	.duty_cycle0                (50),
	.output_clock_frequency1    ("0 MHz"),
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
	.outclk    (outclk_0),
	.locked    (locked),
	.fboutclk  (),
	.fbclk     (1'b0)
);

endmodule
