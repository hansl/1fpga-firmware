//============================================================================
//
//  Blit engine (M2c1 — FILL_RECT only).
//
//  Accepts a single in-flight FILL_RECT request (start_i pulse + latched
//  rect/color) and walks pixels left-to-right, top-to-bottom, issuing
//  one DDRAM write per pixel to the framebuffer at FB_BASE. Asserts
//  busy_o while running and pulses done_o when the last pixel retires.
//
//  M2c1 simplifications:
//    - Opaque blend only (write-only path; no read-modify-write).
//    - No clipping (the host's clip rect doesn't exist yet anyway).
//    - One pixel per 64-bit beat (waste 4 of 8 bytes; M2c5 packs two).
//    - No bursting (BURSTCNT = 1 always).
//    - No bounds-check against FB_WIDTH / FB_HEIGHT — the host is
//      expected to pass valid coordinates. M2c2+ will clamp.
//
//  Pixel byte order: framework's FB_FORMAT = 5'b10110 selects BGR 32bpp,
//  so bytes ascend B, G, R, A in memory. The protocol's RGBA -> u32
//  packs A:R:G:B from MSB->LSB; little-endian store places B at the
//  lowest byte, matching the framework's expectation. We therefore
//  pass the host-supplied 32-bit color word straight to writedata.
//
//============================================================================

module blit_engine (
    input  logic        clk,
    input  logic        rst_n,

    // Command interface from ring_fetcher (single in-flight blit).
    input  logic        start_i,         // 1-cycle pulse
    input  logic [15:0] dst_x_i,
    input  logic [15:0] dst_y_i,
    input  logic [15:0] dst_w_i,
    input  logic [15:0] dst_h_i,
    input  logic [31:0] color_i,

    // Framebuffer geometry (latched values from menu_core.sv).
    input  logic [31:0] fb_base_i,       // byte address (host-physical)
    input  logic [13:0] fb_stride_i,     // bytes per row

    output logic        busy_o,
    output logic        done_o,          // 1-cycle pulse when blit retires

    // DDRAM write-master interface. The top-level mux owns the actual
    // DDRAM_* pins; we just produce these signals when busy_o is high.
    output logic [28:0] ddram_addr_o,
    output logic [7:0]  ddram_burstcnt_o,
    output logic [7:0]  ddram_be_o,
    output logic [63:0] ddram_din_o,
    output logic        ddram_we_o,
    input  logic        ddram_busy_i
);

    // ---- FSM ---------------------------------------------------------
    typedef enum logic [2:0] {
        S_IDLE,
        S_ROW_INIT,
        S_PIXEL,
        S_WAIT,
        S_DONE
    } state_e;

    state_e      state;
    logic [15:0] dst_x_q, dst_y_q, dst_w_q, dst_h_q;
    logic [31:0] color_q;
    logic [15:0] cur_x, cur_y_off;       // cur_y_off counts rows from 0 to dst_h-1
    logic [31:0] row_byte_addr;          // byte address of (dst_x, dst_y + cur_y_off)
    logic [31:0] pixel_byte_addr;

    assign busy_o = (state != S_IDLE) & (state != S_DONE);

    // Pixel byte address = row_byte_addr + cur_x * 4
    assign pixel_byte_addr = row_byte_addr + ({16'd0, cur_x} <<< 2);

    // DDRAM address is word-addressed (8-byte beats).
    wire [28:0] beat_addr  = pixel_byte_addr[31:3];
    wire        upper_half = pixel_byte_addr[2];
    wire [7:0]  pixel_be   = upper_half ? 8'b1111_0000 : 8'b0000_1111;
    wire [63:0] pixel_din  = upper_half
                              ? {color_q, 32'd0}
                              : {32'd0, color_q};

    wire writing_pixel = (state == S_PIXEL) & (cur_x < dst_w_q);

    // ---- Output multiplexing ---------------------------------------
    always_comb begin
        if (writing_pixel) begin
            ddram_addr_o     = beat_addr;
            ddram_burstcnt_o = 8'd1;
            ddram_be_o       = pixel_be;
            ddram_din_o      = pixel_din;
            ddram_we_o       = 1'b1;
        end else begin
            ddram_addr_o     = 29'd0;
            ddram_burstcnt_o = 8'd0;
            ddram_be_o       = 8'd0;
            ddram_din_o      = 64'd0;
            ddram_we_o       = 1'b0;
        end
    end

    // ---- FSM transitions -------------------------------------------
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state         <= S_IDLE;
            dst_x_q       <= '0;
            dst_y_q       <= '0;
            dst_w_q       <= '0;
            dst_h_q       <= '0;
            color_q       <= '0;
            cur_x         <= '0;
            cur_y_off     <= '0;
            row_byte_addr <= '0;
            done_o        <= 1'b0;
        end else begin
            done_o <= 1'b0;        // default: deassert; pulse in S_DONE

            unique case (state)
                S_IDLE: if (start_i) begin
                    dst_x_q   <= dst_x_i;
                    dst_y_q   <= dst_y_i;
                    dst_w_q   <= dst_w_i;
                    dst_h_q   <= dst_h_i;
                    color_q   <= color_i;
                    cur_y_off <= '0;
                    state     <= S_ROW_INIT;
                end

                S_ROW_INIT: begin
                    if (cur_y_off == dst_h_q) begin
                        state <= S_DONE;
                    end else begin
                        // row_byte_addr = fb_base + (dst_y + cur_y_off) * fb_stride + dst_x * 4
                        // Use multipliers; the synthesiser maps to DSP blocks.
                        row_byte_addr <= fb_base_i
                            + ({16'd0, (dst_y_q + cur_y_off)} * {18'd0, fb_stride_i})
                            + ({16'd0, dst_x_q} <<< 2);
                        cur_x <= '0;
                        state <= S_PIXEL;
                    end
                end

                S_PIXEL: begin
                    if (cur_x == dst_w_q) begin
                        cur_y_off <= cur_y_off + 16'd1;
                        state     <= S_ROW_INIT;
                    end else if (~ddram_busy_i) begin
                        // Write accepted this cycle; advance once we
                        // see ddram_busy deassert again.
                        state <= S_WAIT;
                    end
                end

                S_WAIT: begin
                    cur_x <= cur_x + 16'd1;
                    state <= S_PIXEL;
                end

                S_DONE: begin
                    done_o <= 1'b1;
                    state  <= S_IDLE;
                end
            endcase
        end
    end

endmodule
