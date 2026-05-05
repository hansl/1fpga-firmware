//============================================================================
//
//  Blit engine (FILL_RECT + COPY_RECT 1:1 + A8 / tint).
//
//  Modes selected by `mode_i`:
//    MODE_FILL (0): write `color_i` to every pixel of the dst rect.
//    MODE_COPY (1): for every pixel, read the source from a texture in
//      DDR3 and write to the framebuffer.
//
//  In COPY mode:
//    - `format_i` selects RGBA8888 (4 bytes/pixel) or A8 (1 byte/pixel).
//    - `tint_en_i` enables per-channel multiplication. For A8 sources
//      tint is implicit (the texture has no RGB) — we always supply
//      `tint_color_i.RGB` and modulate by the sampled alpha.
//    - Blend is Opaque only at this milestone (write-only path).
//      SrcAlpha (M2c3.3) will read the dst pixel and mix.
//
//  A8 + tint output uses premultiplied alpha:
//    out.RGB = tint.RGB * sampled_alpha / 256
//    out.A   = sampled_alpha
//  The framework's scanout ignores A; premultiplying RGB by alpha gives
//  a visually meaningful gradient between black (alpha=0) and the tint
//  colour (alpha=255) under Opaque blend.
//
//  Still one pixel per 64-bit beat (wasteful but simple); bursting +
//  2-pixels-per-beat is M2c5.
//
//============================================================================

