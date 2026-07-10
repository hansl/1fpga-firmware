# Dump the worst-case setup paths per clock to text files in
# output_files/. Run via:
#
#   just report-timing
#
# (which invokes `quartus_sta menu_core -t report_worst_paths.tcl`
# inside the Quartus docker image). Reads the existing fitter
# output — no rebuild needed.
#
# This is a diagnostic for the recurring -72 ns clk_sys / -5 ns
# clk_video setup failures in the COMPOSITOR_V2.md Phase 2c stack.
# `report_timing` returns the full data path detail (LUT levels,
# routing segments, cumulative delay) which the standard
# menu_core.sta.rpt only summarises.

project_open menu_core

create_timing_netlist -model slow

# Read the project's SDC files (menu_core.sdc + anything pulled in
# via SDC_FILE assignments). Same set the regular STA flow uses.
read_sdc

update_timing_netlist

set clk_sys   {emu|pll_inst|altera_pll_i|general[0].gpll~PLL_OUTPUT_COUNTER|divclk}
set clk_video {emu|pll_inst|altera_pll_i|general[1].gpll~PLL_OUTPUT_COUNTER|divclk}

puts "==> Worst clk_sys setup paths → output_files/worst_clk_sys.txt"
report_timing \
    -setup \
    -from_clock $clk_sys \
    -to_clock   $clk_sys \
    -npaths 5 \
    -detail full_path \
    -file output_files/worst_clk_sys.txt

puts "==> Worst clk_video setup paths → output_files/worst_clk_video.txt"
report_timing \
    -setup \
    -from_clock $clk_video \
    -to_clock   $clk_video \
    -npaths 5 \
    -detail full_path \
    -file output_files/worst_clk_video.txt

puts "==> Worst cross-clock setup paths (clk_video → clk_sys) → output_files/worst_video_to_sys.txt"
report_timing \
    -setup \
    -from_clock $clk_video \
    -to_clock   $clk_sys \
    -npaths 3 \
    -detail full_path \
    -file output_files/worst_video_to_sys.txt

delete_timing_netlist
project_close
