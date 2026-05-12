//============================================================================
//
//  Per-scanline texture sampler (Phase 2c step 2).
//
//  On each `kick_i` pulse, fetches a 32-byte texture descriptor from
//  the table at `tex_table_addr_i + tex_id * 32`, reads the format
//  byte, and bursts a row of pixels from DDR3 into the on-chip
//  `line_buffer`.
//
//  Two paths, selected by the descriptor's format byte:
//
//    BGRA8888 (format == 0): one 64-bit beat = 2 source pixels. One
//      multi-beat burst, one BRAM write per beat (storing both pixels
//      side-by-side in a 64-bit-wide line buffer entry).
//
//    A8 (format == 1): one 64-bit beat = 8 alpha samples. The texture
//      unit reads 1-beat bursts (lets us spread 4 BRAM writes across
//      4 cycles without a FIFO), and each beat is expanded into 4
//      line-buffer entries. Each output pixel is { alpha, tint.b,
//      tint.g, tint.r } in BGRA order — i.e. the layer's `color`
//      field is pre-baked as the RGB tint and the alpha comes from
//      the texture.
//
//  Common limits:
//    - dst_w ≤ MAX_TEX_WIDTH (= 512). Wider layers are silently
//      truncated.
//    - Even `dst_x_lo` and `src_x` (line buffer alignment).
//    - For A8: src_x SHOULD be a multiple of 8 (one beat boundary);
//      we don't currently shift mid-beat alphas if it isn't.
//
//  State machine:
//
//    IDLE        Wait for kick.
//    DESC_REQ    Issue 4-beat burst at tex_table_addr + tex_id*32.
//    DESC_BEATS  Collect 4 beats, latch data_addr, pitch, format.
//    ROW_ADDR    Compute row physical address.
//    ROW_REQ     (BGRA path) Issue ceil(dst_w/2)-beat burst.
//    ROW_BEATS   (BGRA path) Receive beats, write to line buffer.
//    A8_REQ      (A8 path) Issue 1-beat burst at next 8-pixel offset.
//    A8_WAIT     (A8 path) Wait for the beat to arrive.
//    A8_WRITE    (A8 path) Spend 4 cycles writing 4 line-buffer
//                entries (2 pixels each, alphas + pre-baked tint).
//    DONE        Pulse done, back to IDLE.
//
//============================================================================

