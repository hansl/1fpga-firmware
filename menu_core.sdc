derive_pll_clocks
derive_clock_uncertainty

# core specific constraints
#
# The framework's sys/sys_top.sdc declares clock groups using wildcards
# that expect the core's PLL to be hierarchically `*|pll|pll_inst|...`,
# but our PLL is instantiated directly as `pll_inst` inside `emu` — so
# it lives at `emu|pll_inst|altera_pll_i|...` and doesn't match. Without
# this declaration, Quartus treats clk_sys as synchronous to pll_hdmi /
# pll_audio / h2f_user0_clk and analyses billions of impossible paths
# between them, producing massive negative slack (~ -17 ns) that
# corrupts the placement and breaks HDMI generation.
#
# The wildcard `*|divclk` matches BOTH our PLL outputs (clk_sys at
# `general[0].gpll~...|divclk` and clk_video at `general[1].gpll~...|
# divclk`). They share the same async-group, which means Quartus will
# still try to time paths *between* them as synchronous (same PLL,
# related clocks). Inter-domain paths are confined to the two CDC
# synchroniser chains below; see false_path declarations.
set_clock_groups -asynchronous \
    -group [get_clocks {emu|pll_inst|altera_pll_i|*|divclk}] \
    -group [get_clocks {pll_hdmi|*|divclk}] \
    -group [get_clocks {pll_audio|*|divclk}] \
    -group [get_clocks {*|h2f_user0_clk}]

# CDC synchroniser between clk_sys (50 MHz) and clk_video (100 MHz).
# The first stage accepts metastability; the second produces a stable,
# late-arriving value. Marking the first stage as false-path-to keeps
# Quartus from trying to close timing on the inter-domain leg.
# (The compositor-v2 layer/texture CDC chains were removed.)
set_false_path -to [get_registers {emu:emu|comp_rst_n_sync_0}]

# f2sdram ram2 boundary: ram2 was re-clocked from pll_audio to clk_sys
# for blit_engine_1. The f2sdram_safe_terminator in sysmem.sv handles
# the CDC between the user clock and the HPS DDR3 hard IP internally.
# Without these false-paths, Quartus tries to time paths through the
# HPS hard block (producing ~95 ns paths on a 20 ns clock).
set_false_path -from [get_registers {sysmem:sysmem|f2sdram_safe_terminator:f2sdram_safe_terminator_ram2|*}]
set_false_path -to   [get_registers {sysmem:sysmem|f2sdram_safe_terminator:f2sdram_safe_terminator_ram2|*}]
