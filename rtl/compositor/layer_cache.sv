//============================================================================
//
//  Layer-descriptor cache (Phase 2a, dual-clock).
//
//  256-entry × 32-byte BRAM holding the active layer table for the
//  current frame. Write port is filled by `layer_dma` on `wr_clk`
//  (= clk_sys, 50 MHz); read port is consumed by the compositor's
//  scanline walker on `rd_clk` (= clk_video, 100 MHz for native
//  1080p scanout).
//
//  Layout: 256 × 256 bits. Quartus infers ~8 M10K blocks (true
//  dual-port mode with independent clocks) — Cyclone V SE-A6 has
//  hundreds, no pressure.
//
//  Read-during-write coherence: with independent clocks, the read
//  port returns "old" data for a slot concurrently being written.
//  Cross-clock metastability of the data itself isn't a concern —
//  the BRAM cells are stable storage and the read port latches a
//  full 256-bit word per `rd_clk` cycle. We accept one frame of
//  stale data on slots the DMA is updating during the same scanout.
//
//  Bit-mapping mirrors the host-side `LayerDescriptor` (PROTOCOL.md
//  §11.1), starting at byte 0 in the LSB:
//
//    [ 15:  0]  flags          (u16)
//    [ 31: 16]  tex_id         (u16; LAYER_TEX_SOLID = 0xFFFF)
//    [ 47: 32]  dst_x          (i16)
//    [ 63: 48]  dst_y          (i16)
//    [ 79: 64]  dst_w          (u16)
//    [ 95: 80]  dst_h          (u16)
//    [111: 96]  src_x          (u16)
//    [127:112]  src_y          (u16)
//    [143:128]  src_w          (u16)
//    [159:144]  src_h          (u16)
//    [191:160]  color          (BGRA u32)
//    [199:192]  opacity        (u8)
//    [255:200]  reserved (host writes 0)
//
//============================================================================

module layer_cache (
    // Write port — driven by layer_dma on clk_sys.
    input  logic        wr_clk,
    input  logic [7:0]   wr_slot_i,
    input  logic [255:0] wr_data_i,
    input  logic         wr_en_i,

    // Read port — driven by the compositor on clk_video. One-cycle
    // latency: sample `rd_slot_i` on cycle N, `rd_data_o` is valid
    // on cycle N+1.
    input  logic        rd_clk,
    input  logic [7:0]   rd_slot_i,
    output logic [255:0] rd_data_o
);

    logic [255:0] mem [0:255];

    always_ff @(posedge wr_clk) begin
        if (wr_en_i) mem[wr_slot_i] <= wr_data_i;
    end

    always_ff @(posedge rd_clk) begin
        rd_data_o <= mem[rd_slot_i];
    end

endmodule
