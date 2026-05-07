// Core-side PLL.
//
// MiSTer's framework expects every core to provide an `rtl/pll/pll.v`
// that produces the system clock from CLK_50M. The framework's
// sys/pll_q17.qip references rtl/pll.qip, which in turn references
// this file. Without it, sys_top's clock-select blocks fail synthesis
// because their `inclk[3]` input must come from a PLL output, not a
// raw input pin (Cyclone V routing constraint, error 15836).
//
// Outputs:
//   outclk_0 — 49.5 MHz system clock for the blit engine, ring
//              fetcher, register file, etc. (NOT 50 MHz: fractional
//              VCO doesn't include exactly-50 MHz as a legal output
//              counter setting; 49.5 MHz is the closest legal value.
//              Functionally equivalent — 1 % slower than the prior
//              integer-PLL config, transparent to all downstream
//              consumers including the framework's DDR3 controller.)
//   outclk_1 — 148.5 MHz video clock used by the compositor scanout
//              (see rtl/compositor/compositor.sv) for 1080p60 output.
//              Both outputs share VCO = 1485 MHz; outclk_0 = VCO/30,
//              outclk_1 = VCO/10. Single fractional PLL instance.

module pll(
	input  refclk,
	input  rst,
	output outclk_0,
	output outclk_1,
	output locked
);

altera_pll #(
	.fractional_vco_multiplier("true"),
	.reference_clock_frequency  ("50.0 MHz"),
	.operation_mode             ("normal"),
	.number_of_clocks           (2),
	.output_clock_frequency0    ("49.500000 MHz"),
	.phase_shift0               ("0 ps"),
	.duty_cycle0                (50),
	.output_clock_frequency1    ("148.500000 MHz"),
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
