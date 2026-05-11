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
//  Timing: native 1920×1080 with minimum-blanking, 100 MHz pixel
//  clock (= clk_video, separate from clk_sys via a second PLL output).
//  ASCAL still accepts arbitrary core-side timing; producing pixels at
//  native resolution lets text/UI keep their pixel-perfect crispness
//  on a 1080p HDMI sink.
//
//    H: 1920 active + 280 blank = 2200 total
//    V: 1080 active +  20 blank = 1100 total
//    => 100 MHz / (2200 × 1100) ≈ 41.3 Hz
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
    parameter int MAX_ACTIVE = 16
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

    // Texture sampler interface (Phase 2b step 2). The compositor
    // identifies the topmost textured layer in the active list after
    // the filter completes, and pulses `tex_kick_o` with the
    // descriptor params. The texture_unit on clk_sys responds by
    // fetching the texel row into the line buffer; the painter then
    // reads `line_buf_data_i` at half-resolution addresses (each
    // entry holds 2 BGRA pixels). All multi-bit kick params are
    // expected to be held stable while `tex_kick_o` is high.
    output logic        tex_kick_o,
    output logic [15:0] tex_id_o,
    output logic [15:0] tex_src_x_o,
    output logic [15:0] tex_ty_o,
    output logic [11:0] tex_dst_w_o,

    // Line buffer read port (painter side). Address = (x - dst_x_lo)
    // >> 1 — each 64-bit word holds 2 pixels.
    output logic [9:0]  line_buf_addr_o,
    input  logic [63:0] line_buf_data_i
);

    // ---- Timing constants (1920×1080, 100 MHz pixel clock).
    // HBlank widened in Phase 2b step 2 to fit:
    //   - scanline_filter worst case (count=256): ~260 cycles
    //   - kick-to-clk_sys CDC: ~10 cycles
    //   - texture_unit (descriptor + 256-pixel row burst): ~290 cycles
    //   - margin: ~40 cycles
    // Total HBlank ≈ 600 cycles.
    // VBlank = 20 lines (unchanged; layer_dma fits comfortably).
    // fps = 100 MHz / (2520 × 1100) ≈ 36.1 Hz.
    localparam int H_ACTIVE = 1920;
    localparam int H_FP     = 60;
    localparam int H_SYNC   = 40;
    localparam int H_BP     = 500;
    localparam int H_TOTAL  = H_ACTIVE + H_FP + H_SYNC + H_BP; // 2520

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

    // ---- Topmost-textured selector + kick generator -----------------
    // After the filter completes, find the highest-index entry whose
    // tex_id != 0xFFFF. The 16-deep priority scan is too long to
    // chain into the painter's already 16-deep mux loop at 100 MHz
    // (-1.1 ns slack observed), so the combinational result is
    // captured into registers each cycle. The active list is stable
    // for the duration of the active scanout, so a 1-cycle delay is
    // invisible.
    logic       topmost_tex_valid_c;
    logic [4:0] topmost_tex_idx_c;
    always_comb begin
        topmost_tex_valid_c = 1'b0;
        topmost_tex_idx_c   = 5'd0;
        for (int i = 0; i < MAX_ACTIVE; i++) begin
            if (i < int'(active_count) && active_tex_id[i] != 16'hFFFF) begin
                topmost_tex_valid_c = 1'b1;
                topmost_tex_idx_c   = i[4:0];
            end
        end
    end

    logic       topmost_tex_valid;
    logic [4:0] topmost_tex_idx;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            topmost_tex_valid <= 1'b0;
            topmost_tex_idx   <= 5'd0;
        end else begin
            topmost_tex_valid <= topmost_tex_valid_c;
            topmost_tex_idx   <= topmost_tex_idx_c;
        end
    end

    // Kick window: hold tex_kick_o high for a chunk of HBlank after
    // the filter has had time to complete (~260 cycles). 60 clk_video
    // cycles ≈ 30 clk_sys cycles — plenty for the 2-flop synchroniser
    // on the clk_sys side to catch the rising edge.
    wire kick_window = (hcount >= 12'(H_ACTIVE + 280))
                    && (hcount <  12'(H_ACTIVE + 340));
    assign tex_kick_o  = kick_window && topmost_tex_valid;
    assign tex_id_o    = active_tex_id [topmost_tex_idx];
    assign tex_src_x_o = active_src_x  [topmost_tex_idx];
    assign tex_ty_o    = active_ty     [topmost_tex_idx];
    assign tex_dst_w_o = active_dst_x_hi[topmost_tex_idx][11:0]
                       - active_dst_x_lo[topmost_tex_idx][11:0];

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
    //   - Only the topmost textured layer's pixels render. Other
    //     textured slots still paint debug-magenta.
    //   - Blend uses the textured pixel's own alpha channel
    //     (BGRA8888). A8 textures land in step 2.
    //   - "Background" for the blend is the topmost SOLID at this x,
    //     not the full back-to-front composite. Multi-layer blending
    //     lands in step 3.
    //   - 8×8 multiplies inferred for the blend math. Cyclone V has
    //     ~150 DSP blocks; trivial to absorb 3 (R/G/B channels).
    localparam logic [31:0] DEBUG_TEX_COLOR = 32'hFF_FF_00_FF; // BGRA magenta

    wire signed [17:0] x_s = $signed({6'b0, hcount});

    // Line-buffer prefetch: read for `hcount + 1` so the data arrives
    // one cycle later, in time for that pixel's painter pass.
    wire [11:0] next_hcount      = hcount + 12'd1;
    wire [10:0] topmost_dst_lo_u = active_dst_x_lo[topmost_tex_idx][10:0];
    wire [11:0] line_buf_x_off   = next_hcount - {1'b0, topmost_dst_lo_u};
    assign line_buf_addr_o = line_buf_x_off[10:1]; // /2, 10-bit

    // Pixel within the 64-bit line buffer entry (bit 0 of the
    // x-offset selects upper/lower 32-bit half).
    wire        tex_pixel_sel = line_buf_x_off[0];
    wire [31:0] tex_pixel     = tex_pixel_sel ? line_buf_data_i[63:32]
                                              : line_buf_data_i[31:0];

    // First pass: find the topmost solid (= highest-index solid that
    // covers this pixel). `solid_idx_q_c` = its index, or 5'h1F as a
    // sentinel meaning "no solid covers this pixel" (in which case the
    // background defaults to black).
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

    // Is the topmost textured layer covering this pixel, and is it
    // above the topmost solid?
    wire signed [16:0] topmost_lo_s = {active_dst_x_lo[topmost_tex_idx][16],
                                       active_dst_x_lo[topmost_tex_idx]};
    wire signed [17:0] topmost_hi_s = active_dst_x_hi[topmost_tex_idx];
    wire topmost_tex_covers = topmost_tex_valid
                           && x_s >= {topmost_lo_s[16], topmost_lo_s}
                           && x_s <  topmost_hi_s
                           && (!solid_hit_c || (topmost_tex_idx > solid_idx_c));

    // SrcAlpha blend: out = src.rgb * a + dst.rgb * (255 - a), all
    // /255. We approximate /255 as >>8 (saturating mul) — a 0.4%
    // brightness error that's invisible on a 8bpc display. Each
    // channel: (src_c * a + dst_c * (255-a) + 128) >> 8 would be the
    // rounding-true form; we drop the +128 to keep the LUTs cheap.
    wire [7:0] tex_a = tex_pixel[31:24];
    wire [7:0] tex_r = tex_pixel[23:16];
    wire [7:0] tex_g = tex_pixel[15:8];
    wire [7:0] tex_b = tex_pixel[7:0];
    wire [7:0] inv_a = 8'd255 - tex_a;
    wire [7:0] bg_r  = solid_color_c[23:16];
    wire [7:0] bg_g  = solid_color_c[15:8];
    wire [7:0] bg_b  = solid_color_c[7:0];

    wire [15:0] blend_r = tex_r * tex_a + bg_r * inv_a;
    wire [15:0] blend_g = tex_g * tex_a + bg_g * inv_a;
    wire [15:0] blend_b = tex_b * tex_a + bg_b * inv_a;

    wire [31:0] blended_color = {8'hFF, blend_r[15:8], blend_g[15:8], blend_b[15:8]};

    // Other-textured (non-topmost) slots paint debug-magenta so a
    // multi-textured scene visibly flags the unsupported case.
    logic         other_tex_hit_c;
    always_comb begin
        other_tex_hit_c = 1'b0;
        for (int i = 0; i < MAX_ACTIVE; i++) begin
            if (i < int'(active_count)
                && active_tex_id[i] != 16'hFFFF
                && (!topmost_tex_valid || i[4:0] != topmost_tex_idx)
                && x_s >= $signed({active_dst_x_lo[i][16], active_dst_x_lo[i]})
                && x_s <  active_dst_x_hi[i]) begin
                other_tex_hit_c = 1'b1;
            end
        end
    end

    logic [31:0] pix_color;
    always_comb begin
        if (topmost_tex_covers) begin
            pix_color = blended_color;
        end else if (other_tex_hit_c) begin
            pix_color = DEBUG_TEX_COLOR;
        end else if (solid_hit_c) begin
            pix_color = solid_color_c;
        end else begin
            pix_color = 32'h0000_0000;
        end
    end

    logic [7:0] pix_r, pix_g, pix_b;
    always_comb begin
        if (h_active && v_active) begin
            pix_r = pix_color[23:16];
            pix_g = pix_color[15:8];
            pix_b = pix_color[7:0];
        end else begin
            pix_r = 8'd0;
            pix_g = 8'd0;
            pix_b = 8'd0;
        end
    end

    // ---- One-cycle register on the outputs ---------------------------
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
            hsync  <= h_in_sync;
            vsync  <= v_in_sync;
            hblank <= ~h_active;
            vblank <= ~v_active;
        end
    end

endmodule
