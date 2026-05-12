//============================================================================
//
//  Compositor scanout — Phase 2a step 3 (solid-colour layer walker).
//
//  Drives VGA_R/G/B/HS/VS/DE directly from a host-managed layer table
//  cached on-chip by `layer_dma` + `layer_cache`. Each scanline:
//
//    1. During HBlank, `scanline_filter` walks the cached layers and
//       picks those whose `dst` rect covers the *next* scanline,
//       writing them into a small `active` array.
//    2. During active scanout, this module's per-pixel painter walks
//       `active` in parallel: for each x, the topmost (highest-index)
//       hit's `color` is output; if no hit, the pixel is black.
//
//  Slot index is z-order (PROTOCOL.md §11.1): slot 0 = back, slot
//  N = front. The painter therefore wants the *latest* match in
//  `active`, which is achieved by an always_comb for-loop that
//  scans low-to-high with last-assignment-wins semantics.
//
//  Phase 2a only honours solid-colour layers (`tex_id == 0xFFFF`).
//  Textured layers are dropped by `scanline_filter`; they light up
//  in Phase 2b.
//
//  Timing: native 1920×1080, 100 MHz pixel clock (= clk_video,
//  separate from clk_sys via a second PLL output). ASCAL still
//  accepts arbitrary core-side timing; producing pixels at native
//  resolution lets text/UI keep their pixel-perfect crispness on a
//  1080p HDMI sink.
//
//    H: 1920 active + 1500 blank = 3420 total
//    V: 1080 active +   20 blank = 1100 total
//    => 100 MHz / (3420 × 1100) ≈ 26.6 Hz
//
//  HBlank widened from 600 -> 1500 in Phase 2c step 3 to fit the
//  dispatcher's worst-case 4-textured pass. Future optimisation:
//  descriptor caching in texture_unit (same-tex_id glyphs share one
//  descriptor fetch) would let us shrink HBlank back to ~600 for
//  text-heavy scenes and push fps back to ~36.
//
//  HBlank is set just wide enough to fit the worst-case `scanline_filter`
//  walk of all 256 layer slots (count + 2 = 258 cycles) plus a small
//  margin. VBlank just needs to be > 1 scanline so the once-per-frame
//  `layer_dma` pass fits comfortably.
//
//  HSYNC and VSYNC are positive-polarity.
//
//============================================================================

