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
//  Timing: 1280×720 @ 30 Hz with stuffed blanking, 50 MHz pixel clock
//  (== clk_sys). The framework's ASCAL accepts arbitrary core-side
//  timings and scales to whatever HDMI mode the user has configured.
//
//    H: 1280 active + 386 blank = 1666 total
//    V:  720 active + 280 blank = 1000 total
//    => 50 MHz / (1666 × 1000) ≈ 30.01 Hz
//
//  HSYNC and VSYNC are positive-polarity.
//
//============================================================================

module compositor #(
    parameter int MAX_ACTIVE = 16
) (
    input  logic        clk,        // CLK_VIDEO == pixel clock (50 MHz)
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
    input  logic [8:0]   layer_count_i
);

    // ---- Timing constants (1280×720 @ 30 Hz, 50 MHz pixel clock) ------
    localparam int H_ACTIVE = 1280;
    localparam int H_FP     = 110;
    localparam int H_SYNC   = 40;
    localparam int H_BP     = 236;
    localparam int H_TOTAL  = H_ACTIVE + H_FP + H_SYNC + H_BP; // 1666

    localparam int V_ACTIVE = 720;
    localparam int V_FP     = 5;
    localparam int V_SYNC   = 5;
    localparam int V_BP     = 270;
    localparam int V_TOTAL  = V_ACTIVE + V_FP + V_SYNC + V_BP; // 1000

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
        .active_color_o   (active_color)
    );

    // ---- Per-pixel painter ------------------------------------------
    // Scan low-to-high so the topmost (highest-index) hit wins via
    // last-assignment-wins. BGRA byte order: B at LSB, R at byte 2.
    wire signed [17:0] x_s = $signed({6'b0, hcount});

    logic [31:0] pix_color;
    always_comb begin
        pix_color = 32'h0000_0000; // black background
        for (int i = 0; i < MAX_ACTIVE; i++) begin
            if (i < int'(active_count)
                && x_s >= $signed({active_dst_x_lo[i][16], active_dst_x_lo[i]})
                && x_s <  active_dst_x_hi[i]) begin
                pix_color = active_color[i];
            end
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
