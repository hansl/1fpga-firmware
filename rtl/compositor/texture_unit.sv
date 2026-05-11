//============================================================================
//
//  Per-scanline texture sampler (Phase 2b step 2).
//
//  On each `kick_i` pulse, fetches a 32-byte texture descriptor from
//  the table at `tex_table_addr_i + tex_id * 32`, computes the texel-
//  row physical address, and bursts `dst_w/2` 64-bit beats (= dst_w
//  pixels, 2 per beat) from DDR3 directly into the on-chip
//  `line_buffer`. The painter then samples line_buffer[(x - dst_x_lo)
//  >> 1] during active scanout and picks the upper or lower 32-bit
//  pixel via `x[0]`.
//
//  Limits (step 2):
//    - BGRA8888 textures only. Format byte is ignored; A8 / scaling
//      land in Phase 2c.
//    - dst_w ≤ MAX_TEX_WIDTH (= 512 → 256 beats max, well under the
//      255 burstcnt cap) per scanline. Wider layers are silently
//      truncated.
//    - Even `dst_x_lo` and `src_x`. Odd-aligned layers would need a
//      per-beat shift register; we don't have that yet.
//
//  State machine:
//
//    IDLE        Wait for kick.
//    DESC_REQ    Issue 4-beat 64-bit burst at tex_table_addr+tex_id*32.
//    DESC_BEATS  Collect 4 beats, extract data_addr and pitch.
//    ROW_ADDR    Compute row physical address.
//    ROW_REQ     Issue ceil(dst_w/2)-beat burst.
//    ROW_BEATS   Receive beats, write 64-bit (2 pixels) per beat to
//                consecutive line buffer entries.
//    DONE        Pulse done, back to IDLE.
//
//============================================================================

