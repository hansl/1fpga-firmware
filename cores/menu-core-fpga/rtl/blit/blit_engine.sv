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
    input  logic [1:0]  mode_i,           // 0 = FILL, 1 = COPY, 2 = AFFINE
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

    // AFFINE-only inputs (MODE_AFFINE). Inverse 2x3 matrix in Q16.16
    // signed, mapping a destination offset (ox,oy) from the dst rect
    // origin to a *source-sub-rect-relative* coordinate:
    //   srcx = m00*ox + m01*oy + tx
    //   srcy = m10*ox + m11*oy + ty
    // The host folds half-pixel centering into tx/ty; the source
    // origin (src_x_i/src_y_i) is applied by the staging load, so the
    // sampler indexes the staged sub-rect directly (0..sw-1, 0..sh-1).
    // Source sub-rect is capped at 128x128 RGBA8888 (PROTOCOL.md §5.7).
    input  logic [31:0] aff_m00_i,
    input  logic [31:0] aff_m01_i,
    input  logic [31:0] aff_m10_i,
    input  logic [31:0] aff_m11_i,
    input  logic [31:0] aff_tx_i,
    input  logic [31:0] aff_ty_i,

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

    localparam logic [1:0] MODE_FILL   = 2'd0;
    localparam logic [1:0] MODE_COPY   = 2'd1;
    localparam logic [1:0] MODE_AFFINE = 2'd2;
    localparam logic FMT_RGBA  = 1'b0;
    localparam logic FMT_A8    = 1'b1;

    localparam logic [1:0] BLEND_OPAQUE   = 2'd0;
    localparam logic [1:0] BLEND_SRCALPHA = 2'd1;
    localparam logic [1:0] BLEND_ADDITIVE = 2'd2;

    // Burst-COPY path (RGBA→RGBA): each transaction reads/writes up to
    // BURST_BEATS_MAX consecutive 64-bit beats (= 2 RGBA pixels each).
    // S_NEXT_PIXEL picks the largest burst that fits the remainder of
    // the row; the src side realigns via a skid (word-granular for
    // RGBA, byte-granular for A8), so only the dst needs even-pixel
    // alignment. Odd dst starts / odd tails take at most one
    // per-pixel step.
    //
    // BURST_BEATS_MAX = 8 (16 RGBA pixels per transaction). The slave's
    // burstcnt tolerates this — FILL_BURST already uses up to 255.
    typedef enum logic [5:0] {
        S_IDLE,
        S_DIV_WAIT,        // wait for pipelined lpm_divide on scaled COPY_RECT
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
        S_DONE,
        // ---- AFFINE path (MODE_AFFINE) ----------------------------
        // Phase 1: stage the sw x sh source sub-rect into the on-chip
        // even/odd-x banks (one DDR burst per row).
        S_AFF_SETUP,       // latch coeffs, clip, seed accumulators
        S_AFF_LOAD_ROW,    // per-row: compute row addr / burst length
        S_AFF_LOAD_FETCH,  // issue the row's burst read
        S_AFF_LOAD_WAIT,   // capture beats into the staging banks
        // Phase 2: walk the dst AABB, bilinear-gather from BRAM, blend.
        S_AFF_ROW,         // per-row: dst row addr, reset x accumulator
        S_AFF_PIX_MAC,     // compute ix/iy/frac, bank read addresses
        S_AFF_PIX_READ,    // BRAM read-latency bubble
        S_AFF_PIX_FILTER,  // bilinear stage 1: x-interp the 4 taps → top/bot rows
        S_AFF_PIX_FILTER2, // bilinear stage 2: y-interp top/bot → src_pixel_q
        S_AFF_FETCH_DST,   // RMW dst read (SrcAlpha/Additive)
        S_AFF_WAIT_DST,
        S_AFF_BLEND,       // dst blend (own cycle for timing)
        S_AFF_WRITE,       // single-pixel dst write
        S_AFF_NEXT         // advance accumulators / pixel & row cursors
    } state_e;

    // Burst sizing.
    localparam int BURST_BEATS_MAX  = 8;
    localparam int BURST_PIXELS_MAX = BURST_BEATS_MAX * 2;

    state_e      state;
    logic [1:0]  mode_q;
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

    // ---- Pipelined divider for the Q16.16 step computation -------
    // Inferred combinational `/` flattened the entire 32/16 divider
    // chain into one clock cycle (60 LUT levels, -72 ns clk_sys
    // slack — see worst-paths report). Replace with two
    // LPM_PIPELINE=8 lpm_divide instances. On entry to the scaled
    // path the FSM latches numerators/denominators (clipped to
    // dst==0 → step=1.0 below), enters S_DIV_WAIT for 8 cycles,
    // then samples div_quot_x/y on the last wait cycle.
    //
    // 8 stages × 20 ns clk_sys = 160 ns total latency, but a single
    // blit op already runs for thousands of cycles of pixel work,
    // so the overhead is negligible (and non-scaled blits skip the
    // wait entirely).
    localparam int DIV_PIPELINE_STAGES = 8;
    logic [31:0] div_num_x_q;
    logic [15:0] div_den_x_q;
    logic [31:0] div_num_y_q;
    logic [15:0] div_den_y_q;
    logic        div_den_x_zero_q;  // dst_w_i was 0 → use step=1.0
    logic        div_den_y_zero_q;
    logic [15:0] sox_q, soy_q;      // clipped left/top offset, latched for S_DIV_WAIT
    logic [3:0]  div_wait_cnt_q;    // 0..DIV_PIPELINE_STAGES-1

    wire  [31:0] div_quot_x;
    wire  [31:0] div_quot_y;
    wire  [15:0] div_rem_x;
    wire  [15:0] div_rem_y;

    lpm_divide #(
        .LPM_WIDTHN         (32),
        .LPM_WIDTHD         (16),
        .LPM_NREPRESENTATION("UNSIGNED"),
        .LPM_DREPRESENTATION("UNSIGNED"),
        .LPM_PIPELINE       (DIV_PIPELINE_STAGES),
        .LPM_TYPE           ("LPM_DIVIDE")
    ) u_step_div_x (
        .clock    (clk),
        .clken    (1'b1),
        .aclr     (1'b0),
        .numer    (div_num_x_q),
        .denom    (div_den_x_q),
        .quotient (div_quot_x),
        .remain   (div_rem_x)
    );

    lpm_divide #(
        .LPM_WIDTHN         (32),
        .LPM_WIDTHD         (16),
        .LPM_NREPRESENTATION("UNSIGNED"),
        .LPM_DREPRESENTATION("UNSIGNED"),
        .LPM_PIPELINE       (DIV_PIPELINE_STAGES),
        .LPM_TYPE           ("LPM_DIVIDE")
    ) u_step_div_y (
        .clock    (clk),
        .clken    (1'b1),
        .aclr     (1'b0),
        .numer    (div_num_y_q),
        .denom    (div_den_y_q),
        .quotient (div_quot_y),
        .remain   (div_rem_y)
    );

    // The lpm_divide remainder outputs aren't consumed; pin them
    // into an unused-suppress so synthesis doesn't warn.
    wire _unused_div_rem = |{div_rem_x, div_rem_y};

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
    // Blended-FILL burst: reuse the COPY RMW machinery
    // (S_FETCH_DST_BURST → S_BLEND_BURST → S_WRITE_BURST) with the
    // constant fill colour as the blend source. The colour is
    // pre-loaded into src_buf during S_WAIT_DST_BURST (whose src_buf
    // write port is otherwise idle), so the blend and write stages
    // run completely unchanged — no new logic on the blend critical
    // path. Without this tier a translucent fill was one DDR read
    // round-trip PER PIXEL (~300 ns/px); menu-ui's card fills alone
    // were ~200 ms/frame.
    logic        fill_blend_q;
    // Src realignment skid: pixels are 32-bit, beats 64-bit, so any
    // src-vs-dst misalignment is exactly one WORD or none. skew=1
    // means the src pixel run starts in the HIGH half of its first
    // beat: fetch one extra beat and have the capture path drop the
    // leading word (and the trailing one on the last beat). Constant
    // along a row (src and dst x advance together).
    logic        copy_src_skew_q;
    logic [3:0]  copy_fetch_beats_q;   // src fetch beats (RGBA 1..9, A8 1..3)
    // A8 skid: one byte per pixel, so the src run starts at any of the
    // 8 byte lanes of its first beat. Byte-granular analogue of
    // copy_src_skew_q; only one of the two is nonzero per burst.
    logic [2:0]  copy_a8_skew_q;
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

    // ================= AFFINE path state =============================
    //
    // Staging banks: the source sub-rect (≤128×128 RGBA) is split by
    // sub-rect-column parity into an even-x bank and an odd-x bank, each
    // mirrored into two physical copies (a/b) so a bilinear 2×2 quad —
    // which always straddles one even and one odd column across two
    // rows — reads all four taps in a single cycle (even/odd give the
    // two columns, a/b give rows iy and iy+1). 128 rows × 64 (x/2) cols
    // × 32-bit = 8192 entries each. Address = {row[6:0], (col>>1)[5:0]}.
    localparam int AFF_MAX_DIM = 128;
    localparam int AFF_DEPTH   = AFF_MAX_DIM * (AFF_MAX_DIM/2);  // 8192
    (* ramstyle = "M10K" *) logic [31:0] stg_even_a [0:AFF_DEPTH-1];
    (* ramstyle = "M10K" *) logic [31:0] stg_even_b [0:AFF_DEPTH-1];
    (* ramstyle = "M10K" *) logic [31:0] stg_odd_a  [0:AFF_DEPTH-1];
    (* ramstyle = "M10K" *) logic [31:0] stg_odd_b  [0:AFF_DEPTH-1];

    // Latched inverse-affine coefficients (Q16.16 signed).
    logic signed [31:0] aff_m00_q, aff_m01_q, aff_m10_q, aff_m11_q;
    logic signed [31:0] aff_tx_q,  aff_ty_q;

    // Per-pixel / per-row source-coordinate accumulators (Q16.16 signed,
    // sub-rect-relative). DDA: +m00/+m10 per pixel, +m01/+m11 per row.
    logic signed [31:0] aff_srcx_q,     aff_srcy_q;
    logic signed [31:0] aff_srcx_row_q, aff_srcy_row_q;

    // Load-phase cursors.
    logic [7:0]         aff_load_row_q;   // 0..sh
    logic signed [16:0] aff_load_col_q;   // sub-rect col of the low pixel
                                          // of the current beat (signed:
                                          // starts at -1 on odd lead).
    logic [7:0]         aff_load_beats_q; // beats captured this row
    logic [7:0]         aff_row_beats_q;  // beats expected this row
    logic [31:0]        aff_row_byte_q;   // DDR byte addr of this row's
                                          // first wanted source pixel

    // Sample-phase pipeline registers (set in S_AFF_PIX_MAC, consumed
    // in S_AFF_PIX_FILTER after the BRAM read-latency bubble).
    logic signed [15:0] aff_ix_q, aff_iy_q;
    logic [7:0]         aff_fx8_q, aff_fy8_q;
    logic               aff_t00v_q, aff_t01v_q, aff_t10v_q, aff_t11v_q;

    // Bilinear stage-1 outputs: the two x-interpolated source rows,
    // held one cycle for the y-interpolation in S_AFF_PIX_FILTER2.
    logic [31:0] aff_top_q, aff_bot_q;

    // Registered BRAM read outputs (one per physical copy).
    logic [31:0] ea_q, eb_q, oa_q, ob_q;

    // ---- Combinational sample-phase read addresses -----------------
    // Derived from the registered integer source coords (aff_ix_q /
    // aff_iy_q). Out-of-range taps still produce an in-range index
    // (low bits) — the tap is masked to transparent in S_AFF_PIX_FILTER.
    wire [5:0]  aff_odd_col  = aff_ix_q[6:1];                 // ix>>1
    wire [6:0]  aff_even_col = {1'b0, aff_ix_q[6:1]} + {6'd0, aff_ix_q[0]};
    wire [6:0]  aff_iy0      = aff_iy_q[6:0];
    wire [6:0]  aff_iy1      = aff_iy_q[6:0] + 7'd1;
    wire [12:0] ea_addr = {aff_iy0, aff_even_col[5:0]};
    wire [12:0] eb_addr = {aff_iy1, aff_even_col[5:0]};
    wire [12:0] oa_addr = {aff_iy0, aff_odd_col};
    wire [12:0] ob_addr = {aff_iy1, aff_odd_col};

    // ---- Combinational load-phase bank write decode ----------------
    // One arriving 64-bit beat carries two consecutive sub-rect columns
    // (low 32 = lower x). One is even, one is odd → one write to each
    // bank, with per-bank write-enable masking off out-of-range columns
    // (the odd-lead first beat, and the odd-width tail).
    wire        aff_beat_valid = (state == S_AFF_LOAD_WAIT) & ddram_dout_valid_i;
    wire [31:0] aff_lo_pix = ddram_dout_i[31:0];
    wire [31:0] aff_hi_pix = ddram_dout_i[63:32];
    wire signed [16:0] aff_col_lo = aff_load_col_q;          // low pixel col
    wire signed [16:0] aff_col_hi = aff_load_col_q + 17'sd1; // high pixel col
    wire        aff_lo_in = (aff_col_lo >= 0) && (aff_col_lo < $signed({1'b0, src_w_q}));
    wire        aff_hi_in = (aff_col_hi >= 0) && (aff_col_hi < $signed({1'b0, src_w_q}));
    wire        aff_lo_is_even = (aff_col_lo[0] == 1'b0);
    // Route low/high pixel to even/odd bank by parity of the low col.
    wire [31:0] aff_even_pix = aff_lo_is_even ? aff_lo_pix : aff_hi_pix;
    wire [31:0] aff_odd_pix  = aff_lo_is_even ? aff_hi_pix : aff_lo_pix;
    wire signed [16:0] aff_even_colw = aff_lo_is_even ? aff_col_lo : aff_col_hi;
    wire signed [16:0] aff_odd_colw  = aff_lo_is_even ? aff_col_hi : aff_col_lo;
    wire        aff_even_we = aff_beat_valid & (aff_lo_is_even ? aff_lo_in : aff_hi_in);
    wire        aff_odd_we  = aff_beat_valid & (aff_lo_is_even ? aff_hi_in : aff_lo_in);
    wire [12:0] aff_even_waddr = {aff_load_row_q[6:0], aff_even_colw[6:1]};
    wire [12:0] aff_odd_waddr  = {aff_load_row_q[6:0], aff_odd_colw[6:1]};

    // ---- Staging BRAM: mirrored writes (load) + registered reads ----
    always_ff @(posedge clk) begin
        if (aff_even_we) begin
            stg_even_a[aff_even_waddr] <= aff_even_pix;
            stg_even_b[aff_even_waddr] <= aff_even_pix;
        end
        if (aff_odd_we) begin
            stg_odd_a[aff_odd_waddr] <= aff_odd_pix;
            stg_odd_b[aff_odd_waddr] <= aff_odd_pix;
        end
        ea_q <= stg_even_a[ea_addr];
        eb_q <= stg_even_b[eb_addr];
        oa_q <= stg_odd_a[oa_addr];
        ob_q <= stg_odd_b[ob_addr];
    end

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

    // 8-bit linear interpolation a + (b-a)*t/256, with t a /256
    // fraction. Crucially EXACT at t == 0 (returns a) so integer-aligned
    // samples — e.g. the entire identity transform — are bit-exact; the
    // `a*(255-t)+b*t` form would dim full-intensity channels by 1 LSB.
    // Round-to-nearest (+128) and saturate to [0,255].
    function automatic logic [7:0] lerp8(
        input logic [7:0] a,
        input logic [7:0] b,
        input logic [7:0] t
    );
        logic signed [9:0]  diff;
        logic signed [18:0] prod;
        logic signed [18:0] res;
        diff = $signed({2'b00, b}) - $signed({2'b00, a});
        prod = (diff * $signed({1'b0, t})) + 19'sd128;
        res  = $signed({11'd0, a}) + (prod >>> 8);
        if (res < 0)         return 8'h00;
        else if (res > 255)  return 8'hFF;
        else                 return res[7:0];
    endfunction

    // Separable bilinear of a 2x2 texel quad (premultiplied-alpha safe),
    //   t00=(ix,iy) t01=(ix+1,iy) t10=(ix,iy+1) t11=(ix+1,iy+1)
    // split into two pipeline halves so each carries only ONE `lerp8`
    // (one multiply) of combinational depth. The full 2×2 blend in a
    // single cycle chained two multiplies and missed clk_sys by ~8 ns;
    // the FSM now runs `bilinear_row_x` in S_AFF_PIX_FILTER (registering
    // the two x-interpolated rows) and `bilinear_col_y` the next cycle in
    // S_AFF_PIX_FILTER2. Result is bit-identical to the one-shot form
    // (pack_pixel/ch_* round-trip exactly).

    // Stage 1: x-interpolate one source row. `left`/`right` are the two
    // horizontally-adjacent taps; returns the per-channel lerp at fx.
    function automatic logic [31:0] bilinear_row_x(
        input logic [31:0] left,
        input logic [31:0] right,
        input logic [7:0]  fx
    );
        bilinear_row_x = pack_pixel(
            lerp8(ch_r(left), ch_r(right), fx),
            lerp8(ch_g(left), ch_g(right), fx),
            lerp8(ch_b(left), ch_b(right), fx),
            lerp8(ch_a(left), ch_a(right), fx)
        );
    endfunction

    // Stage 2: y-interpolate the two x-interpolated rows at fy.
    function automatic logic [31:0] bilinear_col_y(
        input logic [31:0] top,
        input logic [31:0] bot,
        input logic [7:0]  fy
    );
        bilinear_col_y = pack_pixel(
            lerp8(ch_r(top), ch_r(bot), fy),
            lerp8(ch_g(top), ch_g(bot), fy),
            lerp8(ch_b(top), ch_b(bot), fy),
            lerp8(ch_a(top), ch_a(bot), fy)
        );
    endfunction

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
                // Multi-beat read from the src's beat-aligned base —
                // [31:3] drops the word offset, and copy_fetch_beats_q
                // carries the extra skid beat when the src starts in
                // the high half. Slave latches addr+burstcnt on the
                // first cycle and streams beats back via dout_valid.
                ddram_addr_o     = src_pixel_byte_addr[31:3];
                ddram_burstcnt_o = {4'd0, copy_fetch_beats_q};
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
                // Issue the next-burst src read (16-pixel tier only).
                // Same skid rules as S_FETCH_SRC_BURST: the [31:3]
                // slice is the aligned base, +1 beat when skewed —
                // skew is row-constant so copy_src_skew_q still holds.
                ddram_addr_o     = prefetch_src_addr_q[31:3];
                ddram_burstcnt_o = 8'd8 + {7'd0, copy_src_skew_q};
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
            // ---- AFFINE: staging load (burst read one source row) ----
            S_AFF_LOAD_FETCH: begin
                ddram_addr_o     = aff_row_byte_q[31:3];
                ddram_burstcnt_o = aff_row_beats_q;
                ddram_be_o       = 8'hFF;
                ddram_rd_o       = 1'b1;
            end
            // ---- AFFINE: per-pixel dst RMW read / write --------------
            S_AFF_FETCH_DST: begin
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = be_for_word(dst_pixel_byte_addr[2]);
                ddram_rd_o       = 1'b1;
            end
            S_AFF_WRITE: begin
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = be_for_word(dst_pixel_byte_addr[2]);
                ddram_din_o      = dst_pixel_byte_addr[2]
                                       ? {pixel_data, 32'd0}
                                       : {32'd0, pixel_data};
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
                // Saturating adds: exact for premultiplied sources
                // (src.ch <= src.a makes overflow impossible), but a
                // per-channel tint can break that invariant — the
                // plain adds here used to wrap to garbage colors
                // instead of clamping.
                inv_a = 8'hFF - ch_a(src);
                r = sat_add8(ch_r(src), mul8(ch_r(dst), inv_a));
                g = sat_add8(ch_g(src), mul8(ch_g(dst), inv_a));
                b = sat_add8(ch_b(src), mul8(ch_b(dst), inv_a));
                a = sat_add8(ch_a(src), mul8(ch_a(dst), inv_a));
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
            fill_blend_q      <= 1'b0;
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
            div_num_x_q       <= '0;
            div_den_x_q       <= '0;
            div_num_y_q       <= '0;
            div_den_y_q       <= '0;
            div_den_x_zero_q  <= 1'b0;
            div_den_y_zero_q  <= 1'b0;
            sox_q             <= '0;
            soy_q             <= '0;
            div_wait_cnt_q    <= '0;
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
            copy_src_skew_q      <= 1'b0;
            copy_fetch_beats_q   <= 4'd0;
            copy_a8_skew_q       <= 3'd0;
            copy_beat_idx_q      <= 4'd0;
            copy_pixel_idx_q     <= 5'd0;
            prefetch_alpha_or_q  <= 8'd0;
            prefetch_alpha_and_q <= 8'hFF;
            prefetch_active_q    <= 1'b0;
            prefetch_ready_q     <= 1'b0;
            prefetch_beat_idx_q  <= 4'd0;
            prefetch_src_addr_q  <= 32'd0;
            aff_m00_q            <= 32'sd0;
            aff_m01_q            <= 32'sd0;
            aff_m10_q            <= 32'sd0;
            aff_m11_q            <= 32'sd0;
            aff_tx_q             <= 32'sd0;
            aff_ty_q             <= 32'sd0;
            aff_srcx_q           <= 32'sd0;
            aff_srcy_q           <= 32'sd0;
            aff_srcx_row_q       <= 32'sd0;
            aff_srcy_row_q       <= 32'sd0;
            aff_load_row_q       <= 8'd0;
            aff_load_col_q       <= 17'sd0;
            aff_load_beats_q     <= 8'd0;
            aff_row_beats_q      <= 8'd0;
            aff_row_byte_q       <= 32'd0;
            aff_ix_q             <= 16'sd0;
            aff_iy_q             <= 16'sd0;
            aff_fx8_q            <= 8'd0;
            aff_fy8_q            <= 8'd0;
            aff_t00v_q           <= 1'b0;
            aff_t01v_q           <= 1'b0;
            aff_t10v_q           <= 1'b0;
            aff_t11v_q           <= 1'b0;
            aff_top_q            <= 32'd0;
            aff_bot_q            <= 32'd0;
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
                automatic logic        p_take_lo;
                automatic logic        p_take_hi;
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
                // Same skid take/drop rules as S_WAIT_SRC_BURST —
                // prefetch always covers a full 16-px burst, and the
                // skew is row-constant so copy_src_skew_q applies.
                p_take_lo = ({prefetch_beat_idx_q, 1'b0} >= {4'd0, copy_src_skew_q})
                          & ({prefetch_beat_idx_q, 1'b0} <
                             (5'd16 + {4'd0, copy_src_skew_q}));
                p_take_hi = ({prefetch_beat_idx_q, 1'b1} <
                             (5'd16 + {4'd0, copy_src_skew_q}));
                p_lo_idx = {prefetch_beat_idx_q, 1'b0} - {4'd0, copy_src_skew_q};
                p_hi_idx = {prefetch_beat_idx_q, 1'b1} - {4'd0, copy_src_skew_q};
                if (p_take_lo) prefetch_buf[p_lo_idx[3:0]] <= tinted_lo;
                if (p_take_hi) prefetch_buf[p_hi_idx[3:0]] <= tinted_hi;
                al = ch_a(tinted_lo);
                ah = ch_a(tinted_hi);
                prefetch_alpha_or_q  <= prefetch_alpha_or_q
                                      | (p_take_lo ? al : 8'h00)
                                      | (p_take_hi ? ah : 8'h00);
                prefetch_alpha_and_q <= prefetch_alpha_and_q
                                      & (p_take_lo ? al : 8'hFF)
                                      & (p_take_hi ? ah : 8'hFF);
                prefetch_last = (prefetch_beat_idx_q + 4'd1
                                 == 4'd8 + {3'd0, copy_src_skew_q});
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
                    // nested blocks). step_x/step_y values come
                    // from the pipelined lpm_divide and land in
                    // S_DIV_WAIT.
                    automatic logic        do_scale;
                    automatic logic        is_aff;

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
                    // Cleared at op accept; only the blended-FILL
                    // burst dispatch sets it (COPY ops share the RMW
                    // states and must see it low).
                    fill_blend_q <= 1'b0;
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
                    is_aff   = (mode_i == MODE_AFFINE);
                    // AFFINE keeps src_x/y_q at the sub-rect origin
                    // (sx,sy) — the load reads from there and the
                    // left/top clip offset (sox/soy) is folded into the
                    // accumulator seed in S_AFF_SETUP, like scale mode.
                    src_x_q      <= src_x_i + ((do_scale || is_aff) ? 16'd0 : sox);
                    src_y_q      <= src_y_i + ((do_scale || is_aff) ? 16'd0 : soy);
                    src_w_q      <= src_w_i;
                    src_h_q      <= src_h_i;
                    src_addr_q   <= src_addr_i;
                    src_pitch_q  <= src_pitch_i;
                    cur_y_off    <= '0;

                    // --- Scale-mode setup --------------------------
                    // Active only on COPY when src and dst dimensions
                    // differ. Step = (src_dim / dst_dim) in Q16.16.
                    // The actual division is pipelined (8 stages,
                    // see lpm_divide instances at module-top); on
                    // entry to S_DIV_WAIT we latch the numerator /
                    // denominator inputs, the FSM idles for 8 cycles
                    // for the result, then S_DIV_WAIT's exit cycle
                    // computes src_x_init/acc from sox·step + step/2
                    // (bias for center-of-pixel sampling per
                    // PROTOCOL.md §7.4:
                    //   src_x = sx + floor((i + 0.5) * sw / dw)).
                    // The 8-cycle latency is negligible vs the
                    // thousands of cycles per scaled blit; non-
                    // scaled blits skip the wait entirely.
                    if (is_aff) begin
                        // Latch the inverse matrix; carry the clip
                        // offset into S_AFF_SETUP (reusing sox_q/soy_q)
                        // where it seeds the source accumulators.
                        scale_mode_q <= 1'b0;
                        aff_m00_q    <= $signed(aff_m00_i);
                        aff_m01_q    <= $signed(aff_m01_i);
                        aff_m10_q    <= $signed(aff_m10_i);
                        aff_m11_q    <= $signed(aff_m11_i);
                        aff_tx_q     <= $signed(aff_tx_i);
                        aff_ty_q     <= $signed(aff_ty_i);
                        sox_q        <= sox;
                        soy_q        <= soy;
                        state        <= S_AFF_SETUP;
                    end else if (do_scale) begin
                        scale_mode_q     <= 1'b1;
                        div_num_x_q      <= {src_w_i, 16'd0};
                        div_den_x_q      <= dst_w_i;
                        div_num_y_q      <= {src_h_i, 16'd0};
                        div_den_y_q      <= dst_h_i;
                        div_den_x_zero_q <= (dst_w_i == 16'd0);
                        div_den_y_zero_q <= (dst_h_i == 16'd0);
                        sox_q            <= sox;
                        soy_q            <= soy;
                        div_wait_cnt_q   <= 4'd0;
                        state            <= S_DIV_WAIT;
                    end else begin
                        scale_mode_q <= 1'b0;
                        src_step_x_q <= 32'h0001_0000;
                        src_step_y_q <= 32'h0001_0000;
                        src_x_init_q <= 32'd0;
                        src_y_init_q <= 32'd0;
                        src_x_acc_q  <= 32'd0;
                        src_y_acc_q  <= 32'd0;
                        // If the rect is fully clipped (eff_w == 0
                        // or eff_h == 0), S_ROW_INIT immediately
                        // finds cur_y_off == dst_h_q == 0 and falls
                        // to S_DONE.
                        state        <= S_ROW_INIT;
                    end
                end

                S_DIV_WAIT: begin
                    // 8-cycle latency hold for the pipelined lpm_divide.
                    // On the last wait cycle, the quotient is valid
                    // and we latch step + initial-accumulator state.
                    // dst==0 corner cases (degenerate fully-clipped
                    // rects) reuse the 1:1 step of 1.0 since the
                    // divider's quotient is undefined for that case
                    // and S_ROW_INIT will short-circuit to S_DONE
                    // anyway via cur_y_off == 0 == dst_h_q.
                    if (div_wait_cnt_q == 4'(DIV_PIPELINE_STAGES)) begin
                        automatic logic [31:0] step_x_eff;
                        automatic logic [31:0] step_y_eff;
                        step_x_eff = div_den_x_zero_q ? 32'h0001_0000 : div_quot_x;
                        step_y_eff = div_den_y_zero_q ? 32'h0001_0000 : div_quot_y;
                        src_step_x_q <= step_x_eff;
                        src_step_y_q <= step_y_eff;
                        src_x_init_q <= ({16'd0, sox_q} * step_x_eff) + (step_x_eff >> 1);
                        src_y_init_q <= ({16'd0, soy_q} * step_y_eff) + (step_y_eff >> 1);
                        src_x_acc_q  <= ({16'd0, sox_q} * step_x_eff) + (step_x_eff >> 1);
                        src_y_acc_q  <= ({16'd0, soy_q} * step_y_eff) + (step_y_eff >> 1);
                        state        <= S_ROW_INIT;
                    end else begin
                        div_wait_cnt_q <= div_wait_cnt_q + 4'd1;
                    end
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
                        // dst needs even-pixel (8-byte) alignment for
                        // full-BE write beats; src (RGBA or A8) starts
                        // anywhere thanks to the capture-time skid.
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
                        automatic logic        rgba;
                        automatic logic        dst_al;
                        automatic logic        s_skew;
                        automatic logic [3:0]  blen;
                        automatic logic        prefetch_hit;
                        remaining_copy = dst_w_q - cur_x;
                        rgba   = (format_q == FMT_RGBA);
                        // dst needs only 8-byte (even-pixel) alignment
                        // for full-BE write beats; src can start on
                        // ANY pixel thanks to the one-word skid (see
                        // copy_src_skew_q). Odd dst starts self-align
                        // after one per-pixel iteration.
                        dst_al = (dst_pixel_byte_addr[2:0] == 3'd0);
                        s_skew = src_pixel_byte_addr[2];
                        blen   = (remaining_copy >= 16'd16) ? 4'd8
                                                            : remaining_copy[4:1];
                        prefetch_hit = prefetch_ready_q
                                     & (prefetch_src_addr_q == src_pixel_byte_addr);
                        if (rgba && (remaining_copy >= 16'd16)
                            && dst_al && prefetch_hit) begin
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
                            copy_src_skew_q    <= s_skew;
                            copy_fetch_beats_q <= 4'd8 + {3'd0, s_skew};
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
                        end else if (dst_al && (remaining_copy >= 16'd2)) begin
                            // Generalized burst: 1..8 dst beats
                            // (2..16 px), src at any pixel offset via
                            // the skid. Replaces the old ladder that
                            // required src and dst to share (burst*4)-
                            // byte alignment — which any 1-px relative
                            // offset (e.g. a sliding dst) broke for the
                            // whole row, collapsing to the per-pixel
                            // path and forcing the host's 16-px text
                            // snapping.
                            copy_burst_len_q   <= blen;
                            if (rgba) begin
                                copy_src_skew_q    <= s_skew;
                                copy_a8_skew_q     <= 3'd0;
                                copy_fetch_beats_q <= blen + {3'd0, s_skew};
                            end else begin
                                // A8: 2*blen source BYTES; one beat
                                // carries 8 pixels. Fetch from the
                                // beat-aligned base, ceil((run +
                                // byte_skew)/8) beats (1..3 — an A8
                                // 16-px chunk is 2 payload beats vs
                                // RGBA's 8; this tier is what turns
                                // glyph blits from one bus round-trip
                                // PER PIXEL into ~3 per 16 pixels).
                                copy_src_skew_q    <= 1'b0;
                                copy_a8_skew_q     <= src_pixel_byte_addr[2:0];
                                copy_fetch_beats_q <= 4'(
                                    ({2'd0, blen, 1'b0}
                                     + {4'd0, src_pixel_byte_addr[2:0]}
                                     + 7'd7) >> 3);
                            end
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
                        // FILL non-Opaque needs RMW. Burst tier when
                        // the dst run is 2-px (beat) aligned: ride the
                        // COPY RMW states with the constant colour as
                        // the blend source (fill_blend_q pre-loads it
                        // into src_buf during the dst capture). Odd
                        // lead/trail pixels fall back to the per-pixel
                        // path; the dispatch re-evaluates per burst.
                        automatic logic [15:0] remaining_f;
                        automatic logic        aligned_f;
                        automatic logic [15:0] beats_f;
                        remaining_f = dst_w_q - cur_x;
                        aligned_f   = ~(dst_x_q[0] ^ cur_x[0]);
                        beats_f     = remaining_f >> 1;
                        if (aligned_f && (remaining_f >= 16'd2)) begin
                            copy_burst_len_q <= (beats_f > 16'd8)
                                                    ? 4'd8
                                                    : beats_f[3:0];
                            copy_beat_idx_q  <= 4'd0;
                            copy_pixel_idx_q <= 5'd0;
                            fill_blend_q     <= 1'b1;
                            state            <= S_FETCH_DST_BURST;
                        end else begin
                            src_pixel_q <= color_q;
                            state       <= S_FETCH_DST;
                        end
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
                    // RGBA: each beat holds two pixels (word skid, see
                    // dispatch). A8: each beat holds EIGHT pixels (byte
                    // skid) which are tint-expanded on capture. Both
                    // paths land tinted BGRA in src_buf and fold the
                    // alpha OR/AND summaries over accepted pixels only,
                    // so the shared last-beat fast-path decision below
                    // is format-agnostic.
                    automatic logic [31:0] src_lo;
                    automatic logic [31:0] src_hi;
                    automatic logic [31:0] computed_lo;
                    automatic logic [31:0] computed_hi;
                    automatic logic [4:0]  pix_lo_idx;
                    automatic logic [4:0]  pix_hi_idx;
                    automatic logic        take_lo;
                    automatic logic        take_hi;
                    automatic logic [7:0]  alpha_lo;
                    automatic logic [7:0]  alpha_hi;
                    automatic logic [7:0]  next_alpha_or;
                    automatic logic [7:0]  next_alpha_and;
                    automatic logic        is_last_beat;
                    automatic logic [6:0]  a8_wbase;
                    automatic logic [6:0]  a8_lim;
                    automatic logic [6:0]  a8_widx;
                    automatic logic [7:0]  a8_a;
                    next_alpha_or  = alpha_or_q;
                    next_alpha_and = alpha_and_q;
                    if (format_q == FMT_A8) begin
                        a8_wbase = {copy_beat_idx_q, 3'b000};
                        a8_lim   = {2'd0, copy_burst_len_q, 1'b0}
                                 + {4'd0, copy_a8_skew_q};
                        for (int k = 0; k < 8; k++) begin
                            a8_widx = a8_wbase + 7'(k);
                            a8_a    = ddram_dout_i[k*8 +: 8];
                            if ((a8_widx >= {4'd0, copy_a8_skew_q})
                                && (a8_widx < a8_lim)) begin
                                src_buf[4'(a8_widx - {4'd0, copy_a8_skew_q})]
                                    <= pack_pixel(
                                        mul8(ch_r(tint_color_q), a8_a),
                                        mul8(ch_g(tint_color_q), a8_a),
                                        mul8(ch_b(tint_color_q), a8_a),
                                        a8_a
                                    );
                                next_alpha_or  = next_alpha_or  | a8_a;
                                next_alpha_and = next_alpha_and & a8_a;
                            end
                        end
                    end else begin
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
                    // Skid mapping: arriving word w (= beat*2 + half)
                    // carries pixel (w - skew). The leading word of a
                    // skewed burst and the trailing word of its last
                    // beat are over-read padding — not written, and
                    // excluded from the alpha summaries.
                    take_lo = ({copy_beat_idx_q, 1'b0} >= {4'd0, copy_src_skew_q})
                            & ({copy_beat_idx_q, 1'b0} <
                               ({copy_burst_len_q, 1'b0} + {4'd0, copy_src_skew_q}));
                    take_hi = ({copy_beat_idx_q, 1'b1} <
                               ({copy_burst_len_q, 1'b0} + {4'd0, copy_src_skew_q}));
                    pix_lo_idx = {copy_beat_idx_q, 1'b0} - {4'd0, copy_src_skew_q};
                    pix_hi_idx = {copy_beat_idx_q, 1'b1} - {4'd0, copy_src_skew_q};
                    if (take_lo) src_buf[pix_lo_idx[3:0]] <= computed_lo;
                    if (take_hi) src_buf[pix_hi_idx[3:0]] <= computed_hi;

                    alpha_lo = ch_a(computed_lo);
                    alpha_hi = ch_a(computed_hi);
                    next_alpha_or  = next_alpha_or
                                   | (take_lo ? alpha_lo : 8'h00)
                                   | (take_hi ? alpha_hi : 8'h00);
                    next_alpha_and = next_alpha_and
                                   & (take_lo ? alpha_lo : 8'hFF)
                                   & (take_hi ? alpha_hi : 8'hFF);
                    end
                    alpha_or_q  <= next_alpha_or;
                    alpha_and_q <= next_alpha_and;

                    is_last_beat = (copy_beat_idx_q + 4'd1 == copy_fetch_beats_q);
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
                    // Blended FILL rides these states without a src
                    // fetch: stage the constant colour as the blend
                    // source while the port is idle (see fill_blend_q).
                    if (fill_blend_q) begin
                        src_buf[pix_lo_idx[3:0]] <= color_q;
                        src_buf[pix_hi_idx[3:0]] <= color_q;
                    end

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
                                          // Prefetch reads SRC for the
                                          // next COPY burst — a fill
                                          // has no src to prefetch.
                                          & ~fill_blend_q
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

                // ================= AFFINE path ====================

                S_AFF_SETUP: begin
                    // Safety net for the on-chip staging cap and for
                    // fully-clipped rects (the fetcher already rejects
                    // oversize sources, but never write past the banks).
                    if ((src_w_q > 16'd128) || (src_h_q > 16'd128)
                        || (src_w_q == 16'd0) || (src_h_q == 16'd0)
                        || (dst_w_q == 16'd0) || (dst_h_q == 16'd0)) begin
                        // src_w==0 would issue a ZERO-LENGTH DDR burst
                        // (aff_row_beats = 0): undefined on the f2h
                        // slave and it desyncs the shared-port
                        // arbiter's outstanding-beat counter.
                        state <= S_DONE;
                    end else begin
                        // Seed the source accumulators at the clipped
                        // top-left (ox=sox, oy=soy). The products fold
                        // the left/top clip offset into tx/ty; with no
                        // clip (sox=soy=0) the seed is just tx/ty.
                        automatic logic signed [16:0] soxs;
                        automatic logic signed [16:0] soys;
                        soxs = $signed({1'b0, sox_q});
                        soys = $signed({1'b0, soy_q});
                        aff_srcx_row_q <= aff_tx_q
                                        + 32'($signed(aff_m00_q) * soxs)
                                        + 32'($signed(aff_m01_q) * soys);
                        aff_srcy_row_q <= aff_ty_q
                                        + 32'($signed(aff_m10_q) * soxs)
                                        + 32'($signed(aff_m11_q) * soys);
                        aff_load_row_q <= 8'd0;
                        state          <= S_AFF_LOAD_ROW;
                    end
                end

                S_AFF_LOAD_ROW: begin
                    if (aff_load_row_q == src_h_q[7:0]) begin
                        // All source rows staged → start the draw walk.
                        state <= S_AFF_ROW;
                    end else begin
                        automatic logic [31:0] rb;
                        automatic logic        lead;
                        automatic logic [8:0]  pix;
                        rb = src_addr_q
                           + (({8'd0, src_y_q} + {16'd0, aff_load_row_q}) * src_pitch_q)
                           + ({16'd0, src_x_q} <<< 2);
                        lead = rb[2];
                        pix  = {8'd0, lead} + {1'b0, src_w_q[7:0]};
                        aff_row_byte_q   <= rb;
                        aff_row_beats_q  <= (pix + 9'd1) >> 1;  // ceil(pix/2)
                        aff_load_beats_q <= 8'd0;
                        // Low pixel of beat 0 is col 0 (aligned) or col
                        // -1 (the discarded pixel before, when the first
                        // wanted pixel is the high half of the beat).
                        aff_load_col_q   <= lead ? -17'sd1 : 17'sd0;
                        state            <= S_AFF_LOAD_FETCH;
                    end
                end

                S_AFF_LOAD_FETCH: if (~ddram_busy_i) begin
                    state <= S_AFF_LOAD_WAIT;
                end

                S_AFF_LOAD_WAIT: if (ddram_dout_valid_i & ~prefetch_active_q) begin
                    // The staging-bank writes for this beat happen in the
                    // dedicated RAM always_ff (using aff_load_col_q). Here
                    // we just advance the cursors.
                    aff_load_col_q <= aff_load_col_q + 17'sd2;
                    if (aff_load_beats_q + 8'd1 == aff_row_beats_q) begin
                        aff_load_beats_q <= 8'd0;
                        aff_load_row_q   <= aff_load_row_q + 8'd1;
                        state            <= S_AFF_LOAD_ROW;
                    end else begin
                        aff_load_beats_q <= aff_load_beats_q + 8'd1;
                    end
                end

                S_AFF_ROW: begin
                    if (cur_y_off == dst_h_q) begin
                        state <= S_DONE;
                    end else begin
                        dst_row_byte_addr <= target_base_i
                            + ({16'd0, (dst_y_q + cur_y_off)} * target_pitch_i)
                            + ({16'd0, dst_x_q} <<< 2);
                        cur_x      <= '0;
                        aff_srcx_q <= aff_srcx_row_q;
                        aff_srcy_q <= aff_srcy_row_q;
                        state      <= S_AFF_PIX_MAC;
                    end
                end

                S_AFF_PIX_MAC: begin
                    if (cur_x == dst_w_q) begin
                        // End of row: step the per-row accumulators by
                        // one dst-y (add the m01/m11 column of the
                        // inverse matrix) and move to the next row.
                        cur_y_off      <= cur_y_off + 16'd1;
                        aff_srcx_row_q <= aff_srcx_row_q + aff_m01_q;
                        aff_srcy_row_q <= aff_srcy_row_q + aff_m11_q;
                        state          <= S_AFF_ROW;
                    end else begin
                        // Register integer/frac parts and tap-valid
                        // flags. Bank read addresses (ea_addr…) are
                        // combinational from aff_ix_q/aff_iy_q, so they
                        // present to the BRAM during S_AFF_PIX_READ.
                        automatic logic signed [15:0] ixv;
                        automatic logic signed [15:0] iyv;
                        automatic logic signed [16:0] sww;
                        automatic logic signed [16:0] shh;
                        automatic logic               x0in, x1in, y0in, y1in;
                        ixv = aff_srcx_q[31:16];
                        iyv = aff_srcy_q[31:16];
                        // 17-bit positive signed bounds (sw/sh ≤ 128).
                        sww = $signed({1'b0, src_w_q});
                        shh = $signed({1'b0, src_h_q});
                        // Signed compares: ixv/iyv are signed [15:0],
                        // sww/shh signed [16:0], so each comparison
                        // sign-extends ixv/iyv to 17 bits.
                        x0in = (ixv >= 0)       && (ixv           < sww);
                        x1in = (ixv >= -16'sd1) && ((ixv + 16'sd1) < sww);
                        y0in = (iyv >= 0)       && (iyv           < shh);
                        y1in = (iyv >= -16'sd1) && ((iyv + 16'sd1) < shh);
                        aff_ix_q   <= ixv;
                        aff_iy_q   <= iyv;
                        aff_fx8_q  <= aff_srcx_q[15:8];
                        aff_fy8_q  <= aff_srcy_q[15:8];
                        aff_t00v_q <= x0in & y0in;
                        aff_t01v_q <= x1in & y0in;
                        aff_t10v_q <= x0in & y1in;
                        aff_t11v_q <= x1in & y1in;
                        state      <= S_AFF_PIX_READ;
                    end
                end

                S_AFF_PIX_READ: begin
                    // 1-cycle BRAM read-latency bubble; ea_q/eb_q/oa_q/
                    // ob_q latch at the edge into S_AFF_PIX_FILTER.
                    state <= S_AFF_PIX_FILTER;
                end

                S_AFF_PIX_FILTER: begin
                    automatic logic [31:0] t00, t01, t10, t11;
                    // Parity mux: aff_ix_q[0] picks which bank is the
                    // left column of the quad (even-x bank vs odd-x).
                    t00 = aff_ix_q[0] ? oa_q : ea_q;
                    t01 = aff_ix_q[0] ? ea_q : oa_q;
                    t10 = aff_ix_q[0] ? ob_q : eb_q;
                    t11 = aff_ix_q[0] ? eb_q : ob_q;
                    if (!aff_t00v_q) t00 = 32'd0;
                    if (!aff_t01v_q) t01 = 32'd0;
                    if (!aff_t10v_q) t10 = 32'd0;
                    if (!aff_t11v_q) t11 = 32'd0;
                    // x-interpolate both rows now; y-interpolation runs
                    // next cycle so each stage holds only one lerp8.
                    aff_top_q <= bilinear_row_x(t00, t01, aff_fx8_q);
                    aff_bot_q <= bilinear_row_x(t10, t11, aff_fx8_q);
                    state     <= S_AFF_PIX_FILTER2;
                end

                S_AFF_PIX_FILTER2: begin
                    automatic logic [31:0] srcpix;
                    automatic logic [7:0]  sa;
                    srcpix = bilinear_col_y(aff_top_q, aff_bot_q, aff_fy8_q);
                    sa = ch_a(srcpix);
                    // Source is premultiplied; same blend fast paths as
                    // the COPY path.
                    if (blend_q == BLEND_OPAQUE) begin
                        pixel_data <= srcpix;
                        state      <= S_AFF_WRITE;
                    end else if ((blend_q == BLEND_SRCALPHA) && (sa == 8'hFF)) begin
                        pixel_data <= srcpix;
                        state      <= S_AFF_WRITE;
                    end else if ((blend_q == BLEND_SRCALPHA) && (sa == 8'h00)) begin
                        state <= S_AFF_NEXT;   // fully transparent → skip
                    end else begin
                        src_pixel_q <= srcpix;
                        state       <= S_AFF_FETCH_DST;
                    end
                end

                S_AFF_FETCH_DST: if (~ddram_busy_i) begin
                    state <= S_AFF_WAIT_DST;
                end

                S_AFF_WAIT_DST: if (ddram_dout_valid_i & ~prefetch_active_q) begin
                    dst_pixel_q <= pick_word(ddram_dout_i, dst_pixel_byte_addr[2]);
                    state       <= S_AFF_BLEND;
                end

                S_AFF_BLEND: begin
                    pixel_data <= blend_pixel(src_pixel_q, dst_pixel_q, blend_q);
                    state      <= S_AFF_WRITE;
                end

                S_AFF_WRITE: if (~ddram_busy_i) begin
                    state <= S_AFF_NEXT;
                end

                S_AFF_NEXT: begin
                    cur_x      <= cur_x + 16'd1;
                    aff_srcx_q <= aff_srcx_q + aff_m00_q;
                    aff_srcy_q <= aff_srcy_q + aff_m10_q;
                    state      <= S_AFF_PIX_MAC;
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
