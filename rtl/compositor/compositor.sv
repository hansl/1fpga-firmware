//============================================================================
//
//  Compositor scanout — Phase 1 (test pattern).
//
//  Replaces the framework's MISTER_FB scanout path with our own per-scanline
//  pixel generator. At Phase 1 the output is a fixed test pattern (color
//  bars + a 1-pixel white border) — no layer-table reads yet, no DDR3
//  traffic. Just enough to validate that:
//
//    1. The video clock + timing generator drive a stable HDMI sink.
//    2. VGA_R/G/B/HS/VS/DE wiring flows through the framework's HDMI
//       pipeline correctly with FB_EN = 0.
//    3. The framework's HDMI scaler (if any) and clock-select block
//       accept our timing.
//
//  Phase 2 will replace the test pattern with a layer-table walk that
//  reads from the host-managed layer descriptors at LAYER_TABLE_OFFSET
//  and samples textures from the existing texture pool.
//
//  Timing is hard-coded for 1080p60 (CEA-861 mode 16):
//
//    H: 1920 active + 88 fp + 44 sync + 148 bp = 2200 total
//    V: 1080 active + 4 fp + 5 sync + 36 bp   = 1125 total
//    Pixel clock: 148.5 MHz
//
//  HSYNC and VSYNC are positive-polarity per CEA-861.
//
//============================================================================

module compositor (
    input  logic        clk,        // pixel clock (148.5 MHz for 1080p60)
    input  logic        rst_n,

    // Video output to the framework's HDMI pipeline (or external scaler).
    output logic [7:0]  vga_r,
    output logic [7:0]  vga_g,
    output logic [7:0]  vga_b,
    output logic        vga_hs,
    output logic        vga_vs,
    output logic        vga_de
);

    // ---- 1080p60 timing constants (CEA-861 mode 16) -------------------
    localparam int H_ACTIVE = 1920;
    localparam int H_FP     = 88;
    localparam int H_SYNC   = 44;
    localparam int H_BP     = 148;
    localparam int H_TOTAL  = H_ACTIVE + H_FP + H_SYNC + H_BP; // 2200

    localparam int V_ACTIVE = 1080;
    localparam int V_FP     = 4;
    localparam int V_SYNC   = 5;
    localparam int V_BP     = 36;
    localparam int V_TOTAL  = V_ACTIVE + V_FP + V_SYNC + V_BP; // 1125

    // Counter widths sized to V_TOTAL=1125 → 11 bits.
    logic [11:0] hcount;
    logic [11:0] vcount;

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
    // Sync windows: directly after the front porch.
    wire h_in_sync = (hcount >= H_ACTIVE + H_FP)
                  && (hcount <  H_ACTIVE + H_FP + H_SYNC);
    wire v_in_sync = (vcount >= V_ACTIVE + V_FP)
                  && (vcount <  V_ACTIVE + V_FP + V_SYNC);
    wire h_active  = (hcount < H_ACTIVE);
    wire v_active  = (vcount < V_ACTIVE);

    // ---- Test pattern -------------------------------------------------
    // 8 vertical color bars across the screen (240px each) + 1-pixel
    // white border. Lets us eyeball that DE / sync are aligned and the
    // whole screen is being addressed.
    logic [2:0] bar_idx;
    assign bar_idx = hcount[10:8]; // groups of 256, close enough to 240

    logic [7:0] bar_r, bar_g, bar_b;
    always_comb begin
        unique case (bar_idx)
            3'd0: {bar_r, bar_g, bar_b} = 24'hFF_FF_FF; // white
            3'd1: {bar_r, bar_g, bar_b} = 24'hFF_FF_00; // yellow
            3'd2: {bar_r, bar_g, bar_b} = 24'h00_FF_FF; // cyan
            3'd3: {bar_r, bar_g, bar_b} = 24'h00_FF_00; // green
            3'd4: {bar_r, bar_g, bar_b} = 24'hFF_00_FF; // magenta
            3'd5: {bar_r, bar_g, bar_b} = 24'hFF_00_00; // red
            3'd6: {bar_r, bar_g, bar_b} = 24'h00_00_FF; // blue
            3'd7: {bar_r, bar_g, bar_b} = 24'h20_20_20; // dark grey
        endcase
    end

    wire on_border = h_active && v_active
                  && ((hcount == 0) || (hcount == H_ACTIVE - 1)
                   || (vcount == 0) || (vcount == V_ACTIVE - 1));

    logic [7:0] pix_r, pix_g, pix_b;
    always_comb begin
        if (!h_active || !v_active) begin
            // Blanking interval — must drive 0 per HDMI conventions.
            pix_r = 8'd0;
            pix_g = 8'd0;
            pix_b = 8'd0;
        end else if (on_border) begin
            pix_r = 8'hFF;
            pix_g = 8'hFF;
            pix_b = 8'hFF;
        end else begin
            pix_r = bar_r;
            pix_g = bar_g;
            pix_b = bar_b;
        end
    end

    // ---- One-cycle register on the outputs to keep IO timing clean ----
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            vga_r  <= 8'd0;
            vga_g  <= 8'd0;
            vga_b  <= 8'd0;
            vga_hs <= 1'b0;
            vga_vs <= 1'b0;
            vga_de <= 1'b0;
        end else begin
            vga_r  <= pix_r;
            vga_g  <= pix_g;
            vga_b  <= pix_b;
            vga_hs <= h_in_sync;
            vga_vs <= v_in_sync;
            vga_de <= h_active && v_active;
        end
    end

endmodule