module blit_engine (
    input  logic        clk,
    input  logic        rst_n,

    // Command interface from ring_fetcher.
    input  logic        start_i,
    input  logic        mode_i,           // 0 = FILL, 1 = COPY
    input  logic [1:0]  blend_i,          // 0 = Opaque, 1 = SrcAlpha, 2 = Additive
    input  logic [15:0] dst_x_i,
    input  logic [15:0] dst_y_i,
    input  logic [15:0] dst_w_i,
    input  logic [15:0] dst_h_i,
    input  logic [31:0] color_i,          // FILL: constant; COPY: ignored

    // COPY-only inputs.
    input  logic [15:0] src_x_i,
    input  logic [15:0] src_y_i,
    input  logic [31:0] src_addr_i,
    input  logic [31:0] src_pitch_i,
    input  logic        format_i,         // 0 = RGBA8888, 1 = A8
    input  logic        tint_en_i,
    input  logic [31:0] tint_color_i,

    // Framebuffer geometry.
    input  logic [31:0] fb_base_i,
    input  logic [13:0] fb_stride_i,

    output logic        busy_o,
    output logic        done_o,

    // DDRAM master interface.
    output logic [28:0] ddram_addr_o,
    output logic [7:0]  ddram_burstcnt_o,
    output logic [7:0]  ddram_be_o,
    output logic [63:0] ddram_din_o,
    output logic        ddram_we_o,
    output logic        ddram_rd_o,
    input  logic        ddram_busy_i,
    input  logic [63:0] ddram_dout_i,
    input  logic        ddram_dout_valid_i
);

    localparam logic MODE_FILL = 1'b0;
    localparam logic MODE_COPY = 1'b1;
    localparam logic FMT_RGBA  = 1'b0;
    localparam logic FMT_A8    = 1'b1;

    localparam logic [1:0] BLEND_OPAQUE   = 2'd0;
    localparam logic [1:0] BLEND_SRCALPHA = 2'd1;
    localparam logic [1:0] BLEND_ADDITIVE = 2'd2;

    typedef enum logic [3:0] {
        S_IDLE,
        S_ROW_INIT,
        S_NEXT_PIXEL,
        S_FETCH_SRC,
        S_WAIT_SRC,
        S_FETCH_DST,
        S_WAIT_DST,
        S_BLEND,           // pipeline stage so blend math meets timing
        S_WRITE,
        S_WRITE_WAIT,
        S_FILL_BURST,      // Avalon-MM burst, 2 pixels per beat (FILL Opaque)
        S_DONE
    } state_e;

    state_e      state;
    logic        mode_q;
    logic [1:0]  blend_q;
    logic        format_q;
    logic        tint_en_q;
    logic [31:0] tint_color_q;
    logic [15:0] dst_x_q, dst_y_q, dst_w_q, dst_h_q;
    logic [31:0] color_q;
    logic [15:0] src_x_q, src_y_q;
    logic [31:0] src_addr_q;
    logic [31:0] src_pitch_q;

    logic [15:0] cur_x, cur_y_off;
    logic [31:0] dst_row_byte_addr;
    logic [31:0] src_row_byte_addr;
    logic [31:0] pixel_data;
    logic [31:0] src_pixel_q;        // computed source pixel held while we fetch dst
    logic [31:0] dst_pixel_q;        // captured dst pixel held while blend computes
    logic [7:0]  burst_len_q;        // beats in current burst (1..255)
    logic [7:0]  burst_done_q;       // beats accepted in current burst

    assign busy_o = (state != S_IDLE) & (state != S_DONE);

    // Per-pixel byte addresses.
    // RGBA8888: x*4. A8: x*1.
    wire [31:0] cur_x_offset_dst = ({16'd0, cur_x} <<< 2);
    wire [31:0] cur_x_offset_src = (format_q == FMT_A8)
                                       ? {16'd0, cur_x}
                                       : ({16'd0, cur_x} <<< 2);
    wire [31:0] dst_pixel_byte_addr = dst_row_byte_addr + cur_x_offset_dst;
    wire [31:0] src_pixel_byte_addr = src_row_byte_addr + cur_x_offset_src;

    // ---- Helpers ----------------------------------------------------
    function automatic logic [7:0] be_for_word(input logic upper);
        return upper ? 8'b1111_0000 : 8'b0000_1111;
    endfunction

    function automatic logic [7:0] be_for_byte(input logic [2:0] off);
        return 8'b0000_0001 << off;
    endfunction

    function automatic logic [31:0] pick_word(input logic [63:0] beat,
                                              input logic        upper);
        return upper ? beat[63:32] : beat[31:0];
    endfunction

    function automatic logic [7:0] pick_byte(input logic [63:0] beat,
                                             input logic [2:0]  off);
        return beat[off*8 +: 8];
    endfunction

    // (a * b + 0x80) >> 8 — close to round-to-nearest 8-bit
    // multiplication (per-channel). 0xFF*0xFF + 0x80 = 65153 fits
    // comfortably in 16 bits. DSP-friendly; no division.
    function automatic logic [7:0] mul8(input logic [7:0] a, input logic [7:0] b);
        logic [15:0] product;
        product = ({8'd0, a} * {8'd0, b}) + 16'h0080;
        return product[15:8];
    endfunction

    // Saturating 8-bit add (used by Additive blend).
    function automatic logic [7:0] sat_add8(input logic [7:0] a, input logic [7:0] b);
        logic [8:0] sum;
        sum = {1'b0, a} + {1'b0, b};
        return sum[8] ? 8'hFF : sum[7:0];
    endfunction

    // Compose a 32-bit BGRA-in-memory word from per-channel components.
    // Memory order is B, G, R, A (low to high byte).
    function automatic logic [31:0] pack_pixel(
        input logic [7:0] r,
        input logic [7:0] g,
        input logic [7:0] b,
        input logic [7:0] a
    );
        return {a, r, g, b};
    endfunction

    // Channel extraction from a 32-bit BGRA-in-memory word.
    function automatic logic [7:0] ch_b(input logic [31:0] p); return p[7:0];   endfunction
    function automatic logic [7:0] ch_g(input logic [31:0] p); return p[15:8];  endfunction
    function automatic logic [7:0] ch_r(input logic [31:0] p); return p[23:16]; endfunction
    function automatic logic [7:0] ch_a(input logic [31:0] p); return p[31:24]; endfunction

    // ---- Output multiplexing ---------------------------------------
    always_comb begin
        ddram_addr_o     = 29'd0;
        ddram_burstcnt_o = 8'd0;
        ddram_be_o       = 8'd0;
        ddram_din_o      = 64'd0;
        ddram_we_o       = 1'b0;
        ddram_rd_o       = 1'b0;

        unique case (state)
            S_FETCH_SRC: begin
                ddram_addr_o     = src_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = (format_q == FMT_A8)
                                       ? be_for_byte(src_pixel_byte_addr[2:0])
                                       : be_for_word(src_pixel_byte_addr[2]);
                ddram_rd_o       = 1'b1;
            end
            S_FETCH_DST: begin
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = be_for_word(dst_pixel_byte_addr[2]);
                ddram_rd_o       = 1'b1;
            end
            S_WRITE: begin
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = be_for_word(dst_pixel_byte_addr[2]);
                ddram_din_o      = dst_pixel_byte_addr[2]
                                       ? {pixel_data, 32'd0}
                                       : {32'd0, pixel_data};
                ddram_we_o       = 1'b1;
            end
            S_FILL_BURST: begin
                // 2 pixels per beat, full byteenable, color replicated.
                // Avalon-MM burst: address + burstcnt are looked at on
                // the first beat by the slave; we hold them stable for
                // the whole burst (slaves are tolerant). Master keeps
                // we=1 and din stable (constant colour for FILL); slave
                // accepts one beat per cycle of !waitrequest.
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = burst_len_q;
                ddram_be_o       = 8'hFF;
                ddram_din_o      = {color_q, color_q};
                ddram_we_o       = 1'b1;
            end
            default: ;
        endcase
    end

    // Combinational blend: produces the final pixel given the captured
    // src pixel (in src_pixel_q) and the just-fetched dst pixel.
    function automatic logic [31:0] blend_pixel(
        input logic [31:0] src,
        input logic [31:0] dst,
        input logic [1:0]  blend
    );
        logic [7:0] inv_a;
        logic [7:0] r, g, b, a;
        unique case (blend)
            BLEND_SRCALPHA: begin
                inv_a = 8'hFF - ch_a(src);
                r = ch_r(src) + mul8(ch_r(dst), inv_a);
                g = ch_g(src) + mul8(ch_g(dst), inv_a);
                b = ch_b(src) + mul8(ch_b(dst), inv_a);
                a = ch_a(src) + mul8(ch_a(dst), inv_a);
                blend_pixel = pack_pixel(r, g, b, a);
            end
            BLEND_ADDITIVE: begin
                blend_pixel = pack_pixel(
                    sat_add8(ch_r(src), ch_r(dst)),
                    sat_add8(ch_g(src), ch_g(dst)),
                    sat_add8(ch_b(src), ch_b(dst)),
                    sat_add8(ch_a(src), ch_a(dst))
                );
            end
            default: blend_pixel = src;       // Opaque (shouldn't reach here)
        endcase
    endfunction

    // ---- FSM transitions -------------------------------------------
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state             <= S_IDLE;
            mode_q            <= MODE_FILL;
            blend_q           <= BLEND_OPAQUE;
            format_q          <= FMT_RGBA;
            tint_en_q         <= 1'b0;
            tint_color_q      <= '0;
            dst_x_q           <= '0;
            dst_y_q           <= '0;
            dst_w_q           <= '0;
            dst_h_q           <= '0;
            color_q           <= '0;
            src_x_q           <= '0;
            src_y_q           <= '0;
            src_addr_q        <= '0;
            src_pitch_q       <= '0;
            cur_x             <= '0;
            cur_y_off         <= '0;
            dst_row_byte_addr <= '0;
            src_row_byte_addr <= '0;
            pixel_data        <= '0;
            src_pixel_q       <= '0;
            dst_pixel_q       <= '0;
            burst_len_q       <= '0;
            burst_done_q      <= '0;
            done_o            <= 1'b0;
        end else begin
            done_o <= 1'b0;

            unique case (state)
                S_IDLE: if (start_i) begin
                    mode_q       <= mode_i;
                    blend_q      <= blend_i;
                    format_q     <= format_i;
                    tint_en_q    <= tint_en_i;
                    tint_color_q <= tint_color_i;
                    dst_x_q      <= dst_x_i;
                    dst_y_q      <= dst_y_i;
                    dst_w_q      <= dst_w_i;
                    dst_h_q      <= dst_h_i;
                    color_q      <= color_i;
                    src_x_q      <= src_x_i;
                    src_y_q      <= src_y_i;
                    src_addr_q   <= src_addr_i;
                    src_pitch_q  <= src_pitch_i;
                    cur_y_off    <= '0;
                    state        <= S_ROW_INIT;
                end

                S_ROW_INIT: begin
                    if (cur_y_off == dst_h_q) begin
                        state <= S_DONE;
                    end else begin
                        dst_row_byte_addr <= fb_base_i
                            + ({16'd0, (dst_y_q + cur_y_off)} * {18'd0, fb_stride_i})
                            + ({16'd0, dst_x_q} <<< 2);
                        // src x byte multiplier depends on format.
                        src_row_byte_addr <= src_addr_q
                            + ({16'd0, (src_y_q + cur_y_off)} * src_pitch_q)
                            + ((format_q == FMT_A8)
                                  ? {16'd0, src_x_q}
                                  : ({16'd0, src_x_q} <<< 2));
                        cur_x <= '0;
                        state <= S_NEXT_PIXEL;
                    end
                end

                S_NEXT_PIXEL: begin
                    if (cur_x == dst_w_q) begin
                        cur_y_off <= cur_y_off + 16'd1;
                        state     <= S_ROW_INIT;
                    end else if (mode_q == MODE_COPY) begin
                        state <= S_FETCH_SRC;
                    end else if (blend_q == BLEND_OPAQUE) begin
                        // FILL Opaque — burst write 2 pixels per beat
                        // when start + length are 2-pixel-aligned. For
                        // odd start or odd remaining length, fall back
                        // to per-pixel writes.
                        automatic logic [15:0] remaining = dst_w_q - cur_x;
                        // Beat-aligned start iff (dst_x + cur_x) is
                        // even. Both bits xor together gives parity.
                        automatic logic aligned_start =
                            ~(dst_x_q[0] ^ cur_x[0]);
                        automatic logic aligned_len = (remaining[0] == 1'b0);
                        if (aligned_start && aligned_len && remaining > 16'd1) begin
                            automatic logic [15:0] beats = remaining >> 1;
                            burst_len_q  <= (beats > 16'd255)
                                                ? 8'd255
                                                : beats[7:0];
                            burst_done_q <= 8'd0;
                            state        <= S_FILL_BURST;
                        end else begin
                            pixel_data <= color_q;
                            state      <= S_WRITE;
                        end
                    end else begin
                        // FILL non-Opaque needs RMW.
                        src_pixel_q <= color_q;
                        state       <= S_FETCH_DST;
                    end
                end

                S_FETCH_SRC: if (~ddram_busy_i) begin
                    state <= S_WAIT_SRC;
                end

                S_WAIT_SRC: if (ddram_dout_valid_i) begin
                    automatic logic [31:0] src_word;
                    automatic logic [7:0]  sampled_alpha;
                    automatic logic [31:0] computed_src;
                    if (format_q == FMT_A8) begin
                        sampled_alpha = pick_byte(ddram_dout_i, src_pixel_byte_addr[2:0]);
                        computed_src = pack_pixel(
                            mul8(ch_r(tint_color_q), sampled_alpha),
                            mul8(ch_g(tint_color_q), sampled_alpha),
                            mul8(ch_b(tint_color_q), sampled_alpha),
                            sampled_alpha
                        );
                    end else begin
                        src_word = pick_word(ddram_dout_i, src_pixel_byte_addr[2]);
                        if (tint_en_q) begin
                            computed_src = pack_pixel(
                                mul8(ch_r(src_word), ch_r(tint_color_q)),
                                mul8(ch_g(src_word), ch_g(tint_color_q)),
                                mul8(ch_b(src_word), ch_b(tint_color_q)),
                                mul8(ch_a(src_word), ch_a(tint_color_q))
                            );
                        end else begin
                            computed_src = src_word;
                        end
                    end

                    if (blend_q == BLEND_OPAQUE) begin
                        pixel_data <= computed_src;
                        state      <= S_WRITE;
                    end else begin
                        src_pixel_q <= computed_src;
                        state       <= S_FETCH_DST;
                    end
                end

                S_FETCH_DST: if (~ddram_busy_i) begin
                    state <= S_WAIT_DST;
                end

                S_WAIT_DST: if (ddram_dout_valid_i) begin
                    // Capture only — let the blend math run in the
                    // next cycle so the combinational path doesn't
                    // exceed the clock period.
                    dst_pixel_q <= pick_word(ddram_dout_i, dst_pixel_byte_addr[2]);
                    state       <= S_BLEND;
                end

                S_BLEND: begin
                    pixel_data <= blend_pixel(src_pixel_q, dst_pixel_q, blend_q);
                    state      <= S_WRITE;
                end

                S_WRITE: if (~ddram_busy_i) begin
                    state <= S_WRITE_WAIT;
                end

                S_WRITE_WAIT: begin
                    cur_x <= cur_x + 16'd1;
                    state <= S_NEXT_PIXEL;
                end

                S_FILL_BURST: if (~ddram_busy_i) begin
                    // One beat accepted this cycle.
                    burst_done_q <= burst_done_q + 8'd1;
                    if (burst_done_q + 8'd1 == burst_len_q) begin
                        // Last beat of this burst. Advance cur_x by
                        // 2 × beats. S_NEXT_PIXEL will re-evaluate and
                        // either issue another burst or move to the
                        // next row. The cycle through S_NEXT_PIXEL
                        // (we=0) gives the slave a clean burst boundary.
                        cur_x <= cur_x + ({8'd0, burst_len_q} <<< 1);
                        state <= S_NEXT_PIXEL;
                    end
                end

                S_DONE: begin
                    done_o <= 1'b1;
                    state  <= S_IDLE;
                end
            endcase
        end
    end

endmodule
