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
    input  logic [15:0] dst_x_i,
    input  logic [15:0] dst_y_i,
    input  logic [15:0] dst_w_i,
    input  logic [15:0] dst_h_i,
    input  logic [31:0] color_i,          // FILL: constant; COPY: ignored

    // COPY-only inputs.
    input  logic [15:0] src_x_i,
    input  logic [15:0] src_y_i,
    input  logic [31:0] src_addr_i,
    input  logic [31:0] src_pitch_i,
    input  logic        format_i,         // 0 = RGBA8888, 1 = A8
    input  logic        tint_en_i,
    input  logic [31:0] tint_color_i,

    // Framebuffer geometry.
    input  logic [31:0] fb_base_i,
    input  logic [13:0] fb_stride_i,

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
    logic        format_q;
    logic        tint_en_q;
    logic [31:0] tint_color_q;
    logic [15:0] dst_x_q, dst_y_q, dst_w_q, dst_h_q;
    logic [31:0] color_q;
    logic [15:0] src_x_q, src_y_q;
    logic [31:0] src_addr_q;
    logic [31:0] src_pitch_q;

    logic [15:0] cur_x, cur_y_off;
    logic [31:0] dst_row_byte_addr;
    logic [31:0] src_row_byte_addr;
    logic [31:0] pixel_data;

    assign busy_o = (state != S_IDLE) & (state != S_DONE);

    // Per-pixel byte addresses.
    // RGBA8888: x*4. A8: x*1.
    wire [31:0] cur_x_offset_dst = ({16'd0, cur_x} <<< 2);
    wire [31:0] cur_x_offset_src = (format_q == FMT_A8)
                                       ? {16'd0, cur_x}
                                       : ({16'd0, cur_x} <<< 2);
    wire [31:0] dst_pixel_byte_addr = dst_row_byte_addr + cur_x_offset_dst;
    wire [31:0] src_pixel_byte_addr = src_row_byte_addr + cur_x_offset_src;

    // ---- Helpers ----------------------------------------------------
    function automatic logic [7:0] be_for_word(input logic upper);
        return upper ? 8'b1111_0000 : 8'b0000_1111;
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
                // Reads return the full 64-bit beat regardless of BE,
                // but the slave still wants something sensible asserted.
                ddram_be_o       = (format_q == FMT_A8)
                                       ? be_for_byte(src_pixel_byte_addr[2:0])
                                       : be_for_word(src_pixel_byte_addr[2]);
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
            default: ;
        endcase
    end

    // ---- FSM transitions -------------------------------------------
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state             <= S_IDLE;
            mode_q            <= MODE_FILL;
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
                    mode_q       <= mode_i;
                    format_q     <= format_i;
                    tint_en_q    <= tint_en_i;
                    tint_color_q <= tint_color_i;
                    dst_x_q      <= dst_x_i;
                    dst_y_q      <= dst_y_i;
                    dst_w_q      <= dst_w_i;
                    dst_h_q      <= dst_h_i;
                    color_q      <= color_i;
                    src_x_q      <= src_x_i;
                    src_y_q      <= src_y_i;
                    src_addr_q   <= src_addr_i;
                    src_pitch_q  <= src_pitch_i;
                    cur_y_off    <= '0;
                    state        <= S_ROW_INIT;
                end

                S_ROW_INIT: begin
                    if (cur_y_off == dst_h_q) begin
                        state <= S_DONE;
                    end else begin
                        dst_row_byte_addr <= fb_base_i
                            + ({16'd0, (dst_y_q + cur_y_off)} * {18'd0, fb_stride_i})
                            + ({16'd0, dst_x_q} <<< 2);
                        // src x byte multiplier depends on format.
                        src_row_byte_addr <= src_addr_q
                            + ({16'd0, (src_y_q + cur_y_off)} * src_pitch_q)
                            + ((format_q == FMT_A8)
                                  ? {16'd0, src_x_q}
                                  : ({16'd0, src_x_q} <<< 2));
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
                    automatic logic [31:0] src_word;
                    automatic logic [7:0]  sampled_alpha;
                    if (format_q == FMT_A8) begin
                        sampled_alpha = pick_byte(ddram_dout_i, src_pixel_byte_addr[2:0]);
                        // Tint always applies for A8 (texture is alpha-only).
                        pixel_data <= pack_pixel(
                            mul8(ch_r(tint_color_q), sampled_alpha),
                            mul8(ch_g(tint_color_q), sampled_alpha),
                            mul8(ch_b(tint_color_q), sampled_alpha),
                            sampled_alpha
                        );
                    end else begin
                        src_word = pick_word(ddram_dout_i, src_pixel_byte_addr[2]);
                        if (tint_en_q) begin
                            pixel_data <= pack_pixel(
                                mul8(ch_r(src_word), ch_r(tint_color_q)),
                                mul8(ch_g(src_word), ch_g(tint_color_q)),
                                mul8(ch_b(src_word), ch_b(tint_color_q)),
                                mul8(ch_a(src_word), ch_a(tint_color_q))
                            );
                        end else begin
                            pixel_data <= src_word;
                        end
                    end
                    state <= S_WRITE;
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
