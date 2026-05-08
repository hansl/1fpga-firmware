//============================================================================
//
//  Layer-descriptor cache (Phase 2a step 2).
//
//  256-entry × 32-byte BRAM holding the active layer table for the
//  current frame. Write port is filled by `layer_dma` once per frame
//  during VBlank; read port is consumed by the compositor's scanline
//  walker (Phase 2a step 3).
//
//  Layout: 256 × 256 bits. Quartus infers ~8 M10K blocks per port
//  (Cyclone V SE-A6 has hundreds, no pressure).
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
//  The renderer extracts fields by bit-slice rather than re-decoding
//  bytes, so DDR3 endianness is "first byte in LSB" for the whole
//  256-bit word. The DMA writer assembles beats LSB-first to match.
//
//============================================================================

module layer_cache (
    input  logic        clk,

    // Write port — driven by layer_dma.
    input  logic [7:0]   wr_slot_i,
    input  logic [255:0] wr_data_i,
    input  logic         wr_en_i,

    // Read port — driven by the renderer in step 3. One-cycle latency:
    // sample `rd_slot_i` on cycle N, `rd_data_o` is valid on cycle N+1.
    input  logic [7:0]   rd_slot_i,
    output logic [255:0] rd_data_o
);

    logic [255:0] mem [0:255];

    // Synchronous read with read-during-write "old" semantics
    // (Quartus's default for inferred BRAM). The renderer never reads
    // from a slot that's currently being written (DMA only runs during
    // VBlank, well before the renderer starts its first active line).
    always_ff @(posedge clk) begin
        if (wr_en_i) mem[wr_slot_i] <= wr_data_i;
        rd_data_o <= mem[rd_slot_i];
    end

endmodule
