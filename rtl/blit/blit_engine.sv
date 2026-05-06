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

    // Clipping (PROTOCOL.md §5.5). The blit engine intersects the
    // requested dst rect with `effective_clip`:
    //   effective_clip = ignore_clip_i ? target_bounds
    //                                  : user_clip ∩ target_bounds
    // and adjusts src coordinates by the same offset (for COPY 1:1).
    //
    // The "target" is the active render destination — the framebuffer
    // by default, or a texture's pixel data after SET_RENDER_TARGET
    // (PROTOCOL.md §5.6). The ring fetcher tracks this state and
    // drives the target_* inputs accordingly.
    input  logic [15:0] target_width_i,
    input  logic [15:0] target_height_i,
    input  logic        clip_en_i,
    input  logic [15:0] clip_x_i,
    input  logic [15:0] clip_y_i,
    input  logic [15:0] clip_w_i,
    input  logic [15:0] clip_h_i,
    input  logic        ignore_clip_i,

    // Active render target geometry (FB by default, texture after RTT).
    input  logic [31:0] target_base_i,
    input  logic [31:0] target_pitch_i,

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

    // The pair-COPY path doubles throughput on RGBA→RGBA blits whenever
    // both src and dst rects start on an 8-byte boundary (which is the
    // common case: text RTs pitch is width*4 with width even). Each
    // iteration reads/writes a 64-bit beat = 2 RGBA pixels in one DDRAM
    // round-trip instead of two. Misaligned starts and odd remainders
    // fall back to the per-pixel path.
    typedef enum logic [4:0] {
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
        S_FETCH_SRC_PAIR,  // 64-bit src read (RGBA pair)
        S_WAIT_SRC_PAIR,
        S_FETCH_DST_PAIR,  // 64-bit dst read for RMW
        S_WAIT_DST_PAIR,
        S_BLEND_PAIR,
        S_WRITE_PAIR,      // 64-bit dst write (full byteenable)
        S_WRITE_WAIT_PAIR,
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
    // 64-bit pair holding registers. src_pair_q stores the 2 src pixels
    // already tinted; dst_pair_q the 2 captured dst pixels; pair_data_q
    // the final 2 pixels to write.
    logic [63:0] src_pair_q;
    logic [63:0] dst_pair_q;
    logic [63:0] pair_data_q;

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

    function automatic logic [15:0] u16_max(
        input logic [15:0] a,
        input logic [15:0] b
    );
        return (a > b) ? a : b;
    endfunction

    function automatic logic [15:0] u16_min(
        input logic [15:0] a,
        input logic [15:0] b
    );
        return (a < b) ? a : b;
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
            S_FETCH_SRC_PAIR: begin
                // 64-bit aligned read: full byteenable, 2 RGBA pixels per beat.
                ddram_addr_o     = src_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = 8'hFF;
                ddram_rd_o       = 1'b1;
            end
            S_FETCH_DST_PAIR: begin
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = 8'hFF;
                ddram_rd_o       = 1'b1;
            end
            S_WRITE_PAIR: begin
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = 8'hFF;
                ddram_din_o      = pair_data_q;
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
            src_pair_q        <= '0;
            dst_pair_q        <= '0;
            pair_data_q       <= '0;
            done_o            <= 1'b0;
        end else begin
            done_o <= 1'b0;

            unique case (state)
                S_IDLE: if (start_i) begin
                    // All automatic declarations must precede any
                    // procedural statement in this block (Quartus 17
                    // strictly enforces SystemVerilog ordering rules).
                    automatic logic [15:0] fbw;
                    automatic logic [15:0] fbh;
                    automatic logic [15:0] cx0, cy0, cx1, cy1;
                    automatic logic [15:0] ex0, ey0, ex1, ey1;
                    automatic logic [15:0] eff_w, eff_h;
                    automatic logic [15:0] sox, soy;

                    fbw = target_width_i;
                    fbh = target_height_i;

                    // Effective clip = (ignore_clip ? FB-only :
                    //                   user_clip ∩ FB).
                    if (clip_en_i & ~ignore_clip_i) begin
                        cx0 = u16_max(clip_x_i, 16'd0);
                        cy0 = u16_max(clip_y_i, 16'd0);
                        cx1 = u16_min(clip_x_i + clip_w_i, fbw);
                        cy1 = u16_min(clip_y_i + clip_h_i, fbh);
                    end else begin
                        cx0 = 16'd0;
                        cy0 = 16'd0;
                        cx1 = fbw;
                        cy1 = fbh;
                    end

                    // dst ∩ effective_clip.
                    ex0 = u16_max(dst_x_i, cx0);
                    ey0 = u16_max(dst_y_i, cy0);
                    ex1 = u16_min(dst_x_i + dst_w_i, cx1);
                    ey1 = u16_min(dst_y_i + dst_h_i, cy1);
                    eff_w = (ex1 > ex0) ? (ex1 - ex0) : 16'd0;
                    eff_h = (ey1 > ey0) ? (ey1 - ey0) : 16'd0;
                    // Source offsets (1:1 scale): advance src equally
                    // to however far dst was shifted on left/top.
                    sox = ex0 - dst_x_i;
                    soy = ey0 - dst_y_i;

                    mode_q       <= mode_i;
                    blend_q      <= blend_i;
                    format_q     <= format_i;
                    tint_en_q    <= tint_en_i;
                    tint_color_q <= tint_color_i;
                    dst_x_q      <= ex0;
                    dst_y_q      <= ey0;
                    dst_w_q      <= eff_w;
                    dst_h_q      <= eff_h;
                    color_q      <= color_i;
                    src_x_q      <= src_x_i + sox;
                    src_y_q      <= src_y_i + soy;
                    src_addr_q   <= src_addr_i;
                    src_pitch_q  <= src_pitch_i;
                    cur_y_off    <= '0;
                    // If the rect is fully clipped (eff_w == 0 or
                    // eff_h == 0), S_ROW_INIT immediately finds
                    // cur_y_off == dst_h_q == 0 and falls to S_DONE.
                    state        <= S_ROW_INIT;
                end

                S_ROW_INIT: begin
                    if (cur_y_off == dst_h_q) begin
                        state <= S_DONE;
                    end else begin
                        dst_row_byte_addr <= target_base_i
                            + ({16'd0, (dst_y_q + cur_y_off)} * target_pitch_i)
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
                        // Pair-COPY eligibility: RGBA src + dst, both
                        // pixel byte-addresses 8-byte aligned (i.e. their
                        // x is even), and at least 2 pixels remaining in
                        // the row. Misaligned ends or A8 sources fall
                        // back to the per-pixel path.
                        automatic logic [15:0] remaining_copy;
                        automatic logic        dst_aligned_pair;
                        automatic logic        src_aligned_pair;
                        automatic logic        pair_eligible;
                        remaining_copy   = dst_w_q - cur_x;
                        dst_aligned_pair = ~(dst_x_q[0] ^ cur_x[0]);
                        src_aligned_pair = ~(src_x_q[0] ^ cur_x[0]);
                        pair_eligible    = (format_q == FMT_RGBA)
                                         & dst_aligned_pair
                                         & src_aligned_pair
                                         & (remaining_copy >= 16'd2);
                        if (pair_eligible) begin
                            state <= S_FETCH_SRC_PAIR;
                        end else begin
                            state <= S_FETCH_SRC;
                        end
                    end else if (blend_q == BLEND_OPAQUE) begin
                        // FILL Opaque — burst write 2 pixels per beat
                        // when start + length are 2-pixel-aligned. For
                        // odd start or odd remaining length, fall back
                        // to per-pixel writes.
                        automatic logic [15:0] remaining;
                        automatic logic        aligned_start;
                        automatic logic        aligned_len;
                        automatic logic [15:0] beats;

                        remaining     = dst_w_q - cur_x;
                        // Beat-aligned start iff (dst_x + cur_x) is
                        // even. XOR of low bits gives parity.
                        aligned_start = ~(dst_x_q[0] ^ cur_x[0]);
                        aligned_len   = (remaining[0] == 1'b0);
                        beats         = remaining >> 1;
                        if (aligned_start && aligned_len && remaining > 16'd1) begin
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
                    automatic logic [7:0]  src_alpha;
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
                    src_alpha = ch_a(computed_src);

                    // Fast paths for SrcAlpha — saves DDRAM round-trips
                    // for the common cases at glyph edges and interiors.
                    //   alpha == 0xFF → out = src, no dst read needed.
                    //   alpha == 0x00 → out = dst, skip both read and write.
                    // (Both follow from the SrcAlpha blend formula
                    //  out.RGB = src.RGB + dst.RGB * (1 - src.A) / 256
                    //  with computed_src already premultiplied by alpha.)
                    if (blend_q == BLEND_OPAQUE) begin
                        pixel_data <= computed_src;
                        state      <= S_WRITE;
                    end else if ((blend_q == BLEND_SRCALPHA)
                                 && (src_alpha == 8'hFF)) begin
                        pixel_data <= computed_src;
                        state      <= S_WRITE;
                    end else if ((blend_q == BLEND_SRCALPHA)
                                 && (src_alpha == 8'h00)) begin
                        cur_x <= cur_x + 16'd1;
                        state <= S_NEXT_PIXEL;
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

                // ----- Pair-COPY path (RGBA + aligned, ≥2 pixels) -----
                S_FETCH_SRC_PAIR: if (~ddram_busy_i) begin
                    state <= S_WAIT_SRC_PAIR;
                end

                S_WAIT_SRC_PAIR: if (ddram_dout_valid_i) begin
                    // The 64-bit beat holds two RGBA pixels. cur_x is the
                    // lower-x pixel of the pair, so it lives in the low
                    // half of the beat (DDRAM is little-endian).
                    automatic logic [31:0] src_lo;
                    automatic logic [31:0] src_hi;
                    automatic logic [31:0] computed_lo;
                    automatic logic [31:0] computed_hi;
                    automatic logic [7:0]  alpha_lo;
                    automatic logic [7:0]  alpha_hi;
                    src_lo = ddram_dout_i[31:0];
                    src_hi = ddram_dout_i[63:32];
                    if (tint_en_q) begin
                        computed_lo = pack_pixel(
                            mul8(ch_r(src_lo), ch_r(tint_color_q)),
                            mul8(ch_g(src_lo), ch_g(tint_color_q)),
                            mul8(ch_b(src_lo), ch_b(tint_color_q)),
                            mul8(ch_a(src_lo), ch_a(tint_color_q))
                        );
                        computed_hi = pack_pixel(
                            mul8(ch_r(src_hi), ch_r(tint_color_q)),
                            mul8(ch_g(src_hi), ch_g(tint_color_q)),
                            mul8(ch_b(src_hi), ch_b(tint_color_q)),
                            mul8(ch_a(src_hi), ch_a(tint_color_q))
                        );
                    end else begin
                        computed_lo = src_lo;
                        computed_hi = src_hi;
                    end
                    alpha_lo = ch_a(computed_lo);
                    alpha_hi = ch_a(computed_hi);

                    src_pair_q <= {computed_hi, computed_lo};

                    // Pair-level fast paths:
                    //   Opaque blend OR both alphas == 0xFF → write src
                    //                                          directly.
                    //   SrcAlpha + both alphas == 0          → skip pair.
                    //   anything else                         → fall to
                    //                                          dst RMW.
                    if ((blend_q == BLEND_OPAQUE)
                        || ((blend_q == BLEND_SRCALPHA)
                            && (alpha_lo == 8'hFF) && (alpha_hi == 8'hFF))) begin
                        pair_data_q <= {computed_hi, computed_lo};
                        state       <= S_WRITE_PAIR;
                    end else if ((blend_q == BLEND_SRCALPHA)
                                 && (alpha_lo == 8'h00) && (alpha_hi == 8'h00)) begin
                        cur_x <= cur_x + 16'd2;
                        state <= S_NEXT_PIXEL;
                    end else begin
                        state <= S_FETCH_DST_PAIR;
                    end
                end

                S_FETCH_DST_PAIR: if (~ddram_busy_i) begin
                    state <= S_WAIT_DST_PAIR;
                end

                S_WAIT_DST_PAIR: if (ddram_dout_valid_i) begin
                    dst_pair_q <= ddram_dout_i;
                    state      <= S_BLEND_PAIR;
                end

                S_BLEND_PAIR: begin
                    // Blend each half independently. blend_pixel is pure
                    // combinational; both calls fan out from the same
                    // captured sources so timing closure should match
                    // the per-pixel S_BLEND path.
                    pair_data_q <= {
                        blend_pixel(src_pair_q[63:32], dst_pair_q[63:32], blend_q),
                        blend_pixel(src_pair_q[31:0],  dst_pair_q[31:0],  blend_q)
                    };
                    state <= S_WRITE_PAIR;
                end

                S_WRITE_PAIR: if (~ddram_busy_i) begin
                    state <= S_WRITE_WAIT_PAIR;
                end

                S_WRITE_WAIT_PAIR: begin
                    cur_x <= cur_x + 16'd2;
                    state <= S_NEXT_PIXEL;
                end

                S_DONE: begin
                    done_o <= 1'b1;
                    state  <= S_IDLE;
                end
            endcase
        end
    end

endmodule
