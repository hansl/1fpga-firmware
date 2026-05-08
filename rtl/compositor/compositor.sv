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
//    3. ASCAL accepts our timing and scales it to the user's HDMI mode.
//
//  Phase 2 will replace the test pattern with a layer-table walk that
//  reads from the host-managed layer descriptors at LAYER_TABLE_OFFSET
//  and samples textures from the existing texture pool.
//
//  Timing: 1280×720 @ 30 Hz with stuffed blanking, 50 MHz pixel clock
//  (== clk_sys). The framework's ASCAL accepts arbitrary core-side
//  timings and scales to whatever HDMI mode the user has configured;
//  we just need a stable VSYNC/HSYNC/DE pulse train. Stuffed blanking
//  (large H_TOTAL, V_TOTAL) lets the math come out at 50 MHz × ~30 Hz
//  with active 1280×720.
//
//    H: 1280 active + 386 blank = 1666 total
//    V:  720 active + 280 blank = 1000 total
//    => 50 MHz / (1666 × 1000) ≈ 30.01 Hz
//
//  HSYNC and VSYNC are positive-polarity (matches what most ASCAL
//  configurations and HDMI sinks accept).
//
//============================================================================

module compositor (
    input  logic        clk,        // pixel clock (50 MHz here)
    input  logic        rst_n,

    // Pixel data + timing for the framework's video_mixer / ASCAL.
    // HSync / VSync are positive-polarity pulses; HBlank / VBlank
    // are positive during the blanking interval (i.e. inverse of DE).
    output logic [7:0]  r,
    output logic [7:0]  g,
    output logic [7:0]  b,
    output logic        hsync,
    output logic        vsync,
    output logic        hblank,
    output logic        vblank
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

    // Counter widths sized to H_TOTAL=1666 → 11 bits, V_TOTAL=1000 → 10 bits.
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
    // 8 vertical color bars across the screen + 1-pixel white border.
    // Lets us eyeball that DE / sync are aligned and the whole screen
    // is being addressed. With H_ACTIVE = 1280, /160 gives us 8 bars.
    logic [2:0] bar_idx;
    assign bar_idx = hcount[9:7]; // groups of 128, close enough to 160

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