module texture_unit #(
    // Hard cap on per-scanline pixel count. 512 pixels = 256 64-bit
    // beats (BGRA), well under the 255 burstcnt limit AND the HBlank
    // budget. For A8 it's 512 alphas = 64 single-beat reads.
    parameter int MAX_TEX_WIDTH = 512
) (
    input  logic        clk,
    input  logic        rst_n,

    // Edge-triggered: 1-cycle pulse to start a fetch. Ignored if busy.
    input  logic        kick_i,
    input  logic [15:0] tex_id_i,
    input  logic [15:0] ty_i,         // texture-Y for this scanline
    input  logic [15:0] src_x_i,      // texture-X start (MUST be even;
                                      // for A8, multiple of 8)
    input  logic [11:0] dst_w_i,      // pixel count
    // Tint colour for A8 expansion. Ignored for BGRA8888.
    input  logic [31:0] tint_color_i,

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

    typedef enum logic [3:0] {
        S_IDLE,
        S_DESC_REQ,
        S_DESC_BEATS,
        S_ROW_ADDR,
        S_ROW_REQ,
        S_ROW_BEATS,
        S_A8_REQ,
        S_A8_WAIT,
        S_A8_WRITE,
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
    logic [31:0]  tint_latched;
    logic [7:0]   format_latched;        // 0 = BGRA8888, 1 = A8
    logic [1:0]   desc_beat_q;
    logic [255:0] desc_beats_q;
    logic [7:0]   row_beats_total;       // BGRA path
    logic [7:0]   row_beats_recv;        // BGRA path
    logic [9:0]   line_buf_wr_q;
    logic [31:0]  row_phys_addr;
    // A8 path
    logic [63:0]  a8_beat_q;             // currently-held 8 alpha samples
    logic [1:0]   a8_write_idx_q;        // 0..3 (4 cycles per beat)
    logic [11:0]  a8_alpha_off_q;        // alpha-byte offset within the row

    assign ddram_be_o = 8'hFF;
    assign ddram_rd_o = rd_q;
    assign busy_o     = (state_q != S_IDLE);

    // Descriptor field extraction (TextureDescriptor layout: data_addr
    // at byte 0, pitch at byte 4, width@8, height@10, format@12).
    wire [31:0] desc_data_addr = desc_beats_q[31:0];
    wire [31:0] desc_pitch     = desc_beats_q[63:32];
    wire [7:0]  desc_format    = desc_beats_q[103:96];

    // BGRA: 2 pixels per beat → ceil(dst_w/2) beats.
    wire [11:0] dst_w_sat   = (dst_w_i > MAX_TEX_WIDTH[11:0])
                                ? MAX_TEX_WIDTH[11:0] : dst_w_i;
    wire [11:0] dst_w_even  = dst_w_sat[0] ? (dst_w_sat + 12'd1) : dst_w_sat;
    wire [7:0]  beats_needed = dst_w_even[8:1]; // dst_w_even / 2

    // Expand one alpha byte into a 32-bit BGRA pixel using
    // `tint_latched` as the RGB. The alpha replaces the tint's own A
    // byte.
    function automatic logic [31:0] expand_a8(input logic [7:0] a);
        expand_a8 = {a, tint_latched[23:0]};
    endfunction

    // Pack two expanded pixels into one 64-bit BRAM word (pixel[0] in
    // low half, pixel[1] in high half — matches the BGRA path).
    function automatic logic [63:0] pack_a8_pair(
        input logic [7:0] a0,
        input logic [7:0] a1
    );
        pack_a8_pair = {expand_a8(a1), expand_a8(a0)};
    endfunction

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
            tint_latched     <= 32'd0;
            format_latched   <= 8'd0;
            desc_beat_q      <= 2'd0;
            desc_beats_q     <= 256'd0;
            row_beats_total  <= 8'd0;
            row_beats_recv   <= 8'd0;
            line_buf_wr_q    <= 10'd0;
            row_phys_addr    <= 32'd0;
            a8_beat_q        <= 64'd0;
            a8_write_idx_q   <= 2'd0;
            a8_alpha_off_q   <= 12'd0;
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
                        tint_latched     <= tint_color_i;
                        row_beats_total  <= beats_needed;
                        desc_beat_q      <= 2'd0;
                        state_q          <= S_DESC_REQ;
                    end
                end

                S_DESC_REQ: begin
                    if (~ddram_busy_i && ~rd_q) begin
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
                    format_latched <= desc_format;
                    line_buf_wr_q  <= 10'd0;
                    if (desc_format == 8'd1) begin
                        // A8: 1 byte per pixel. row = data_addr + ty*pitch + src_x.
                        row_phys_addr  <= desc_data_addr
                                        + ({16'd0, ty_latched} * desc_pitch)
                                        + {16'd0, src_x_latched};
                        a8_alpha_off_q <= 12'd0;
                        state_q        <= S_A8_REQ;
                    end else begin
                        // BGRA8888 (default): 4 bytes per pixel.
                        row_phys_addr  <= desc_data_addr
                                        + ({16'd0, ty_latched} * desc_pitch)
                                        + ({14'd0, src_x_latched, 2'd0});
                        row_beats_recv <= 8'd0;
                        state_q        <= S_ROW_REQ;
                    end
                end

                // === BGRA path ============================================
                S_ROW_REQ: begin
                    if (~ddram_busy_i && ~rd_q) begin
                        ddram_addr_o     <= row_phys_addr[31:3];
                        ddram_burstcnt_o <= row_beats_total;
                        rd_q             <= 1'b1;
                        state_q          <= S_ROW_BEATS;
                    end
                end

                S_ROW_BEATS: begin
                    if (ddram_dout_valid_i) begin
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

                // === A8 path ==============================================
                S_A8_REQ: begin
                    if (~ddram_busy_i && ~rd_q) begin
                        // Each iteration reads 1 beat (8 alpha samples).
                        // byte_addr = row_phys_addr + a8_alpha_off_q.
                        // word_addr = byte_addr >> 3. (Assumes src_x is
                        // multiple of 8 so the byte addr starts aligned.)
                        ddram_addr_o     <= (row_phys_addr + {20'd0, a8_alpha_off_q}) >> 3;
                        ddram_burstcnt_o <= 8'd1;
                        rd_q             <= 1'b1;
                        state_q          <= S_A8_WAIT;
                    end
                end

                S_A8_WAIT: begin
                    if (ddram_dout_valid_i) begin
                        a8_beat_q      <= ddram_dout_i;
                        a8_write_idx_q <= 2'd0;
                        state_q        <= S_A8_WRITE;
                    end
                end

                S_A8_WRITE: begin
                    // Spend 4 cycles writing 4 line-buffer entries (each
                    // 64-bit word holds 2 expanded pixels). a8_beat_q
                    // holds 8 alphas: bits [7:0]=alpha0, [15:8]=alpha1, …
                    line_buf_addr_o <= line_buf_wr_q;
                    line_buf_we_o   <= 1'b1;
                    unique case (a8_write_idx_q)
                        2'd0: line_buf_data_o <= pack_a8_pair(a8_beat_q[7:0],
                                                              a8_beat_q[15:8]);
                        2'd1: line_buf_data_o <= pack_a8_pair(a8_beat_q[23:16],
                                                              a8_beat_q[31:24]);
                        2'd2: line_buf_data_o <= pack_a8_pair(a8_beat_q[39:32],
                                                              a8_beat_q[47:40]);
                        2'd3: line_buf_data_o <= pack_a8_pair(a8_beat_q[55:48],
                                                              a8_beat_q[63:56]);
                    endcase
                    line_buf_wr_q  <= line_buf_wr_q + 10'd1;
                    a8_write_idx_q <= a8_write_idx_q + 2'd1;
                    if (a8_write_idx_q == 2'd3) begin
                        // Finished one beat (8 alphas / 4 entries).
                        // Advance to the next 8 alphas, or done.
                        if (a8_alpha_off_q + 12'd8 >= dst_w_clipped) begin
                            state_q <= S_DONE;
                        end else begin
                            a8_alpha_off_q <= a8_alpha_off_q + 12'd8;
                            state_q        <= S_A8_REQ;
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
