//============================================================================
//
//  Per-scanline texel line buffer (Phase 2b step 2).
//
//  Dual-clock BRAM that texture_unit fills (on clk_sys, during HBlank
//  after the scanline_filter completes) and the painter reads (on
//  clk_video, during the next active scanout).
//
//  Layout: 1024 entries × 64 bits = 64 Kbit. Each entry holds TWO
//  BGRA8888 pixels (one DDR3 beat) — bits [31:0] = even pixel,
//  bits [63:32] = odd pixel. Quartus infers ~8 M10K blocks (true
//  dual-port mode, independent clocks).
//
//  This makes the write side trivial: every DDR3 beat is one BRAM
//  write. The painter does the half-selection on the read side via
//  `x[0]` to pick which 32-bit pixel out of the 64-bit word.
//
//  Constraint (step 2): textured layers must have even `dst_x` and
//  even `src_x`. Odd-aligned layers would require either a per-pixel
//  BE shuffle or a separate shift-register pipeline, both of which
//  complicate the state machine without unlocking new use cases for
//  the menu UI (which positions everything on even-pixel boundaries
//  by convention).
//
//============================================================================

module line_buffer (
    // Write port — driven by texture_unit on clk_sys. Address indexes
    // 64-bit words; each write stores 2 pixels.
    input  logic        wr_clk,
    input  logic [9:0]  wr_addr_i,
    input  logic [63:0] wr_data_i,
    input  logic        wr_en_i,

    // Read port — driven by the compositor's painter on clk_video.
    // One-cycle latency.
    input  logic        rd_clk,
    input  logic [9:0]  rd_addr_i,
    output logic [63:0] rd_data_o
);

    logic [63:0] mem [0:1023];

    always_ff @(posedge wr_clk) begin
        if (wr_en_i) mem[wr_addr_i] <= wr_data_i;
    end

    always_ff @(posedge rd_clk) begin
        rd_data_o <= mem[rd_addr_i];
    end

endmodule
