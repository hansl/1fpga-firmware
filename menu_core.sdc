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
set_clock_groups -asynchronous \
    -group [get_clocks {emu|pll_inst|altera_pll_i|*|divclk}] \
    -group [get_clocks {pll_hdmi|*|divclk}] \
    -group [get_clocks {pll_audio|*|divclk}] \
    -group [get_clocks {*|h2f_user0_clk}]
