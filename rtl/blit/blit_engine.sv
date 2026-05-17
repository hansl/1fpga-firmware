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
    // Source rect width/height. When `src_w_i == dst_w_i &&
    // src_h_i == dst_h_i` the engine takes the existing 1:1 path
    // (burst-friendly). Otherwise it routes into the nearest-neighbor
    // scaled-copy path: per-pixel src reads with a fixed-point step
    // accumulator, no src bursts. Ignored when `mode_i == MODE_FILL`.
    input  logic [15:0] src_w_i,
    input  logic [15:0] src_h_i,
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

    // Burst-COPY path (RGBA→RGBA): each transaction reads/writes up to
    // BURST_BEATS_MAX consecutive 64-bit beats (= 2 RGBA pixels each).
    // S_NEXT_PIXEL picks the largest aligned burst that fits the
    // remainder of the row; misaligned starts and odd-length tails fall
    // back to the per-pixel path. A8 sources stay per-pixel (different
    // addressing).
    //
    // BURST_BEATS_MAX = 8 (16 RGBA pixels per transaction). The slave's
    // burstcnt tolerates this — FILL_BURST already uses up to 255.
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
        S_FETCH_SRC_BURST, // burst src read (1..8 beats = 2..16 RGBA pixels)
        S_WAIT_SRC_BURST,
        S_FETCH_DST_BURST, // burst dst read for RMW
        S_WAIT_DST_BURST,
        S_ISSUE_PREFETCH,  // 1-cycle: drive RD=1 for next-burst src prefetch
        S_BLEND_BURST,     // sequential blend: 1 pixel/cycle, in-place into src_buf
        S_WRITE_BURST,     // burst dst write
        S_DONE
    } state_e;

    // Burst sizing.
    localparam int BURST_BEATS_MAX  = 8;
    localparam int BURST_PIXELS_MAX = BURST_BEATS_MAX * 2;

    state_e      state;
    logic        mode_q;
    logic [1:0]  blend_q;
    logic        format_q;
    logic        tint_en_q;
    logic [31:0] tint_color_q;
    logic [15:0] dst_x_q, dst_y_q, dst_w_q, dst_h_q;
    logic [31:0] color_q;
    logic [15:0] src_x_q, src_y_q;
    logic [15:0] src_w_q, src_h_q;
    logic [31:0] src_addr_q;
    logic [31:0] src_pitch_q;

    // ---- Scaled-copy state (nearest-neighbour) --------------------
    // Set in S_IDLE when (src_w != dst_w || src_h != dst_h). Forces
    // the COPY path through the per-pixel branch (no burst, no
    // prefetch) and replaces the linear src column index with a
    // Q16.16 accumulator stepped by `src_w_q / dst_w_q` per pixel.
    // `src_y_acc_q` is per-row; `src_x_acc_q` resets to its init
    // value at the start of every row.
    logic        scale_mode_q;
    logic [31:0] src_step_x_q;   // Q16.16: src dx per dst pixel
    logic [31:0] src_step_y_q;   // Q16.16: src dy per dst row
    logic [31:0] src_x_acc_q;    // Q16.16, reset per row
    logic [31:0] src_y_acc_q;    // Q16.16, advanced per row
    logic [31:0] src_x_init_q;   // Q16.16: initial src_x_acc (clipped sox in src space)
    logic [31:0] src_y_init_q;   // Q16.16: initial src_y_acc

    // One-pixel src cache. In upscale (the typical case) consecutive
    // dst pixels often map to the same src column — caching the
    // tinted/computed result and reusing it skips the DDR read +
    // 64-bit beat wait, which on this design is ~20-40 cycles. Big
    // win at 1.5x..2x scale; harmless at 1:1 (we just never check).
    // Reset to invalid at every S_ROW_INIT so a new src_y can't
    // accidentally hit the previous row's cached x.
    logic [31:0] cached_src_pixel_q;  // post-tint / post-A8-expand
    logic [15:0] last_src_col_q;
    logic        cache_valid_q;

    logic [15:0] cur_x, cur_y_off;
    logic [31:0] dst_row_byte_addr;
    logic [31:0] src_row_byte_addr;
    logic [31:0] pixel_data;
    logic [31:0] src_pixel_q;        // computed source pixel held while we fetch dst
    logic [31:0] dst_pixel_q;        // captured dst pixel held while blend computes
    logic [7:0]  burst_len_q;        // beats in current burst (1..255)
    logic [7:0]  burst_done_q;       // beats accepted in current burst

    // Burst-COPY working set:
    //   src_buf — captured src pixels (already tinted), reused for the
    //             blended output during S_BLEND_BURST so the 4×16×32-bit
    //             write port doesn't double-up on register usage.
    //   dst_buf — captured dst pixels for RMW.
    //   alpha_or  / alpha_and  — accumulated across all src pixels in the
    //                            burst so the burst-level fast paths can
    //                            decide skip / direct-write / RMW with one
    //                            comparison.
    logic [31:0] src_buf [0:BURST_PIXELS_MAX-1];
    logic [31:0] dst_buf [0:BURST_PIXELS_MAX-1];
    logic [7:0]  alpha_or_q;
    logic [7:0]  alpha_and_q;
    // Active burst length in beats (1..BURST_BEATS_MAX).
    logic [3:0]  copy_burst_len_q;
    // Per-burst beat / pixel cursors.
    logic [3:0]  copy_beat_idx_q;
    logic [4:0]  copy_pixel_idx_q;

    // ---- Src prefetch (outstanding-read pipelining) -------------------
    //
    // For the common case of contiguous 16-pixel bursts in the middle
    // of a row (RGBA, both src and dst 64-byte aligned), we hide the
    // bus-read latency of the *next* burst's src behind the current
    // burst's blend + write phases. The read is issued from
    // S_ISSUE_PREFETCH after current's dst capture (or after src in
    // fast-path frames that skip dst), and beats land into prefetch_buf
    // throughout S_BLEND_BURST and S_WRITE_BURST as ddram_dout_valid
    // pulses arrive. By the time we transition to S_NEXT_PIXEL the
    // prefetch is typically complete; the dispatch in S_NEXT_PIXEL
    // checks prefetch_ready_q and, if the next burst's parameters
    // match, copies prefetch_buf into src_buf in one cycle and skips
    // straight past S_FETCH_SRC_BURST / S_WAIT_SRC_BURST.
    //
    // Only the 16-pixel (8-beat) tier participates: the smaller tiers
    // are too narrow to be worth the bookkeeping, and prefetch is
    // suppressed for the last burst of a row (no next-burst to pair
    // with) and for non-RGBA / misaligned cases.
    logic [31:0] prefetch_buf [0:BURST_PIXELS_MAX-1];
    logic [7:0]  prefetch_alpha_or_q;
    logic [7:0]  prefetch_alpha_and_q;
    logic        prefetch_active_q;     // dout_valid pulses we observe go into prefetch_buf
    logic        prefetch_ready_q;      // all 8 beats captured; dispatch may consume
    logic [3:0]  prefetch_beat_idx_q;   // 0..7
    // Cached byte address the prefetch was issued for. The dispatch
    // in S_NEXT_PIXEL verifies this matches src_pixel_byte_addr before
    // adopting prefetch_buf — guards against any dispatch edge case
    // (row transition, alignment changes) that would invalidate it.
    logic [31:0] prefetch_src_addr_q;

    assign busy_o = (state != S_IDLE) & (state != S_DONE);

    // Per-pixel byte addresses.
    // RGBA8888: x*4. A8: x*1.
    //
    // Source-side column index: in 1:1 mode it's the dst column
    // `cur_x` directly; in scaled mode it's the integer part of the
    // Q16.16 src-x accumulator (nearest-neighbour sampling).
    wire [15:0] src_col_eff = scale_mode_q ? src_x_acc_q[31:16] : cur_x;
    wire [31:0] cur_x_offset_dst = ({16'd0, cur_x} <<< 2);
    wire [31:0] cur_x_offset_src = (format_q == FMT_A8)
                                       ? {16'd0, src_col_eff}
                                       : ({16'd0, src_col_eff} <<< 2);
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
            S_FETCH_SRC_BURST: begin
                // Multi-beat aligned read. Slave latches addr+burstcnt
                // on the first cycle (beat_idx == 0) and streams beats
                // back via dout_valid. We hold rd=1 until the slave
                // accepts it (~busy), then move to wait.
                ddram_addr_o     = src_pixel_byte_addr[31:3];
                ddram_burstcnt_o = {4'd0, copy_burst_len_q};
                ddram_be_o       = 8'hFF;
                ddram_rd_o       = 1'b1;
            end
            S_FETCH_DST_BURST: begin
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = {4'd0, copy_burst_len_q};
                ddram_be_o       = 8'hFF;
                ddram_rd_o       = 1'b1;
            end
            S_WRITE_BURST: begin
                // Hold address+burstcnt+we for the whole burst. din
                // changes per beat — we emit the (2k, 2k+1) pixel pair
                // for the current beat_idx.
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = {4'd0, copy_burst_len_q};
                ddram_be_o       = 8'hFF;
                ddram_din_o      = {
                    src_buf[{copy_beat_idx_q, 1'b1}],
                    src_buf[{copy_beat_idx_q, 1'b0}]
                };
                ddram_we_o       = 1'b1;
            end
            S_ISSUE_PREFETCH: begin
                // Issue the next-burst src read. Fixed 8-beat burst:
                // prefetch only ever runs for the 16-pixel tier.
                ddram_addr_o     = prefetch_src_addr_q[31:3];
                ddram_burstcnt_o = 8'd8;
                ddram_be_o       = 8'hFF;
                ddram_rd_o       = 1'b1;
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
    integer i;
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
            src_w_q           <= '0;
            src_h_q           <= '0;
            src_addr_q        <= '0;
            src_pitch_q       <= '0;
            scale_mode_q      <= 1'b0;
            src_step_x_q      <= 32'h0001_0000;
            src_step_y_q      <= 32'h0001_0000;
            src_x_acc_q       <= '0;
            src_y_acc_q       <= '0;
            src_x_init_q      <= '0;
            src_y_init_q      <= '0;
            cached_src_pixel_q <= '0;
            last_src_col_q    <= '0;
            cache_valid_q     <= 1'b0;
            cur_x             <= '0;
            cur_y_off         <= '0;
            dst_row_byte_addr <= '0;
            src_row_byte_addr <= '0;
            pixel_data        <= '0;
            src_pixel_q       <= '0;
            dst_pixel_q       <= '0;
            burst_len_q       <= '0;
            burst_done_q      <= '0;
            for (i = 0; i < BURST_PIXELS_MAX; i = i + 1) begin
                src_buf[i]      <= '0;
                dst_buf[i]      <= '0;
                prefetch_buf[i] <= '0;
            end
            alpha_or_q           <= '0;
            alpha_and_q          <= '0;
            copy_burst_len_q     <= 4'd0;
            copy_beat_idx_q      <= 4'd0;
            copy_pixel_idx_q     <= 5'd0;
            prefetch_alpha_or_q  <= 8'd0;
            prefetch_alpha_and_q <= 8'hFF;
            prefetch_active_q    <= 1'b0;
            prefetch_ready_q     <= 1'b0;
            prefetch_beat_idx_q  <= 4'd0;
            prefetch_src_addr_q  <= 32'd0;
            done_o               <= 1'b0;
        end else begin
            done_o <= 1'b0;

            // ---- Prefetch capture (concurrent with main FSM) ----
            // Once a prefetch read is in flight (prefetch_active_q high
            // after S_ISSUE_PREFETCH retires), every dout_valid pulse
            // is for prefetch — current-burst reads have all completed
            // by then because we only issue prefetch *after* the main
            // FSM finishes capturing src/dst for the burst we're about
            // to write. We rely on the slave returning beats in issue
            // order, so this happens to also work in fast-path frames
            // where we skipped dst.
            if (prefetch_active_q & ddram_dout_valid_i) begin
                automatic logic [31:0] p_lo;
                automatic logic [31:0] p_hi;
                automatic logic [31:0] tinted_lo;
                automatic logic [31:0] tinted_hi;
                automatic logic [4:0]  p_lo_idx;
                automatic logic [4:0]  p_hi_idx;
                automatic logic [7:0]  al;
                automatic logic [7:0]  ah;
                automatic logic        prefetch_last;
                p_lo = ddram_dout_i[31:0];
                p_hi = ddram_dout_i[63:32];
                if (tint_en_q) begin
                    tinted_lo = pack_pixel(
                        mul8(ch_r(p_lo), ch_r(tint_color_q)),
                        mul8(ch_g(p_lo), ch_g(tint_color_q)),
                        mul8(ch_b(p_lo), ch_b(tint_color_q)),
                        mul8(ch_a(p_lo), ch_a(tint_color_q))
                    );
                    tinted_hi = pack_pixel(
                        mul8(ch_r(p_hi), ch_r(tint_color_q)),
                        mul8(ch_g(p_hi), ch_g(tint_color_q)),
                        mul8(ch_b(p_hi), ch_b(tint_color_q)),
                        mul8(ch_a(p_hi), ch_a(tint_color_q))
                    );
                end else begin
                    tinted_lo = p_lo;
                    tinted_hi = p_hi;
                end
                p_lo_idx = {prefetch_beat_idx_q, 1'b0};
                p_hi_idx = {prefetch_beat_idx_q, 1'b1};
                prefetch_buf[p_lo_idx] <= tinted_lo;
                prefetch_buf[p_hi_idx] <= tinted_hi;
                al = ch_a(tinted_lo);
                ah = ch_a(tinted_hi);
                prefetch_alpha_or_q  <= prefetch_alpha_or_q  | al | ah;
                prefetch_alpha_and_q <= prefetch_alpha_and_q & al & ah;
                prefetch_last = (prefetch_beat_idx_q + 4'd1 == 4'd8);
                if (prefetch_last) begin
                    prefetch_active_q   <= 1'b0;
                    prefetch_ready_q    <= 1'b1;
                    prefetch_beat_idx_q <= 4'd0;
                end else begin
                    prefetch_beat_idx_q <= prefetch_beat_idx_q + 4'd1;
                end
            end

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
                    // Scale-mode locals — declared at the top so the
                    // nested if/else doesn't need its own
                    // declarations (Quartus 17 rejects automatic in
                    // nested blocks).
                    automatic logic        do_scale;
                    automatic logic [31:0] step_x;
                    automatic logic [31:0] step_y;

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
                    // For 1:1 (non-scaled) blits the existing src
                    // start advances by sox/soy in dst space (1:1).
                    // For scaled blits we keep src_x/y_q at the
                    // unclipped src origin and let the row/pixel
                    // accumulators handle the scaled left/top offset.
                    do_scale = (mode_i == MODE_COPY) &&
                               ((src_w_i != dst_w_i) || (src_h_i != dst_h_i));
                    src_x_q      <= src_x_i + (do_scale ? 16'd0 : sox);
                    src_y_q      <= src_y_i + (do_scale ? 16'd0 : soy);
                    src_w_q      <= src_w_i;
                    src_h_q      <= src_h_i;
                    src_addr_q   <= src_addr_i;
                    src_pitch_q  <= src_pitch_i;
                    cur_y_off    <= '0;

                    // --- Scale-mode setup --------------------------
                    // Active only on COPY when src and dst dimensions
                    // differ. Step = (src_dim / dst_dim) in Q16.16.
                    // Initial src accumulators include sox·step /
                    // soy·step so left/top clipping advances the src
                    // tap by the scaled amount, not a 1:1 amount.
                    // (Guard against dst==0 — degenerate fully-
                    // clipped — S_ROW_INIT short-circuits anyway.)
                    // `do_scale` is reused from the src_x/y_q
                    // selection above.
                    step_x = (dst_w_i == 16'd0)
                             ? 32'h0001_0000
                             : ({16'd0, src_w_i} <<< 16) / {16'd0, dst_w_i};
                    step_y = (dst_h_i == 16'd0)
                             ? 32'h0001_0000
                             : ({16'd0, src_h_i} <<< 16) / {16'd0, dst_h_i};
                    if (do_scale) begin
                        scale_mode_q <= 1'b1;
                        src_step_x_q <= step_x;
                        src_step_y_q <= step_y;
                        // sox/soy were computed in dst pixels (1:1
                        // sense). Multiply by step to get the
                        // matching Q16.16 advance in src space.
                        src_x_init_q <= {16'd0, sox} * step_x;
                        src_y_init_q <= {16'd0, soy} * step_y;
                        src_x_acc_q  <= {16'd0, sox} * step_x;
                        src_y_acc_q  <= {16'd0, soy} * step_y;
                    end else begin
                        scale_mode_q <= 1'b0;
                        src_step_x_q <= 32'h0001_0000;
                        src_step_y_q <= 32'h0001_0000;
                        src_x_init_q <= 32'd0;
                        src_y_init_q <= 32'd0;
                        src_x_acc_q  <= 32'd0;
                        src_y_acc_q  <= 32'd0;
                    end

                    // If the rect is fully clipped (eff_w == 0 or
                    // eff_h == 0), S_ROW_INIT immediately finds
                    // cur_y_off == dst_h_q == 0 and falls to S_DONE.
                    state        <= S_ROW_INIT;
                end

                S_ROW_INIT: begin
                    if (cur_y_off == dst_h_q) begin
                        state <= S_DONE;
                    end else begin
                        automatic logic [15:0] src_y_eff;
                        // In scale mode the src row index comes from
                        // the Q16.16 accumulator (integer part). In
                        // 1:1 mode it's the dst row offset directly.
                        src_y_eff = scale_mode_q
                                        ? src_y_acc_q[31:16]
                                        : cur_y_off;
                        dst_row_byte_addr <= target_base_i
                            + ({16'd0, (dst_y_q + cur_y_off)} * target_pitch_i)
                            + ({16'd0, dst_x_q} <<< 2);
                        // Row base: src.x byte multiplier depends on
                        // format. src_x_q already has sox baked in
                        // for 1:1, or stays at src_x_i for scale (the
                        // sox·step bias lives in src_x_acc_q below).
                        src_row_byte_addr <= src_addr_q
                            + ({16'd0, (src_y_q + src_y_eff)} * src_pitch_q)
                            + ((format_q == FMT_A8)
                                  ? {16'd0, src_x_q}
                                  : ({16'd0, src_x_q} <<< 2));
                        cur_x <= '0;
                        // Restart the src-x accumulator at its row-
                        // start value every row; src_y advances by
                        // one step per row from S_NEXT_PIXEL's
                        // end-of-row branch.
                        src_x_acc_q <= src_x_init_q;
                        // New row → src_y changed → cached pixel
                        // (which was from the previous row's src
                        // column) is no longer valid for any
                        // column on this row.
                        cache_valid_q <= 1'b0;
                        state <= S_NEXT_PIXEL;
                    end
                end

                S_NEXT_PIXEL: begin
                    if (cur_x == dst_w_q) begin
                        cur_y_off <= cur_y_off + 16'd1;
                        // Advance the row-step accumulator in scale
                        // mode so S_ROW_INIT picks up the next src_y.
                        if (scale_mode_q) begin
                            src_y_acc_q <= src_y_acc_q + src_step_y_q;
                        end
                        // Row's last burst can't match the prefetch
                        // (it'd be for a same-row position past dst_w);
                        // the address-match check would handle this
                        // anyway, but clearing makes the intent explicit.
                        prefetch_ready_q <= 1'b0;
                        state     <= S_ROW_INIT;
                    end else if (mode_q == MODE_COPY && scale_mode_q) begin
                        // Scale-mode COPY: per-pixel reads with a
                        // 1-pixel src cache. Consecutive dst pixels
                        // landing on the same src column reuse the
                        // last fetched (and post-tint / post-A8)
                        // result — the upscale "read once, write a
                        // bunch" optimisation. Cache miss falls
                        // through to the normal per-pixel fetch.
                        automatic logic [7:0] cached_alpha;
                        cached_alpha = ch_a(cached_src_pixel_q);
                        if (cache_valid_q && (src_col_eff == last_src_col_q)) begin
                            // Mirror S_WAIT_SRC's tail dispatch, but
                            // using the cached value instead of the
                            // just-fetched one.
                            if ((blend_q == BLEND_OPAQUE)
                                || ((blend_q == BLEND_SRCALPHA)
                                    && (cached_alpha == 8'hFF))) begin
                                pixel_data <= cached_src_pixel_q;
                                state      <= S_WRITE;
                            end else if ((blend_q == BLEND_SRCALPHA)
                                         && (cached_alpha == 8'h00)) begin
                                // Fully transparent: dst unchanged.
                                // Same per-pixel step bump as the
                                // alpha-zero path in S_WAIT_SRC.
                                cur_x <= cur_x + 16'd1;
                                src_x_acc_q <= src_x_acc_q + src_step_x_q;
                                state <= S_NEXT_PIXEL;
                            end else begin
                                src_pixel_q <= cached_src_pixel_q;
                                state       <= S_FETCH_DST;
                            end
                        end else begin
                            state <= S_FETCH_SRC;
                        end
                    end else if (mode_q == MODE_COPY) begin
                        // Burst-COPY dispatch: pick the largest aligned
                        // burst that fits the remainder of the row.
                        // Both src and dst pixel byte-addresses must be
                        // (burst_pixels * 4)-byte aligned for the slave to
                        // accept the burst. Falls back through smaller
                        // bursts down to the per-pixel path. A8 sources
                        // bypass burst entirely (different addressing).
                        //
                        // For 16-px bursts we also check whether a
                        // src prefetch (issued during the previous
                        // burst's dst-wait) has landed and matches the
                        // address we'd otherwise fetch. If so, copy
                        // prefetch_buf into src_buf in one cycle and
                        // jump straight to the alpha-summary decision
                        // — saving the L1 cycles a fresh fetch would
                        // pay.
                        automatic logic [15:0] remaining_copy;
                        automatic logic [5:0]  src_lo;
                        automatic logic [5:0]  dst_lo;
                        automatic logic        rgba;
                        automatic logic        prefetch_hit;
                        remaining_copy = dst_w_q - cur_x;
                        src_lo = src_pixel_byte_addr[5:0];
                        dst_lo = dst_pixel_byte_addr[5:0];
                        rgba = (format_q == FMT_RGBA);
                        prefetch_hit = prefetch_ready_q
                                     & (prefetch_src_addr_q == src_pixel_byte_addr);
                        if (rgba && (remaining_copy >= 16'd16)
                            && (src_lo == 6'd0) && (dst_lo == 6'd0)
                            && prefetch_hit) begin
                            // Use prefetch_buf as src_buf. Replicates
                            // the alpha-summary branch from
                            // S_WAIT_SRC_BURST's last-beat handler.
                            // Manual unroll keeps Quartus 17 happy
                            // about loop-variable scoping inside an
                            // always_ff branch.
                            src_buf[0]  <= prefetch_buf[0];
                            src_buf[1]  <= prefetch_buf[1];
                            src_buf[2]  <= prefetch_buf[2];
                            src_buf[3]  <= prefetch_buf[3];
                            src_buf[4]  <= prefetch_buf[4];
                            src_buf[5]  <= prefetch_buf[5];
                            src_buf[6]  <= prefetch_buf[6];
                            src_buf[7]  <= prefetch_buf[7];
                            src_buf[8]  <= prefetch_buf[8];
                            src_buf[9]  <= prefetch_buf[9];
                            src_buf[10] <= prefetch_buf[10];
                            src_buf[11] <= prefetch_buf[11];
                            src_buf[12] <= prefetch_buf[12];
                            src_buf[13] <= prefetch_buf[13];
                            src_buf[14] <= prefetch_buf[14];
                            src_buf[15] <= prefetch_buf[15];
                            alpha_or_q       <= prefetch_alpha_or_q;
                            alpha_and_q      <= prefetch_alpha_and_q;
                            prefetch_ready_q <= 1'b0;
                            copy_burst_len_q <= 4'd8;
                            copy_beat_idx_q  <= 4'd0;
                            copy_pixel_idx_q <= 5'd0;
                            if ((blend_q == BLEND_OPAQUE)
                                || ((blend_q == BLEND_SRCALPHA)
                                    && (prefetch_alpha_and_q == 8'hFF))) begin
                                state <= S_WRITE_BURST;
                            end else if ((blend_q == BLEND_SRCALPHA)
                                         && (prefetch_alpha_or_q == 8'h00)) begin
                                cur_x <= cur_x + 16'd16;
                                state <= S_NEXT_PIXEL;
                            end else begin
                                state <= S_FETCH_DST_BURST;
                            end
                        end else if (rgba && (remaining_copy >= 16'd16)
                            && (src_lo == 6'd0) && (dst_lo == 6'd0)) begin
                            // 16 px / 8 beats / 64-byte aligned.
                            copy_burst_len_q <= 4'd8;
                            copy_beat_idx_q  <= 4'd0;
                            copy_pixel_idx_q <= 5'd0;
                            alpha_or_q       <= 8'd0;
                            alpha_and_q      <= 8'hFF;
                            state            <= S_FETCH_SRC_BURST;
                        end else if (rgba && (remaining_copy >= 16'd8)
                                     && (src_lo[4:0] == 5'd0)
                                     && (dst_lo[4:0] == 5'd0)) begin
                            copy_burst_len_q <= 4'd4;
                            copy_beat_idx_q  <= 4'd0;
                            copy_pixel_idx_q <= 5'd0;
                            alpha_or_q       <= 8'd0;
                            alpha_and_q      <= 8'hFF;
                            state            <= S_FETCH_SRC_BURST;
                        end else if (rgba && (remaining_copy >= 16'd4)
                                     && (src_lo[3:0] == 4'd0)
                                     && (dst_lo[3:0] == 4'd0)) begin
                            copy_burst_len_q <= 4'd2;
                            copy_beat_idx_q  <= 4'd0;
                            copy_pixel_idx_q <= 5'd0;
                            alpha_or_q       <= 8'd0;
                            alpha_and_q      <= 8'hFF;
                            state            <= S_FETCH_SRC_BURST;
                        end else if (rgba && (remaining_copy >= 16'd2)
                                     && (src_lo[2:0] == 3'd0)
                                     && (dst_lo[2:0] == 3'd0)) begin
                            copy_burst_len_q <= 4'd1;
                            copy_beat_idx_q  <= 4'd0;
                            copy_pixel_idx_q <= 5'd0;
                            alpha_or_q       <= 8'd0;
                            alpha_and_q      <= 8'hFF;
                            state            <= S_FETCH_SRC_BURST;
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

                S_WAIT_SRC: if (ddram_dout_valid_i & ~prefetch_active_q) begin
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

                    // Populate the 1-pixel src cache in scale mode
                    // so the next consecutive dst pixel landing on
                    // the same src column can short-circuit the
                    // entire fetch+wait round-trip. Cache is reset
                    // at S_ROW_INIT, so cross-row reuse is off (a
                    // future enhancement could keep a small row
                    // buffer; not worth it at our current sizes).
                    if (scale_mode_q) begin
                        cached_src_pixel_q <= computed_src;
                        last_src_col_q     <= src_col_eff;
                        cache_valid_q      <= 1'b1;
                    end

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
                        // Same step bump as the S_WRITE_WAIT path:
                        // we're advancing one dst pixel here too, so
                        // src_x_acc must move forward in scale mode.
                        if (scale_mode_q) begin
                            src_x_acc_q <= src_x_acc_q + src_step_x_q;
                        end
                        state <= S_NEXT_PIXEL;
                    end else begin
                        src_pixel_q <= computed_src;
                        state       <= S_FETCH_DST;
                    end
                end

                S_FETCH_DST: if (~ddram_busy_i) begin
                    state <= S_WAIT_DST;
                end

                S_WAIT_DST: if (ddram_dout_valid_i & ~prefetch_active_q) begin
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
                    // In scale mode, step the src column accumulator
                    // one Q16.16 step per dst pixel. Non-scale mode
                    // leaves src_x_acc_q untouched (its initial 0
                    // is never read since `src_col_eff` selects on
                    // `scale_mode_q`).
                    if (scale_mode_q) begin
                        src_x_acc_q <= src_x_acc_q + src_step_x_q;
                    end
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

                // ----- Burst-COPY path (RGBA + aligned, ≥burst_pixels) -----
                S_FETCH_SRC_BURST: if (~ddram_busy_i) begin
                    state <= S_WAIT_SRC_BURST;
                end

                // Beats arriving while prefetch_active_q is high are
                // for the in-flight prefetch (concurrent capture block
                // routes them into prefetch_buf). They are NOT for
                // this wait-state's request, so we must not capture
                // them here — otherwise src_buf is filled with
                // prefetch data, copy_beat_idx_q over-advances, the
                // state thinks the burst is done after 8 "beats" that
                // were actually all prefetch, and the real read's
                // beats arrive into a later wait-state's buffer.
                // Result: horizontal slits of foreign pixels in the
                // current row, visible in menu-ui's text rasters.
                S_WAIT_SRC_BURST: if (ddram_dout_valid_i & ~prefetch_active_q) begin
                    // Each beat is a 64-bit word holding two RGBA pixels
                    // (low 32 = lower-x pixel, high 32 = upper-x). Apply
                    // tint per-pixel as we capture, and accumulate alpha
                    // OR/AND so the burst-level fast-path decision after
                    // the last beat is one comparison.
                    automatic logic [31:0] src_lo;
                    automatic logic [31:0] src_hi;
                    automatic logic [31:0] computed_lo;
                    automatic logic [31:0] computed_hi;
                    automatic logic [4:0]  pix_lo_idx;
                    automatic logic [4:0]  pix_hi_idx;
                    automatic logic [7:0]  alpha_lo;
                    automatic logic [7:0]  alpha_hi;
                    automatic logic [7:0]  next_alpha_or;
                    automatic logic [7:0]  next_alpha_and;
                    automatic logic        is_last_beat;
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
                    pix_lo_idx = {copy_beat_idx_q, 1'b0};
                    pix_hi_idx = {copy_beat_idx_q, 1'b1};
                    src_buf[pix_lo_idx] <= computed_lo;
                    src_buf[pix_hi_idx] <= computed_hi;

                    alpha_lo = ch_a(computed_lo);
                    alpha_hi = ch_a(computed_hi);
                    next_alpha_or  = alpha_or_q  | alpha_lo | alpha_hi;
                    next_alpha_and = alpha_and_q & alpha_lo & alpha_hi;
                    alpha_or_q  <= next_alpha_or;
                    alpha_and_q <= next_alpha_and;

                    is_last_beat = (copy_beat_idx_q + 4'd1 == copy_burst_len_q);
                    if (is_last_beat) begin
                        // Whole burst captured. Burst-level fast paths:
                        //   Opaque blend OR every alpha == 0xFF → write
                        //     src buffer directly, skip the dst fetch.
                        //   SrcAlpha + every alpha == 0          → skip
                        //     this burst entirely (no read, no write).
                        //   anything else                         → fall
                        //     into dst RMW for the whole burst.
                        copy_beat_idx_q <= 4'd0;
                        if ((blend_q == BLEND_OPAQUE)
                            || ((blend_q == BLEND_SRCALPHA)
                                && (next_alpha_and == 8'hFF))) begin
                            state <= S_WRITE_BURST;
                        end else if ((blend_q == BLEND_SRCALPHA)
                                     && (next_alpha_or == 8'h00)) begin
                            cur_x <= cur_x + {11'd0, copy_burst_len_q, 1'b0};
                            state <= S_NEXT_PIXEL;
                        end else begin
                            state <= S_FETCH_DST_BURST;
                        end
                    end else begin
                        copy_beat_idx_q <= copy_beat_idx_q + 4'd1;
                    end
                end

                S_FETCH_DST_BURST: if (~ddram_busy_i) begin
                    state <= S_WAIT_DST_BURST;
                end

                // Same prefetch-race guard as S_WAIT_SRC_BURST: don't
                // capture into dst_buf while prefetch beats are still
                // in flight on the response bus.
                S_WAIT_DST_BURST: if (ddram_dout_valid_i & ~prefetch_active_q) begin
                    automatic logic [4:0] pix_lo_idx;
                    automatic logic [4:0] pix_hi_idx;
                    automatic logic       is_last_beat;
                    automatic logic [15:0] remaining_after;
                    automatic logic        prefetch_eligible;
                    pix_lo_idx = {copy_beat_idx_q, 1'b0};
                    pix_hi_idx = {copy_beat_idx_q, 1'b1};
                    dst_buf[pix_lo_idx] <= ddram_dout_i[31:0];
                    dst_buf[pix_hi_idx] <= ddram_dout_i[63:32];

                    is_last_beat = (copy_beat_idx_q + 4'd1 == copy_burst_len_q);
                    if (is_last_beat) begin
                        copy_beat_idx_q  <= 4'd0;
                        copy_pixel_idx_q <= 5'd0;
                        // Eligible for src prefetch iff this is a 16-px
                        // burst and there's at least one more 16-px
                        // worth of pixels left in the row. Same-row
                        // address arithmetic is trivial (next burst is
                        // at the same y, just +64 bytes), so we can
                        // compute the prefetch address from the
                        // current src_pixel_byte_addr below.
                        remaining_after = dst_w_q - cur_x - 16'd16;
                        prefetch_eligible = (copy_burst_len_q == 4'd8)
                                          & (format_q == FMT_RGBA)
                                          & (remaining_after >= 16'd16)
                                          & ~prefetch_active_q
                                          & ~prefetch_ready_q;
                        if (prefetch_eligible) begin
                            prefetch_src_addr_q  <= src_pixel_byte_addr + 32'd64;
                            prefetch_alpha_or_q  <= 8'd0;
                            prefetch_alpha_and_q <= 8'hFF;
                            state                <= S_ISSUE_PREFETCH;
                        end else begin
                            state <= S_BLEND_BURST;
                        end
                    end else begin
                        copy_beat_idx_q <= copy_beat_idx_q + 4'd1;
                    end
                end

                S_ISSUE_PREFETCH: if (~ddram_busy_i) begin
                    // Slave latched the read request — mark prefetch
                    // active so the universal capture path (above the
                    // unique case) routes incoming dout_valid pulses
                    // into prefetch_buf. Then proceed with the blend
                    // we postponed for one cycle.
                    prefetch_active_q   <= 1'b1;
                    prefetch_beat_idx_q <= 4'd0;
                    state               <= S_BLEND_BURST;
                end

                S_BLEND_BURST: begin
                    // Sequential per-pixel blend. blend_pixel is pure
                    // combinational; doing it 1 pixel/cycle keeps the
                    // critical path identical to the per-pixel S_BLEND
                    // and amortises ~16 cycles over a burst that already
                    // saved many bus round-trips. Result lands back in
                    // src_buf so S_WRITE_BURST can stream it out.
                    automatic logic [4:0] total_pixels;
                    total_pixels = {copy_burst_len_q, 1'b0};
                    src_buf[copy_pixel_idx_q] <= blend_pixel(
                        src_buf[copy_pixel_idx_q],
                        dst_buf[copy_pixel_idx_q],
                        blend_q
                    );
                    if (copy_pixel_idx_q + 5'd1 == total_pixels) begin
                        copy_pixel_idx_q <= 5'd0;
                        state            <= S_WRITE_BURST;
                    end else begin
                        copy_pixel_idx_q <= copy_pixel_idx_q + 5'd1;
                    end
                end

                S_WRITE_BURST: if (~ddram_busy_i) begin
                    automatic logic is_last_beat;
                    is_last_beat = (copy_beat_idx_q + 4'd1 == copy_burst_len_q);
                    if (is_last_beat) begin
                        copy_beat_idx_q <= 4'd0;
                        cur_x <= cur_x + {11'd0, copy_burst_len_q, 1'b0};
                        state <= S_NEXT_PIXEL;
                    end else begin
                        copy_beat_idx_q <= copy_beat_idx_q + 4'd1;
                    end
                end

                S_DONE: begin
                    // Stall until any in-flight prefetch beats arrive
                    // and the capture block (above) clears
                    // prefetch_active_q. Pairs with the
                    // ~prefetch_active_q gate on the S_WAIT_* states:
                    // if we cleared prefetch_active_q manually here
                    // while beats were still en route, the gate
                    // would let the next blit's wait state capture
                    // them as if they were its own — exactly the
                    // misrouting the gate exists to prevent.
                    if (~prefetch_active_q) begin
                        done_o              <= 1'b1;
                        prefetch_ready_q    <= 1'b0;
                        state               <= S_IDLE;
                    end
                end
            endcase
        end
    end

endmodule
