//============================================================================
//
//  Blit engine (M2c1 FILL_RECT + M2c3.1 COPY_RECT 1:1 RGBA8888 opaque).
//
//  Two modes selected by `mode_i` at the start of a blit:
//
//    MODE_FILL (0): walk every pixel of the destination rect and write
//      `color_i` (constant). Used by FILL_RECT.
//
//    MODE_COPY (1): for every pixel, read the matching source pixel
//      from a texture in DDR3 and write it to the framebuffer. Used by
//      COPY_RECT in its simplest form — no scaling (src_w == dst_w,
//      src_h == dst_h), no tint, no blend (Opaque only). Source format
//      must be RGBA8888 for now; A8 + tint + SrcAlpha land in M2c3.2/3.
//
//  Both modes still write one pixel per 64-bit DDR3 beat (wasteful but
//  simple); bursting + 2-pixels-per-beat is M2c5.
//
//============================================================================

module blit_engine (
    input  logic        clk,
    input  logic        rst_n,

    // Command interface from ring_fetcher (single in-flight blit).
    input  logic        start_i,
    input  logic        mode_i,           // 0 = FILL, 1 = COPY
    input  logic [15:0] dst_x_i,
    input  logic [15:0] dst_y_i,
    input  logic [15:0] dst_w_i,
    input  logic [15:0] dst_h_i,
    input  logic [31:0] color_i,          // FILL: constant; COPY: ignored

    // COPY-only inputs.
    input  logic [15:0] src_x_i,
    input  logic [15:0] src_y_i,
    input  logic [31:0] src_addr_i,       // data_addr from texture descriptor
    input  logic [31:0] src_pitch_i,      // pitch_bytes from descriptor

    // Framebuffer geometry.
    input  logic [31:0] fb_base_i,
    input  logic [13:0] fb_stride_i,

    output logic        busy_o,
    output logic        done_o,

    // DDRAM master interface (writes for both modes; reads only in COPY).
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

    typedef enum logic [3:0] {
        S_IDLE,
        S_ROW_INIT,
        S_NEXT_PIXEL,
        S_FETCH_SRC,
        S_WAIT_SRC,
        S_WRITE,
        S_WRITE_WAIT,
        S_DONE
    } state_e;

    state_e      state;
    logic        mode_q;
    logic [15:0] dst_x_q, dst_y_q, dst_w_q, dst_h_q;
    logic [31:0] color_q;
    logic [15:0] src_x_q, src_y_q;
    logic [31:0] src_addr_q;
    logic [31:0] src_pitch_q;

    logic [15:0] cur_x, cur_y_off;
    logic [31:0] dst_row_byte_addr;
    logic [31:0] src_row_byte_addr;
    logic [31:0] pixel_data;          // value to write this pixel

    assign busy_o = (state != S_IDLE) & (state != S_DONE);

    // Per-pixel byte addresses.
    wire [31:0] dst_pixel_byte_addr = dst_row_byte_addr + ({16'd0, cur_x} <<< 2);
    wire [31:0] src_pixel_byte_addr = src_row_byte_addr + ({16'd0, cur_x} <<< 2);

    // Helpers for the 64-bit beat alignment.
    function automatic logic [7:0] be_for(input logic upper);
        return upper ? 8'b1111_0000 : 8'b0000_1111;
    endfunction

    function automatic logic [31:0] pick_word(input logic [63:0] beat,
                                              input logic        upper);
        return upper ? beat[63:32] : beat[31:0];
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
                ddram_be_o       = be_for(src_pixel_byte_addr[2]);
                ddram_rd_o       = 1'b1;
            end
            S_WRITE: begin
                ddram_addr_o     = dst_pixel_byte_addr[31:3];
                ddram_burstcnt_o = 8'd1;
                ddram_be_o       = be_for(dst_pixel_byte_addr[2]);
                ddram_din_o      = dst_pixel_byte_addr[2]
                                       ? {pixel_data, 32'd0}
                                       : {32'd0, pixel_data};
                ddram_we_o       = 1'b1;
            end
            default: ;
        endcase
    end

    // ---- FSM transitions -------------------------------------------
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state             <= S_IDLE;
            mode_q            <= MODE_FILL;
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
            done_o            <= 1'b0;
        end else begin
            done_o <= 1'b0;

            unique case (state)
                S_IDLE: if (start_i) begin
                    mode_q      <= mode_i;
                    dst_x_q     <= dst_x_i;
                    dst_y_q     <= dst_y_i;
                    dst_w_q     <= dst_w_i;
                    dst_h_q     <= dst_h_i;
                    color_q     <= color_i;
                    src_x_q     <= src_x_i;
                    src_y_q     <= src_y_i;
                    src_addr_q  <= src_addr_i;
                    src_pitch_q <= src_pitch_i;
                    cur_y_off   <= '0;
                    state       <= S_ROW_INIT;
                end

                S_ROW_INIT: begin
                    if (cur_y_off == dst_h_q) begin
                        state <= S_DONE;
                    end else begin
                        dst_row_byte_addr <= fb_base_i
                            + ({16'd0, (dst_y_q + cur_y_off)} * {18'd0, fb_stride_i})
                            + ({16'd0, dst_x_q} <<< 2);
                        src_row_byte_addr <= src_addr_q
                            + ({16'd0, (src_y_q + cur_y_off)} * src_pitch_q)
                            + ({16'd0, src_x_q} <<< 2);
                        cur_x <= '0;
                        state <= S_NEXT_PIXEL;
                    end
                end

                S_NEXT_PIXEL: begin
                    if (cur_x == dst_w_q) begin
                        cur_y_off <= cur_y_off + 16'd1;
                        state     <= S_ROW_INIT;
                    end else if (mode_q == MODE_COPY) begin
                        state <= S_FETCH_SRC;
                    end else begin
                        pixel_data <= color_q;
                        state      <= S_WRITE;
                    end
                end

                S_FETCH_SRC: if (~ddram_busy_i) begin
                    state <= S_WAIT_SRC;
                end

                S_WAIT_SRC: if (ddram_dout_valid_i) begin
                    pixel_data <= pick_word(ddram_dout_i, src_pixel_byte_addr[2]);
                    state      <= S_WRITE;
                end

                S_WRITE: if (~ddram_busy_i) begin
                    state <= S_WRITE_WAIT;
                end

                S_WRITE_WAIT: begin
                    cur_x <= cur_x + 16'd1;
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
