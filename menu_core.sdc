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

# CDC false-paths for the scanout compositor (sys_top|u_compositor), which
# replaced ASCAL. Each marks the FIRST stage of a synchroniser chain so
# Quartus doesn't try to close timing on the inter-domain leg. (The old
# compositor-v2 lived in emu as comp_rst_n_sync_0; it's gone.)
#
# FB geometry: clk_sys regs → clk_100m read domain (first sync stage).
set_false_path -to [get_registers {*comp_fb_base_s0[*]}]
set_false_path -to [get_registers {*comp_fb_stride_s0[*]}]
# Content-mask base + enable: clk_sys regs → clk_100m (first sync stage).
set_false_path -to [get_registers {*comp_mask_base_s0[*]}]
set_false_path -to [get_registers {*comp_mask_en_s[0]}]
# Reset deassertion bridges (async assert, sync deassert) — first stage.
set_false_path -to [get_registers {*comp_h_rst[0]}]
set_false_path -to [get_registers {*comp_a_rst[0]}]
# Frame-start toggle clk_hdmi → clk_100m (first sync stage).
set_false_path -to [get_registers {*frame_tgl_a0}]
# Gray-coded consumed-line count clk_hdmi → clk_100m (first sync stage).
set_false_path -to [get_registers {*cons_gray_a0[*]}]

# f2sdram ram2 boundary: ram2 was re-clocked from pll_audio to clk_sys
# for blit_engine_1. The f2sdram_safe_terminator in sysmem.sv handles
# the CDC between the user clock and the HPS DDR3 hard IP internally.
# Without these false-paths, Quartus tries to time paths through the
# HPS hard block (producing ~95 ns paths on a 20 ns clock).
set_false_path -from [get_registers {sysmem:sysmem|f2sdram_safe_terminator:f2sdram_safe_terminator_ram2|*}]
set_false_path -to   [get_registers {sysmem:sysmem|f2sdram_safe_terminator:f2sdram_safe_terminator_ram2|*}]