module texture_unit #(
    // Hard cap on per-scanline pixel count. 512 pixels = 256 64-bit
    // beats, just under the 255 burstcnt limit AND the HBlank budget.
    // Wider textured layers are clipped to MAX_TEX_WIDTH.
    parameter int MAX_TEX_WIDTH = 512
) (
    input  logic        clk,
    input  logic        rst_n,

    // Edge-triggered: 1-cycle pulse to start a fetch. Ignored if busy.
    input  logic        kick_i,
    input  logic [15:0] tex_id_i,
    input  logic [15:0] ty_i,         // texture-Y for this scanline
    input  logic [15:0] src_x_i,      // texture-X start (MUST be even)
    input  logic [11:0] dst_w_i,      // pixel count (will be rounded up to even)

    // Texture descriptor table base (programmed by host).
    input  logic [31:0] tex_table_addr_i,

    // Line buffer write port. Each entry stores 2 pixels.
    output logic [9:0]  line_buf_addr_o,
    output logic [63:0] line_buf_data_o,
    output logic        line_buf_we_o,

    // DDRAM read master (shared bus; menu_core arbitrates).
    output logic [28:0] ddram_addr_o,
    output logic [7:0]  ddram_burstcnt_o,
    output logic [7:0]  ddram_be_o,
    output logic        ddram_rd_o,
    input  logic        ddram_busy_i,
    input  logic [63:0] ddram_dout_i,
    input  logic        ddram_dout_valid_i,

    // Status.
    output logic        busy_o,
    output logic        done_pulse_o
);

    typedef enum logic [2:0] {
        S_IDLE,
        S_DESC_REQ,
        S_DESC_BEATS,
        S_ROW_ADDR,
        S_ROW_REQ,
        S_ROW_BEATS,
        S_DONE
    } state_t;

    state_t       state_q;
    logic         rd_q;
    logic [31:0]  data_addr_q;
    logic [31:0]  pitch_q;
    logic [15:0]  tex_id_latched;
    logic [15:0]  ty_latched;
    logic [15:0]  src_x_latched;
    logic [11:0]  dst_w_clipped;
    logic [1:0]   desc_beat_q;
    logic [255:0] desc_beats_q;
    logic [7:0]   row_beats_total;
    logic [7:0]   row_beats_recv;
    logic [9:0]   line_buf_wr_q;
    logic [31:0]  row_phys_addr;

    assign ddram_be_o = 8'hFF;
    assign ddram_rd_o = rd_q;
    assign busy_o     = (state_q != S_IDLE);

    // Descriptor field extraction (TextureDescriptor layout: data_addr
    // at byte 0, pitch at byte 4).
    wire [31:0] desc_data_addr = desc_beats_q[31:0];
    wire [31:0] desc_pitch     = desc_beats_q[63:32];

    // Clip dst_w to MAX_TEX_WIDTH and round up to even.
    wire [11:0] dst_w_sat   = (dst_w_i > MAX_TEX_WIDTH[11:0])
                                ? MAX_TEX_WIDTH[11:0] : dst_w_i;
    wire [11:0] dst_w_even  = dst_w_sat[0] ? (dst_w_sat + 12'd1) : dst_w_sat;
    wire [7:0]  beats_needed = dst_w_even[8:1]; // = dst_w_even / 2

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state_q          <= S_IDLE;
            rd_q             <= 1'b0;
            data_addr_q      <= 32'd0;
            pitch_q          <= 32'd0;
            tex_id_latched   <= 16'd0;
            ty_latched       <= 16'd0;
            src_x_latched    <= 16'd0;
            dst_w_clipped    <= 12'd0;
            desc_beat_q      <= 2'd0;
            desc_beats_q     <= 256'd0;
            row_beats_total  <= 8'd0;
            row_beats_recv   <= 8'd0;
            line_buf_wr_q    <= 10'd0;
            row_phys_addr    <= 32'd0;
            line_buf_addr_o  <= 10'd0;
            line_buf_data_o  <= 64'd0;
            line_buf_we_o    <= 1'b0;
            done_pulse_o     <= 1'b0;
            ddram_addr_o     <= 29'd0;
            ddram_burstcnt_o <= 8'd0;
        end else begin
            line_buf_we_o <= 1'b0;
            done_pulse_o  <= 1'b0;
            if (~ddram_busy_i && rd_q) rd_q <= 1'b0;

            unique case (state_q)
                S_IDLE: begin
                    if (kick_i && dst_w_i != 12'd0) begin
                        tex_id_latched   <= tex_id_i;
                        ty_latched       <= ty_i;
                        src_x_latched    <= src_x_i;
                        dst_w_clipped    <= dst_w_sat;
                        row_beats_total  <= beats_needed;
                        desc_beat_q      <= 2'd0;
                        state_q          <= S_DESC_REQ;
                    end
                end

                S_DESC_REQ: begin
                    if (~ddram_busy_i && ~rd_q) begin
                        // tex_table byte_addr = tex_table_addr + tex_id*32
                        // → 64-bit-word addr = (tex_table_addr>>3) + tex_id*4
                        ddram_addr_o     <= (tex_table_addr_i[31:3])
                                          + {13'd0, tex_id_latched, 2'd0};
                        ddram_burstcnt_o <= 8'd4;
                        rd_q             <= 1'b1;
                        state_q          <= S_DESC_BEATS;
                    end
                end

                S_DESC_BEATS: begin
                    if (ddram_dout_valid_i) begin
                        unique case (desc_beat_q)
                            2'd0: desc_beats_q[63:0]    <= ddram_dout_i;
                            2'd1: desc_beats_q[127:64]  <= ddram_dout_i;
                            2'd2: desc_beats_q[191:128] <= ddram_dout_i;
                            2'd3: desc_beats_q[255:192] <= ddram_dout_i;
                        endcase
                        desc_beat_q <= desc_beat_q + 2'd1;
                        if (desc_beat_q == 2'd3) state_q <= S_ROW_ADDR;
                    end
                end

                S_ROW_ADDR: begin
                    data_addr_q    <= desc_data_addr;
                    pitch_q        <= desc_pitch;
                    // row = data_addr + ty * pitch + src_x * 4
                    // (ty * pitch may exceed 32 bits for pathological
                    // sizes; for the menu UI's <= 2048×2048 textures
                    // 32-bit is plenty.)
                    row_phys_addr  <= desc_data_addr
                                    + ({16'd0, ty_latched} * desc_pitch)
                                    + ({14'd0, src_x_latched, 2'd0});
                    row_beats_recv <= 8'd0;
                    // Always start writing at line_buf[0]; the painter
                    // computes its read index relative to dst_x_lo, so
                    // line_buf[0] is "the first beat" regardless of
                    // where on screen the textured layer sits.
                    line_buf_wr_q  <= 10'd0;
                    state_q        <= S_ROW_REQ;
                end

                S_ROW_REQ: begin
                    if (~ddram_busy_i && ~rd_q) begin
                        // Byte-addr >> 3 = 64-bit word addr. Even src_x
                        // assumed, so [2:0] of (data_addr + ty*pitch +
                        // src_x*4) are zero relative to start of texture
                        // row. data_addr is texture-pool aligned (host
                        // BumpAllocator uses 64-byte alignment), so the
                        // bottom 3 bits are 0.
                        ddram_addr_o     <= row_phys_addr[31:3];
                        ddram_burstcnt_o <= row_beats_total;
                        rd_q             <= 1'b1;
                        state_q          <= S_ROW_BEATS;
                    end
                end

                S_ROW_BEATS: begin
                    if (ddram_dout_valid_i) begin
                        // One DDR3 beat = 64 bits = 2 BGRA8888 pixels.
                        // Bits [31:0] = pixel (src_x + 2k), [63:32] =
                        // pixel (src_x + 2k+1). Store both in one
                        // 64-bit-wide BRAM cell — painter does the
                        // half-select.
                        line_buf_addr_o <= line_buf_wr_q;
                        line_buf_data_o <= ddram_dout_i;
                        line_buf_we_o   <= 1'b1;
                        line_buf_wr_q   <= line_buf_wr_q + 10'd1;
                        row_beats_recv  <= row_beats_recv + 8'd1;
                        if (row_beats_recv + 8'd1 == row_beats_total) begin
                            state_q <= S_DONE;
                        end
                    end
                end

                S_DONE: begin
                    done_pulse_o <= 1'b1;
                    state_q      <= S_IDLE;
                end

                default: state_q <= S_IDLE;
            endcase
        end
    end

endmodule
