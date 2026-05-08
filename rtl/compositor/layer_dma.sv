//============================================================================
//
//  Layer-table DMA fetcher (Phase 2a step 2).
//
//  On each `start_i` pulse, reads `count_i` × 32-byte layer descriptors
//  starting at byte address `base_i` and writes each into `layer_cache`
//  at the slot index it came from. One 4-beat 64-bit burst per
//  descriptor; assembled little-endian in the receive buffer so the
//  layer-cache word matches the host-side `LayerDescriptor` layout
//  byte-for-byte.
//
//  Address translation: DDRAM_ADDR is 29-bit and addresses 64-bit
//  (8-byte) beats:
//
//    DDRAM_ADDR = (base_i + slot * 32) >> 3
//               = base_i[31:3] + slot * 4
//
//  busy_o stays high while the DMA is filling. The renderer must hold
//  off reading from the cache until busy_o falls (i.e. until the
//  done_pulse_o cycle has been observed). In normal operation this is
//  trivially satisfied — the DMA fires once per frame on VBlank entry
//  and finishes well before the next active line.
//
//  Observability: `descriptors_o` is a 32-bit free-running counter that
//  increments once per descriptor written into the cache. Surfaces
//  through the LAYER_DEBUG register so the host can sanity-check that
//  the DMA is actually ticking at frame rate.
//
//============================================================================

module layer_dma (
    input  logic        clk,
    input  logic        rst_n,

    // Edge-triggered: 1-cycle pulse to start a fetch pass. Ignored if
    // the DMA is already busy.
    input  logic        start_i,
    input  logic [31:0] base_i,        // host physical byte address
    input  logic [8:0]  count_i,       // 0..256 descriptors to fetch

    // Layer cache write port.
    output logic [7:0]   cache_slot_o,
    output logic [255:0] cache_data_o,
    output logic         cache_we_o,

    // DDRAM read master (one of three; menu_core arbitrates).
    output logic [28:0] ddram_addr_o,
    output logic [7:0]  ddram_burstcnt_o,
    output logic [7:0]  ddram_be_o,
    output logic        ddram_rd_o,
    input  logic        ddram_busy_i,
    input  logic [63:0] ddram_dout_i,
    input  logic        ddram_dout_valid_i,

    // Status.
    output logic        busy_o,
    output logic        done_pulse_o,
    output logic [31:0] descriptors_o
);

    typedef enum logic [1:0] {
        S_IDLE,
        S_REQ,
        S_BEATS,
        S_COMMIT
    } state_t;

    state_t       state_q;
    logic [8:0]   slot_q;
    logic [8:0]   count_q;
    logic [28:0]  base_word_q;     // base_i / 8
    logic [1:0]   beat_q;
    logic [255:0] beats_q;
    logic         rd_q;
    logic [31:0]  desc_count_q;

    assign ddram_be_o    = 8'hFF;   // full 64-bit beat
    assign ddram_rd_o    = rd_q;
    assign busy_o        = (state_q != S_IDLE);
    assign descriptors_o = desc_count_q;

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state_q          <= S_IDLE;
            slot_q           <= 9'd0;
            count_q          <= 9'd0;
            base_word_q      <= 29'd0;
            beat_q           <= 2'd0;
            beats_q          <= 256'd0;
            rd_q             <= 1'b0;
            cache_we_o       <= 1'b0;
            cache_slot_o     <= 8'd0;
            cache_data_o     <= 256'd0;
            done_pulse_o     <= 1'b0;
            ddram_addr_o     <= 29'd0;
            ddram_burstcnt_o <= 8'd0;
            desc_count_q     <= 32'd0;
        end else begin
            // Defaults for one-cycle pulses.
            cache_we_o   <= 1'b0;
            done_pulse_o <= 1'b0;
            // Avalon-MM: drop RD once the slave latched the request
            // (busy went low while RD was high).
            if (~ddram_busy_i && rd_q) rd_q <= 1'b0;

            unique case (state_q)
                S_IDLE: begin
                    if (start_i && count_i != 9'd0) begin
                        slot_q      <= 9'd0;
                        count_q     <= count_i;
                        base_word_q <= base_i[31:3];
                        state_q     <= S_REQ;
                    end
                end

                S_REQ: begin
                    if (slot_q >= count_q) begin
                        done_pulse_o <= 1'b1;
                        state_q      <= S_IDLE;
                    end else if (~ddram_busy_i && ~rd_q) begin
                        // 64-bit-word address of the descriptor:
                        //   base_word + slot * 4 (4 beats per slot).
                        ddram_addr_o     <= base_word_q + {18'd0, slot_q, 2'd0};
                        ddram_burstcnt_o <= 8'd4;
                        rd_q             <= 1'b1;
                        beat_q           <= 2'd0;
                        state_q          <= S_BEATS;
                    end
                end

                S_BEATS: begin
                    if (ddram_dout_valid_i) begin
                        // Assemble little-endian: beat 0 → bits [63:0],
                        // beat 1 → [127:64], etc. — matches the
                        // host-side struct's first-byte-at-LSB layout.
                        unique case (beat_q)
                            2'd0: beats_q[63:0]    <= ddram_dout_i;
                            2'd1: beats_q[127:64]  <= ddram_dout_i;
                            2'd2: beats_q[191:128] <= ddram_dout_i;
                            2'd3: beats_q[255:192] <= ddram_dout_i;
                        endcase
                        beat_q <= beat_q + 2'd1;
                        if (beat_q == 2'd3) state_q <= S_COMMIT;
                    end
                end

                S_COMMIT: begin
                    // beats_q is fully landed by now (the [255:192]
                    // assignment from the 4th beat completed last
                    // cycle).
                    cache_slot_o <= slot_q[7:0];
                    cache_data_o <= beats_q;
                    cache_we_o   <= 1'b1;
                    desc_count_q <= desc_count_q + 32'd1;
                    slot_q       <= slot_q + 9'd1;
                    state_q      <= S_REQ;
                end

                default: state_q <= S_IDLE;
            endcase
        end
    end

endmodule
