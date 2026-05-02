//============================================================================
//
//  Triple-buffer state machine (M2c4).
//
//  Owns FB_DISPLAY / FB_RENDER / FB_READY per PROTOCOL.md §3.2 and §4.6.
//
//  - On `present_pulse_i` from the ring fetcher: latch READY <- RENDER and
//    rotate RENDER to the third (currently free) buffer so the host can
//    immediately keep drawing without stalling for vsync. PROTOCOL.md
//    describes RENDER updating "on next vsync" but that contradicts the
//    "PRESENT without stalling" property of triple-buffering, and the
//    practical interpretation we use is to advance RENDER at PRESENT
//    time.
//
//  - On `vsync_pulse_i` (rising edge of FB_VBL): if a frame is queued
//    (READY != 3), atomically swap DISPLAY <- READY, clear READY, and
//    bump FRAME_COUNT. VSYNC_COUNT increments unconditionally on every
//    vsync.
//
//  Buffer-rotation math: the "third index" is `3 ^ display ^ render`,
//  exploiting `0 ^ 1 ^ 2 == 3`. To break the boot-time degeneracy where
//  display and render would both be 0 (formula yields 3, an invalid FB
//  index), we initialise RENDER to 1.
//
//  When `present_pulse_i` and `vsync_pulse_i` fire on the same cycle,
//  vsync logically completes first (so its DISPLAY update is visible to
//  the present logic via the combinational next-state function below).
//  This avoids the trap where a naive non-blocking-assignment-ordering
//  would let render_q rotate onto the buffer that just became DISPLAY.
//
//============================================================================

module fb_swapper (
    input  logic        clk,
    input  logic        rst_n,

    input  logic        present_pulse_i,
    input  logic        vsync_pulse_i,

    output logic [1:0]  display_idx_o,
    output logic [1:0]  render_idx_o,
    output logic [1:0]  ready_idx_o,
    output logic [31:0] fb_state_o,
    output logic [31:0] frame_count_o,
    output logic [31:0] vsync_count_o
);

    localparam logic [1:0] EMPTY = 2'd3;

    logic [1:0]  display_q;
    logic [1:0]  render_q;
    logic [1:0]  ready_q;
    logic [31:0] frame_count_q;
    logic [31:0] vsync_count_q;

    logic [1:0]  next_display;
    logic [1:0]  next_render;
    logic [1:0]  next_ready;
    logic [31:0] next_frame_count;
    logic [31:0] next_vsync_count;

    always_comb begin
        next_display     = display_q;
        next_render      = render_q;
        next_ready       = ready_q;
        next_frame_count = frame_count_q;
        next_vsync_count = vsync_count_q;

        // Vsync logically completes first so its DISPLAY update feeds
        // into PRESENT's render rotation if both pulses arrive together.
        if (vsync_pulse_i) begin
            next_vsync_count = vsync_count_q + 32'd1;
            if (ready_q != EMPTY) begin
                next_display     = ready_q;
                next_ready       = EMPTY;
                next_frame_count = frame_count_q + 32'd1;
            end
        end

        if (present_pulse_i) begin
            next_ready  = render_q;
            next_render = 2'd3 ^ next_display ^ render_q;
        end
    end

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            display_q     <= 2'd0;
            render_q      <= 2'd1;
            ready_q       <= EMPTY;
            frame_count_q <= 32'd0;
            vsync_count_q <= 32'd0;
        end else begin
            display_q     <= next_display;
            render_q      <= next_render;
            ready_q       <= next_ready;
            frame_count_q <= next_frame_count;
            vsync_count_q <= next_vsync_count;
        end
    end

    assign display_idx_o = display_q;
    assign render_idx_o  = render_q;
    assign ready_idx_o   = ready_q;
    assign fb_state_o    = {26'd0, ready_q, render_q, display_q};
    assign frame_count_o = frame_count_q;
    assign vsync_count_o = vsync_count_q;

endmodule