module compositor #(
    // Reduced from 16 to 8 to keep the painter's per-pixel scans
    // under the 10 ns clk_video budget. 8 active layers per scanline
    // is enough for a typical UI scene; if we ever need more, the
    // first move is pipelining the scan into stages of 8 (each stage
    // half the LUT depth), not bumping this further.
    parameter int MAX_ACTIVE = 8,
    // Per-scanline limit on textured layers. Each one consumes a
    // dedicated line buffer and a sequential texture_unit pass.
    // Excess textured layers (beyond the first 4 found in active
    // list order) are silently dropped.
    parameter int MAX_TEXTURED = 4
) (
    input  logic        clk,        // CLK_VIDEO == pixel clock (100 MHz)
    input  logic        rst_n,

    // CE_PIXEL for the framework — always 1 (we run at pixel rate).
    output logic        ce_pix,

    // Pixel data + timing for the framework's ASCAL.
    output logic [7:0]  r,
    output logic [7:0]  g,
    output logic [7:0]  b,
    output logic        hsync,
    output logic        vsync,
    output logic        hblank,
    output logic        vblank,

    // Layer-cache read port. The compositor owns this; it walks the
    // cache during each HBlank to build the active list for the next
    // scanline.
    output logic [7:0]   cache_slot_o,
    input  logic [255:0] cache_data_i,

    // Count from the host (LAYER_COMMIT.count). Sampled at each
    // HBlank start; 0 means "no layers", and the painter emits black.
    input  logic [8:0]   layer_count_i,

    // Texture sampler interface (Phase 2c step 3). The compositor's
    // dispatcher state machine walks the active list during HBlank,
    // fires up to MAX_TEXTURED kicks (one per textured slot), and
    // waits for each texture_unit pass to complete before the next.
    // `tex_buffer_sel_o` selects which of the 4 line buffers the
    // current pass writes into. All kick params + buffer_sel are
    // expected to be held stable while `tex_kick_o` is high.
    output logic        tex_kick_o,
    output logic [15:0] tex_id_o,
    output logic [15:0] tex_src_x_o,
    output logic [15:0] tex_ty_o,
    output logic [11:0] tex_dst_w_o,
    // Tint colour for A8 textures (= layer.color). Ignored by
    // texture_unit when format = BGRA8888.
    output logic [31:0] tex_tint_color_o,
    output logic [1:0]  tex_buffer_sel_o,

    // texture_unit busy signal, sync'd into the clk_video domain by
    // menu_core. The dispatcher uses this for handshaking — wait
    // for busy to rise (kick accepted) then fall (pass complete).
    input  logic        tex_unit_busy_sync_i,

    // Line buffer read ports — 4 ports, one per buffer slot. The
    // painter issues 4 reads in parallel each cycle (one per slot's
    // own dst_x_lo offset) and selects the data from whichever
    // slot's topmost-textured covers the current pixel.
    output logic [9:0]  line_buf_addr_o [MAX_TEXTURED-1:0],
    input  logic [63:0] line_buf_data_i [MAX_TEXTURED-1:0]
);

    // ---- Timing constants (1920×1080, 100 MHz pixel clock).
    // HBlank widened to 1500 cycles in Phase 2c step 3 to fit the
    // dispatcher's worst case (4 textured layers × ~300 cycles each
    // for moderately-sized BGRA + filter 260 + walk + margin).
    // VBlank = 20 lines (unchanged; layer_dma fits comfortably).
    // fps = 100 MHz / (3420 × 1100) ≈ 26.6 Hz.
    localparam int H_ACTIVE = 1920;
    localparam int H_FP     = 60;
    localparam int H_SYNC   = 40;
    localparam int H_BP     = 1400;
    localparam int H_TOTAL  = H_ACTIVE + H_FP + H_SYNC + H_BP; // 3420

    localparam int V_ACTIVE = 1080;
    localparam int V_FP     = 4;
    localparam int V_SYNC   = 4;
    localparam int V_BP     = 12;
    localparam int V_TOTAL  = V_ACTIVE + V_FP + V_SYNC + V_BP; // 1100

    logic [11:0] hcount;
    logic [11:0] vcount;

    assign ce_pix = 1'b1;

    // ---- Counter advance ---------------------------------------------
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            hcount <= 12'd0;
            vcount <= 12'd0;
        end else begin
            if (hcount == H_TOTAL - 1) begin
                hcount <= 12'd0;
                if (vcount == V_TOTAL - 1) begin
                    vcount <= 12'd0;
                end else begin
                    vcount <= vcount + 12'd1;
                end
            end else begin
                hcount <= hcount + 12'd1;
            end
        end
    end

    // ---- Sync / DE generation ----------------------------------------
    wire h_in_sync = (hcount >= H_ACTIVE + H_FP)
                  && (hcount <  H_ACTIVE + H_FP + H_SYNC);
    wire v_in_sync = (vcount >= V_ACTIVE + V_FP)
                  && (vcount <  V_ACTIVE + V_FP + V_SYNC);
    wire h_active  = (hcount < H_ACTIVE);
    wire v_active  = (vcount < V_ACTIVE);

    // ---- HBlank edge → kick filter ------------------------------------
    // Trigger one cycle after hcount == H_ACTIVE-1 transitions to
    // hcount == H_ACTIVE (i.e. exactly when h_active falls).
    logic prev_h_active;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) prev_h_active <= 1'b0;
        else        prev_h_active <= h_active;
    end
    wire build_start = prev_h_active & ~h_active;

    // y_next wraps at V_TOTAL so HBlank of the last line builds the
    // first line of the next frame.
    wire [11:0] y_next = (vcount == V_TOTAL - 1) ? 12'd0 : vcount + 12'd1;

    // ---- Scanline filter ---------------------------------------------
    logic [4:0]                 active_count;
    logic signed [16:0]         active_dst_x_lo [MAX_ACTIVE-1:0];
    logic signed [17:0]         active_dst_x_hi [MAX_ACTIVE-1:0];
    logic [31:0]                active_color    [MAX_ACTIVE-1:0];
    logic [15:0]                active_tex_id   [MAX_ACTIVE-1:0];
    logic [15:0]                active_src_x    [MAX_ACTIVE-1:0];
    logic [15:0]                active_ty       [MAX_ACTIVE-1:0];

    scanline_filter #(.MAX_ACTIVE(MAX_ACTIVE)) u_filter (
        .clk              (clk),
        .rst_n            (rst_n),
        .start_i          (build_start),
        .y_next_i         (y_next),
        .layer_count_i    (layer_count_i),
        .cache_slot_o     (cache_slot_o),
        .cache_data_i     (cache_data_i),
        .active_count_o   (active_count),
        .active_dst_x_lo_o(active_dst_x_lo),
        .active_dst_x_hi_o(active_dst_x_hi),
        .active_color_o   (active_color),
        .active_tex_id_o  (active_tex_id),
        .active_src_x_o   (active_src_x),
        .active_ty_o      (active_ty)
    );

    // ---- Multi-textured dispatcher ----------------------------------
    // After the filter completes, scan the active list and fire
    // texture_unit kicks for up to MAX_TEXTURED textured slots
    // (each kick fills a different line buffer). Z-order is
    // preserved: textured slots are processed in active-list order
    // (slot 0 first = lowest z), and the painter walks all of them
    // per pixel to pick the topmost match.
    //
    // Handshake: tex_kick_o is held high until tex_unit_busy_sync_i
    // goes high (kick accepted by texture_unit on clk_sys via the
    // 2-flop sync); then dropped, and we wait for busy to go low
    // again (texture_unit done) before advancing to the next slot.

    typedef enum logic [2:0] {
        D_IDLE,
        D_WAIT_FILTER,
        D_WALK,
        D_KICK_WAIT_BUSY_HI,
        D_KICK_WAIT_BUSY_LO
    } disp_state_t;

    disp_state_t disp_state;
    logic [9:0]  disp_wait_counter;
    logic [3:0]  disp_walk_i;          // 0..MAX_ACTIVE
    logic [2:0]  disp_buf_count;       // 0..MAX_TEXTURED
    logic        kick_q;
    logic [1:0]  buf_sel_q;
    logic [4:0]  active_for_buf [MAX_TEXTURED-1:0]; // which active slot per buffer
    // Reverse map for the painter: 0..MAX_TEXTURED for "this active
    // slot maps to buffer K"; MAX_TEXTURED (= 4) is the "no buffer"
    // sentinel.
    logic [2:0]  buffer_for_active [MAX_ACTIVE-1:0];

    // Kick params indexed by the current active slot being dispatched.
    wire [2:0] cur_active = disp_walk_i[2:0];

    assign tex_kick_o       = kick_q;
    assign tex_buffer_sel_o = buf_sel_q;
    assign tex_id_o         = active_tex_id  [cur_active];
    assign tex_src_x_o      = active_src_x   [cur_active];
    assign tex_ty_o         = active_ty      [cur_active];
    assign tex_tint_color_o = active_color   [cur_active];
    assign tex_dst_w_o      = active_dst_x_hi[cur_active][11:0]
                            - active_dst_x_lo[cur_active][11:0];

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            disp_state        <= D_IDLE;
            disp_wait_counter <= 10'd0;
            disp_walk_i       <= 4'd0;
            disp_buf_count    <= 3'd0;
            kick_q            <= 1'b0;
            buf_sel_q         <= 2'd0;
            for (int i = 0; i < MAX_TEXTURED; i++) active_for_buf[i] <= 5'h1F;
            for (int i = 0; i < MAX_ACTIVE; i++)   buffer_for_active[i] <= 3'd4;
        end else begin
            unique case (disp_state)
                D_IDLE: begin
                    if (build_start) begin
                        disp_state        <= D_WAIT_FILTER;
                        disp_wait_counter <= 10'd0;
                        disp_walk_i       <= 4'd0;
                        disp_buf_count    <= 3'd0;
                        kick_q            <= 1'b0;
                        // Reset mappings — gets refilled each scanline.
                        for (int i = 0; i < MAX_TEXTURED; i++) active_for_buf[i] <= 5'h1F;
                        for (int i = 0; i < MAX_ACTIVE; i++)   buffer_for_active[i] <= 3'd4;
                    end
                end

                D_WAIT_FILTER: begin
                    // scanline_filter takes count+2 cycles for the worst
                    // case (count = MAX_LAYER_COUNT = 256). Wait 270.
                    disp_wait_counter <= disp_wait_counter + 10'd1;
                    if (disp_wait_counter >= 10'd270) begin
                        disp_state <= D_WALK;
                    end
                end

                D_WALK: begin
                    if (disp_walk_i >= MAX_ACTIVE[3:0]
                        || disp_walk_i[3:0] >= active_count[3:0]
                        || disp_buf_count >= MAX_TEXTURED[2:0]) begin
                        disp_state <= D_IDLE;
                    end else if (active_tex_id[cur_active] != 16'hFFFF) begin
                        // Textured — fire kick with this buffer index.
                        buf_sel_q                  <= disp_buf_count[1:0];
                        active_for_buf[disp_buf_count[1:0]] <= {2'd0, cur_active};
                        buffer_for_active[cur_active]       <= {1'b0, disp_buf_count[1:0]};
                        kick_q                              <= 1'b1;
                        disp_state                          <= D_KICK_WAIT_BUSY_HI;
                    end else begin
                        // Solid — skip.
                        disp_walk_i <= disp_walk_i + 4'd1;
                    end
                end

                D_KICK_WAIT_BUSY_HI: begin
                    if (tex_unit_busy_sync_i) begin
                        kick_q     <= 1'b0;
                        disp_state <= D_KICK_WAIT_BUSY_LO;
                    end
                end

                D_KICK_WAIT_BUSY_LO: begin
                    if (!tex_unit_busy_sync_i) begin
                        disp_walk_i    <= disp_walk_i + 4'd1;
                        disp_buf_count <= disp_buf_count + 3'd1;
                        disp_state     <= D_WALK;
                    end
                end

                default: disp_state <= D_IDLE;
            endcase
        end
    end

    // ---- Per-pixel painter ------------------------------------------
    // Two-pass logic. First pass (combinational): find the topmost
    // SOLID layer covering this pixel — that's the "background" the
    // textured layer (if any) blends against. Second pass: if the
    // topmost textured layer covers this pixel AND its z-order is
    // above the topmost solid, SrcAlpha-blend the textured pixel
    // over the solid; otherwise output the solid (or black if no
    // layer covers this pixel).
    //
    // Phase 2c step 1 limitations:
    //   - Only the topmost textured layer renders. Other textured
    //     slots are silently skipped (no debug colour — keeping the
    //     painter's combinational depth small enough to close timing
    //     at 100 MHz).
    //   - Blend uses the textured pixel's own alpha channel
    //     (BGRA8888). A8 textures land in step 2.
    //   - "Background" for the blend is the topmost SOLID at this x,
    //     not the full back-to-front composite. Multi-layer blending
    //     lands in step 3.
    //   - 8×8 multiplies inferred for the blend math. Cyclone V has
    //     ~150 DSP blocks; trivial to absorb 3 (R/G/B channels).

    wire signed [17:0] x_s = $signed({6'b0, hcount});
    wire [11:0] next_hcount = hcount + 12'd1;

    // ---- Stage 0: per-pixel scans + 4 parallel line buffer reads ---
    // Find topmost SOLID and topmost TEXTURED covering this pixel
    // independently. Both are 8-deep last-wins reductions over the
    // active list. The textured scan produces the active-slot index
    // of the topmost textured layer at this x (if any).
    logic        solid_hit_c;
    logic [4:0]  solid_idx_c;
    logic [31:0] solid_color_c;
    always_comb begin
        solid_hit_c   = 1'b0;
        solid_idx_c   = 5'd0;
        solid_color_c = 32'h0000_0000;
        for (int i = 0; i < MAX_ACTIVE; i++) begin
            if (i < int'(active_count)
                && active_tex_id[i] == 16'hFFFF
                && x_s >= $signed({active_dst_x_lo[i][16], active_dst_x_lo[i]})
                && x_s <  active_dst_x_hi[i]) begin
                solid_hit_c   = 1'b1;
                solid_idx_c   = i[4:0];
                solid_color_c = active_color[i];
            end
        end
    end

    // For each buffer K compute (a) whether its owning active slot
    // covers this pixel x, (b) the owning slot's active-list index
    // (for z-order vs the topmost solid), (c) the line-buffer
    // sub-pixel select bit. All three feed the stage-1 register.
    logic        buf_covers_c [MAX_TEXTURED-1:0];
    logic [4:0]  buf_slot_c   [MAX_TEXTURED-1:0]; // active-list index
    logic        buf_lsb_c    [MAX_TEXTURED-1:0];

    genvar gk;
    generate for (gk = 0; gk < MAX_TEXTURED; gk++) begin : g_buf
        wire [4:0]         slot       = active_for_buf[gk];
        wire               valid      = !slot[4]; // 5'h1F sentinel has bit 4 set
        wire [2:0]         slot_idx   = slot[2:0];
        wire signed [16:0] k_lo_s     = {active_dst_x_lo[slot_idx][16],
                                         active_dst_x_lo[slot_idx]};
        wire signed [17:0] k_hi_s     = active_dst_x_hi[slot_idx];
        wire [10:0]        k_dst_x_lo = active_dst_x_lo[slot_idx][10:0];
        wire [11:0]        k_x_off    = next_hcount - {1'b0, k_dst_x_lo};
        assign line_buf_addr_o[gk] = k_x_off[10:1];
        assign buf_lsb_c[gk]       = k_x_off[0];
        assign buf_slot_c[gk]      = slot;
        assign buf_covers_c[gk]    = valid
                                  && x_s >= {k_lo_s[16], k_lo_s}
                                  && x_s <  k_hi_s;
    end endgenerate

    // ---- Stage 1 register --------------------------------------------
    // Captures stage-0 reductions: topmost solid (the accumulator's
    // start value), plus per-buffer coverage / z-order / sub-pixel
    // info. line_buf_data_i becomes valid in this cycle (1-cycle
    // BRAM read latency).
    logic        solid_hit_q1;
    logic [4:0]  solid_idx_q1;
    logic [31:0] solid_color_q1;
    logic        buf_covers_q1 [MAX_TEXTURED-1:0];
    logic [4:0]  buf_slot_q1   [MAX_TEXTURED-1:0];
    logic        buf_lsb_q1    [MAX_TEXTURED-1:0];
    logic        h_in_sync_q1, v_in_sync_q1;
    logic        h_active_q1,  v_active_q1;

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            solid_hit_q1     <= 1'b0;
            solid_idx_q1     <= 5'd0;
            solid_color_q1   <= 32'd0;
            for (int i = 0; i < MAX_TEXTURED; i++) begin
                buf_covers_q1[i] <= 1'b0;
                buf_slot_q1[i]   <= 5'h1F;
                buf_lsb_q1[i]    <= 1'b0;
            end
            h_in_sync_q1     <= 1'b0;
            v_in_sync_q1     <= 1'b0;
            h_active_q1      <= 1'b0;
            v_active_q1      <= 1'b0;
        end else begin
            solid_hit_q1     <= solid_hit_c;
            solid_idx_q1     <= solid_idx_c;
            solid_color_q1   <= solid_color_c;
            for (int i = 0; i < MAX_TEXTURED; i++) begin
                buf_covers_q1[i] <= buf_covers_c[i];
                buf_slot_q1[i]   <= buf_slot_c[i];
                buf_lsb_q1[i]    <= buf_lsb_c[i];
            end
            h_in_sync_q1     <= h_in_sync;
            v_in_sync_q1     <= v_in_sync;
            h_active_q1      <= h_active;
            v_active_q1      <= v_active;
        end
    end

    // ---- Stage 1 combinational: half-select per buffer + contributes ----
    // line_buf_data_i[K] is valid this cycle. Pick the right 32-bit
    // half via buf_lsb_q1[K]. A buffer "contributes" to back-to-front
    // composition if it covers AND its owning slot is above the
    // topmost solid in z-order (the solid is otherwise fully opaque
    // and would hide it).
    logic [31:0] buf_pixel_c   [MAX_TEXTURED-1:0];
    logic        contributes_c [MAX_TEXTURED-1:0];
    always_comb begin
        for (int i = 0; i < MAX_TEXTURED; i++) begin
            buf_pixel_c[i]   = buf_lsb_q1[i] ? line_buf_data_i[i][63:32]
                                             : line_buf_data_i[i][31:0];
            contributes_c[i] = buf_covers_q1[i]
                            && (!solid_hit_q1 || (buf_slot_q1[i] > solid_idx_q1));
        end
    end

    // ---- Back-to-front blend pipeline --------------------------------
    // 4 register stages. Each one applies one buffer to the
    // accumulator. The contributes-bits and not-yet-applied pixel
    // values for later buffers propagate alongside the accumulator
    // so each stage has all it needs.
    //
    // Stage 2 register: accum = (solid ? solid_color : black) blended
    //                   with buffer 0 if it contributes.
    // Stage 3: accum blended with buffer 1.
    // Stage 4: accum blended with buffer 2.
    // Stage 5: accum blended with buffer 3.
    // Output register: r/g/b from final accum.
    //
    // Pixel latency: stage1 (1) + stages 2..5 (4) + output (1) = 6.
    // Sync/active signals follow the same depth via a shift chain.

    // Helper: blend src over dst using src's alpha. /255 ≈ >>8.
    function automatic logic [31:0] blend_over (
        input logic [31:0] src,
        input logic [31:0] dst
    );
        logic [7:0] sa, sr, sg, sb, ia, dr, dg, db;
        logic [15:0] br, bg, bb;
        sa = src[31:24]; sr = src[23:16]; sg = src[15:8]; sb = src[7:0];
        dr = dst[23:16]; dg = dst[15:8];  db = dst[7:0];
        ia = 8'd255 - sa;
        br = sr * sa + dr * ia;
        bg = sg * sa + dg * ia;
        bb = sb * sa + db * ia;
        blend_over = {8'hFF, br[15:8], bg[15:8], bb[15:8]};
    endfunction

    // Initial accumulator value (background): topmost solid or black.
    wire [31:0] accum_init_c = solid_hit_q1 ? solid_color_q1 : 32'h0;

    logic [31:0] accum_q2, accum_q3, accum_q4, accum_q5;
    // Propagated pixels + contributes for buffers not yet applied at
    // each stage.
    logic        contributes_q2_1, contributes_q2_2, contributes_q2_3;
    logic [31:0] buf_pixel_q2_1,   buf_pixel_q2_2,   buf_pixel_q2_3;
    logic        contributes_q3_2, contributes_q3_3;
    logic [31:0] buf_pixel_q3_2,   buf_pixel_q3_3;
    logic        contributes_q4_3;
    logic [31:0] buf_pixel_q4_3;

    // Sync signals shift through 4 extra register stages.
    logic h_in_sync_q2, h_in_sync_q3, h_in_sync_q4, h_in_sync_q5;
    logic v_in_sync_q2, v_in_sync_q3, v_in_sync_q4, v_in_sync_q5;
    logic h_active_q2,  h_active_q3,  h_active_q4,  h_active_q5;
    logic v_active_q2,  v_active_q3,  v_active_q4,  v_active_q5;

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            accum_q2 <= 32'd0; accum_q3 <= 32'd0;
            accum_q4 <= 32'd0; accum_q5 <= 32'd0;
            contributes_q2_1 <= 1'b0; contributes_q2_2 <= 1'b0; contributes_q2_3 <= 1'b0;
            buf_pixel_q2_1 <= 32'd0; buf_pixel_q2_2 <= 32'd0; buf_pixel_q2_3 <= 32'd0;
            contributes_q3_2 <= 1'b0; contributes_q3_3 <= 1'b0;
            buf_pixel_q3_2 <= 32'd0; buf_pixel_q3_3 <= 32'd0;
            contributes_q4_3 <= 1'b0;
            buf_pixel_q4_3 <= 32'd0;
            h_in_sync_q2 <= 1'b0; h_in_sync_q3 <= 1'b0; h_in_sync_q4 <= 1'b0; h_in_sync_q5 <= 1'b0;
            v_in_sync_q2 <= 1'b0; v_in_sync_q3 <= 1'b0; v_in_sync_q4 <= 1'b0; v_in_sync_q5 <= 1'b0;
            h_active_q2  <= 1'b0; h_active_q3  <= 1'b0; h_active_q4  <= 1'b0; h_active_q5  <= 1'b0;
            v_active_q2  <= 1'b0; v_active_q3  <= 1'b0; v_active_q4  <= 1'b0; v_active_q5  <= 1'b0;
        end else begin
            // Stage 2: initialise accum + apply buffer 0.
            accum_q2 <= contributes_c[0]
                      ? blend_over(buf_pixel_c[0], accum_init_c)
                      : accum_init_c;
            contributes_q2_1 <= contributes_c[1];
            contributes_q2_2 <= contributes_c[2];
            contributes_q2_3 <= contributes_c[3];
            buf_pixel_q2_1   <= buf_pixel_c[1];
            buf_pixel_q2_2   <= buf_pixel_c[2];
            buf_pixel_q2_3   <= buf_pixel_c[3];
            h_in_sync_q2 <= h_in_sync_q1;
            v_in_sync_q2 <= v_in_sync_q1;
            h_active_q2  <= h_active_q1;
            v_active_q2  <= v_active_q1;

            // Stage 3: apply buffer 1.
            accum_q3 <= contributes_q2_1
                      ? blend_over(buf_pixel_q2_1, accum_q2)
                      : accum_q2;
            contributes_q3_2 <= contributes_q2_2;
            contributes_q3_3 <= contributes_q2_3;
            buf_pixel_q3_2   <= buf_pixel_q2_2;
            buf_pixel_q3_3   <= buf_pixel_q2_3;
            h_in_sync_q3 <= h_in_sync_q2;
            v_in_sync_q3 <= v_in_sync_q2;
            h_active_q3  <= h_active_q2;
            v_active_q3  <= v_active_q2;

            // Stage 4: apply buffer 2.
            accum_q4 <= contributes_q3_2
                      ? blend_over(buf_pixel_q3_2, accum_q3)
                      : accum_q3;
            contributes_q4_3 <= contributes_q3_3;
            buf_pixel_q4_3   <= buf_pixel_q3_3;
            h_in_sync_q4 <= h_in_sync_q3;
            v_in_sync_q4 <= v_in_sync_q3;
            h_active_q4  <= h_active_q3;
            v_active_q4  <= v_active_q3;

            // Stage 5: apply buffer 3.
            accum_q5 <= contributes_q4_3
                      ? blend_over(buf_pixel_q4_3, accum_q4)
                      : accum_q4;
            h_in_sync_q5 <= h_in_sync_q4;
            v_in_sync_q5 <= v_in_sync_q4;
            h_active_q5  <= h_active_q4;
            v_active_q5  <= v_active_q4;
        end
    end

    logic [7:0] pix_r, pix_g, pix_b;
    always_comb begin
        if (h_active_q5 && v_active_q5) begin
            pix_r = accum_q5[23:16];
            pix_g = accum_q5[15:8];
            pix_b = accum_q5[7:0];
        end else begin
            pix_r = 8'd0;
            pix_g = 8'd0;
            pix_b = 8'd0;
        end
    end

    // ---- Output register ---------------------------------------------
    // r/g/b now lag hcount by 6 cycles (stage 1 + 4 blend stages +
    // output register). Sync / blank likewise pipeline through.
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            r      <= 8'd0;
            g      <= 8'd0;
            b      <= 8'd0;
            hsync  <= 1'b0;
            vsync  <= 1'b0;
            hblank <= 1'b1;
            vblank <= 1'b1;
        end else begin
            r      <= pix_r;
            g      <= pix_g;
            b      <= pix_b;
            hsync  <= h_in_sync_q5;
            vsync  <= v_in_sync_q5;
            hblank <= ~h_active_q5;
            vblank <= ~v_active_q5;
        end
    end

endmodule
