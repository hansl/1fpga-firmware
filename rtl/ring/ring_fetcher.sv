//============================================================================
//
//  Command ring fetcher (M2b + M2c1 + M2c3.1).
//
//  SPSC ring per PROTOCOL.md §4. We retire commands in order, advancing
//  RING_HEAD as we go.
//
//  Supported opcodes:
//    NOP       (0x00) — advance HEAD by 4 + length_w*4
//    PRESENT   (0x01) — pulse fb_swapper, advance HEAD
//    FENCE     (0x02) — fetch 1 arg word, write to FENCE_VALUE
//    FILL_RECT (0x10) — fetch 3 arg words, dispatch blit (FILL mode)
//    COPY_RECT (0x11) — fetch 5 arg words + 4 descriptor words at
//                       TEX_TABLE_ADDR + tex_id * 32, dispatch blit
//                       (COPY mode). M2c3.1 supports the simplest
//                       sub-form only: 1:1 scale, RGBA8888, opaque
//                       blend, no tint. Tint flag is ignored if set.
//
//  SET_CLIP / CLEAR_CLIP and unrecognised opcodes trip
//  ERR_UNKNOWN_OPCODE and halt; M2c2+ adds them.
//
//  Address mapping for DDRAM_*:
//    DDRAM_ADDR = byte_addr >> 3  (29-bit word-address, 8-byte beats)
//    DDRAM_BE   selects upper or lower 4 bytes of the 64-bit beat
//    one 4-byte command word per DDR3 transaction (no bursting yet)
//
//============================================================================

module ring_fetcher (
    input  logic        clk,
    input  logic        rst_n,

    // Sideband to the register file.
    input  logic        enable_i,
    input  logic [31:0] ring_base_i,
    input  logic [31:0] ring_size_i,
    input  logic [31:0] ring_tail_i,
    input  logic [31:0] tex_table_addr_i,
    output logic [31:0] ring_head_o,
    output logic [31:0] fence_value_o,
    output logic [31:0] error_info_o,
    output logic        status_busy_o,
    output logic        status_error_o,

    // Triple-buffer dispatch.
    output logic        present_pulse_o,

    // Blit engine dispatch.
    output logic        blit_start_o,
    output logic        blit_mode_o,        // 0 = FILL, 1 = COPY
    output logic [1:0]  blit_blend_o,       // header.flags[1:0]
    output logic [15:0] blit_dst_x_o,
    output logic [15:0] blit_dst_y_o,
    output logic [15:0] blit_dst_w_o,
    output logic [15:0] blit_dst_h_o,
    output logic [31:0] blit_color_o,
    output logic [15:0] blit_src_x_o,
    output logic [15:0] blit_src_y_o,
    output logic [31:0] blit_src_addr_o,
    output logic [31:0] blit_src_pitch_o,
    output logic        blit_format_o,      // 0 = RGBA8888, 1 = A8
    output logic        blit_tint_en_o,
    output logic [31:0] blit_tint_color_o,
    output logic        blit_clip_en_o,
    output logic [15:0] blit_clip_x_o,
    output logic [15:0] blit_clip_y_o,
    output logic [15:0] blit_clip_w_o,
    output logic [15:0] blit_clip_h_o,
    output logic        blit_ignore_clip_o,
    input  logic        blit_done_i,

    // DDRAM_* read-master interface.
    output logic [28:0] ddram_addr_o,
    output logic [7:0]  ddram_burstcnt_o,
    output logic [7:0]  ddram_be_o,
    output logic        ddram_rd_o,
    input  logic        ddram_busy_i,
    input  logic [63:0] ddram_dout_i,
    input  logic        ddram_dout_valid_i
);

    // ---- Opcode table (PROTOCOL.md §5.2) -----------------------------
    localparam logic [7:0] OP_NOP        = 8'h00;
    localparam logic [7:0] OP_PRESENT    = 8'h01;
    localparam logic [7:0] OP_FENCE      = 8'h02;
    localparam logic [7:0] OP_SET_CLIP   = 8'h03;
    localparam logic [7:0] OP_CLEAR_CLIP = 8'h04;
    localparam logic [7:0] OP_FILL_RECT  = 8'h10;
    localparam logic [7:0] OP_COPY_RECT  = 8'h11;

    // ---- Error codes (PROTOCOL.md §8.1) ------------------------------
    localparam logic [7:0] ERR_UNKNOWN_OPCODE = 8'h01;

    // ---- Blit modes (matches blit_engine.sv) -------------------------
    localparam logic MODE_FILL = 1'b0;
    localparam logic MODE_COPY = 1'b1;

    typedef enum logic [3:0] {
        S_IDLE,
        S_FETCH_HEADER,
        S_WAIT_HEADER,
        S_DECODE,
        S_FETCH_ARG,
        S_WAIT_ARG,
        S_FETCH_DESC,
        S_WAIT_DESC,
        S_BLIT_DISPATCH,
        S_BLIT_WAIT,
        S_RETIRE,
        S_HALT
    } state_e;

    state_e      state;
    logic [31:0] head_q;
    logic [31:0] header_q;
    logic [31:0] arg_q  [0:5];       // up to 6 arg words (COPY_RECT + tint)
    logic [31:0] desc_q [0:3];       // 4 descriptor words for COPY_RECT
    logic [2:0]  arg_idx;
    logic [2:0]  arg_total;
    logic [1:0]  desc_idx;
    logic [31:0] fetch_addr;
    logic [31:0] retire_advance;
    logic [31:0] fence_value_q;
    logic [31:0] error_info_q;
    logic [7:0]  pending_opcode;
    // Persistent user clip state, updated on SET_CLIP / CLEAR_CLIP.
    logic        clip_en_q;
    logic [15:0] clip_x_q, clip_y_q, clip_w_q, clip_h_q;

    wire [31:0] head_mask = ring_size_i - 32'd1;

    function automatic logic [31:0] pick_word(input logic [63:0] beat,
                                              input logic        upper);
        return upper ? beat[63:32] : beat[31:0];
    endfunction

    function automatic logic [7:0] be_for(input logic upper);
        return upper ? 8'b1111_0000 : 8'b0000_1111;
    endfunction

    // ---- Combinational outputs ---------------------------------------
    assign ring_head_o     = head_q;
    assign fence_value_o   = fence_value_q;
    assign error_info_o    = error_info_q;
    assign status_busy_o   = (state != S_IDLE) & (state != S_HALT);
    assign status_error_o  = (state == S_HALT);
    assign present_pulse_o = (state == S_RETIRE) & (pending_opcode == OP_PRESENT);

    // Argument layouts diverge between FILL_RECT and COPY_RECT:
    //   FILL: arg[0]=dst.xy, arg[1]=dst.wh, arg[2]=color
    //   COPY: arg[0]=tex_id, arg[1]=src.xy, arg[2]=src.wh,
    //         arg[3]=dst.xy, arg[4]=dst.wh
    wire is_copy = (pending_opcode == OP_COPY_RECT);
    wire [31:0] dst_xy_word = is_copy ? arg_q[3] : arg_q[0];
    wire [31:0] dst_wh_word = is_copy ? arg_q[4] : arg_q[1];

    assign blit_start_o      = (state == S_BLIT_DISPATCH);
    assign blit_mode_o       = is_copy ? MODE_COPY : MODE_FILL;
    // Blend mode lives in header.flags[1:0] for both FILL_RECT (§5.3
    // #FILL_RECT) and COPY_RECT (§5.3 #COPY_RECT).
    assign blit_blend_o      = header_q[1:0];
    assign blit_dst_x_o      = dst_xy_word[31:16];
    assign blit_dst_y_o      = dst_xy_word[15:0];
    assign blit_dst_w_o      = dst_wh_word[31:16];
    assign blit_dst_h_o      = dst_wh_word[15:0];
    assign blit_color_o      = arg_q[2];      // only meaningful for FILL
    assign blit_src_x_o      = arg_q[1][31:16];
    assign blit_src_y_o      = arg_q[1][15:0];
    assign blit_src_addr_o   = desc_q[0];     // descriptor §6.1: data_addr
    assign blit_src_pitch_o  = desc_q[1];     // descriptor §6.1: pitch_bytes
    assign blit_format_o     = desc_q[3][0];  // descriptor §6.1 format byte: 0=RGBA, 1=A8
    // header.flags bit 4 = tint_en for COPY_RECT (PROTOCOL.md §5.3 #COPY_RECT).
    assign blit_tint_en_o    = is_copy & header_q[4];
    assign blit_tint_color_o = arg_q[5];      // optional 6th arg, valid only when tint_en
    // Clip state forwarded to the blit engine. ignore_clip is a
    // per-FILL_RECT flag (header bit 2); COPY_RECT always honours
    // the user clip rect.
    assign blit_clip_en_o    = clip_en_q;
    assign blit_clip_x_o     = clip_x_q;
    assign blit_clip_y_o     = clip_y_q;
    assign blit_clip_w_o     = clip_w_q;
    assign blit_clip_h_o     = clip_h_q;
    assign blit_ignore_clip_o = (pending_opcode == OP_FILL_RECT) & header_q[2];

    // Texture descriptor base = tex_table_addr + tex_id * 32.
    wire [31:0] desc_base = tex_table_addr_i + (arg_q[0] <<< 5);

    // ---- FSM transitions ---------------------------------------------
    integer i;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state          <= S_IDLE;
            head_q         <= 32'd0;
            header_q       <= 32'd0;
            for (i = 0; i < 6; i = i + 1) arg_q[i]  <= 32'd0;
            for (i = 0; i < 4; i = i + 1) desc_q[i] <= 32'd0;
            arg_idx        <= 3'd0;
            arg_total      <= 3'd0;
            desc_idx       <= 2'd0;
            fetch_addr     <= 32'd0;
            retire_advance <= 32'd0;
            fence_value_q  <= 32'd0;
            error_info_q   <= 32'd0;
            pending_opcode <= 8'd0;
            clip_en_q      <= 1'b0;
            clip_x_q       <= 16'd0;
            clip_y_q       <= 16'd0;
            clip_w_q       <= 16'd0;
            clip_h_q       <= 16'd0;
            ddram_addr_o     <= 29'd0;
            ddram_burstcnt_o <= 8'd0;
            ddram_be_o       <= 8'd0;
            ddram_rd_o       <= 1'b0;
        end else begin
            if (~ddram_busy_i) ddram_rd_o <= 1'b0;

            unique case (state)

                S_IDLE: begin
                    if (enable_i & (head_q != ring_tail_i)) begin
                        fetch_addr <= ring_base_i + head_q;
                        state      <= S_FETCH_HEADER;
                    end
                end

                S_FETCH_HEADER: begin
                    ddram_addr_o     <= fetch_addr[31:3];
                    ddram_burstcnt_o <= 8'd1;
                    ddram_be_o       <= be_for(fetch_addr[2]);
                    ddram_rd_o       <= 1'b1;
                    if (~ddram_busy_i) state <= S_WAIT_HEADER;
                end

                S_WAIT_HEADER: if (ddram_dout_valid_i) begin
                    header_q <= pick_word(ddram_dout_i, fetch_addr[2]);
                    state    <= S_DECODE;
                end

                S_DECODE: begin
                    automatic logic [7:0] opcode = header_q[31:24];
                    automatic logic [7:0] len_w  = header_q[23:16];
                    retire_advance <= 32'd4 + (32'(len_w) <<< 2);
                    pending_opcode <= opcode;
                    arg_idx        <= 3'd0;
                    desc_idx       <= 2'd0;

                    unique case (opcode)
                        OP_NOP, OP_PRESENT, OP_CLEAR_CLIP: begin
                            arg_total <= 3'd0;
                            state     <= S_RETIRE;
                        end
                        OP_FENCE: begin
                            arg_total  <= 3'd1;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_SET_CLIP: begin
                            arg_total  <= 3'd2;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_FILL_RECT: begin
                            arg_total  <= 3'd3;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_COPY_RECT: begin
                            // length_w distinguishes 5 (no tint) vs 6
                            // (tint_en) per PROTOCOL.md §5.3.
                            arg_total  <= (header_q[23:16] == 8'd6) ? 3'd6 : 3'd5;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        default: begin
                            error_info_q <= {24'd0, ERR_UNKNOWN_OPCODE};
                            state        <= S_HALT;
                        end
                    endcase
                end

                S_FETCH_ARG: begin
                    ddram_addr_o     <= fetch_addr[31:3];
                    ddram_burstcnt_o <= 8'd1;
                    ddram_be_o       <= be_for(fetch_addr[2]);
                    ddram_rd_o       <= 1'b1;
                    if (~ddram_busy_i) state <= S_WAIT_ARG;
                end

                S_WAIT_ARG: if (ddram_dout_valid_i) begin
                    arg_q[arg_idx] <= pick_word(ddram_dout_i, fetch_addr[2]);
                    if (arg_idx + 3'd1 == arg_total) begin
                        // All args fetched. COPY_RECT also needs the
                        // texture descriptor; everything else dispatches
                        // straight to retire / blit.
                        unique case (pending_opcode)
                            OP_FILL_RECT: state <= S_BLIT_DISPATCH;
                            OP_COPY_RECT: begin
                                // desc_base uses arg_q[0] = tex_id.
                                // arg_q[0] was just written this cycle
                                // (non-blocking) so its new value isn't
                                // visible until next cycle; transition
                                // to S_FETCH_DESC and let it pick up
                                // the address combinationally.
                                state <= S_FETCH_DESC;
                            end
                            default: state <= S_RETIRE;
                        endcase
                    end else begin
                        arg_idx    <= arg_idx + 3'd1;
                        fetch_addr <= fetch_addr + 32'd4;
                        state      <= S_FETCH_ARG;
                    end
                end

                S_FETCH_DESC: begin
                    automatic logic [31:0] desc_word_addr =
                        desc_base + ({30'd0, desc_idx} <<< 2);
                    fetch_addr       <= desc_word_addr;
                    ddram_addr_o     <= desc_word_addr[31:3];
                    ddram_burstcnt_o <= 8'd1;
                    ddram_be_o       <= be_for(desc_word_addr[2]);
                    ddram_rd_o       <= 1'b1;
                    if (~ddram_busy_i) state <= S_WAIT_DESC;
                end

                S_WAIT_DESC: if (ddram_dout_valid_i) begin
                    desc_q[desc_idx] <= pick_word(ddram_dout_i, fetch_addr[2]);
                    if (desc_idx == 2'd3) begin
                        state <= S_BLIT_DISPATCH;
                    end else begin
                        desc_idx <= desc_idx + 2'd1;
                        state    <= S_FETCH_DESC;
                    end
                end

                S_BLIT_DISPATCH: state <= S_BLIT_WAIT;

                S_BLIT_WAIT: if (blit_done_i) state <= S_RETIRE;

                S_RETIRE: begin
                    unique case (pending_opcode)
                        OP_FENCE: fence_value_q <= arg_q[0];
                        OP_SET_CLIP: begin
                            // Word 0: x|y, Word 1: w|h (PROTOCOL.md §5.3).
                            clip_x_q  <= arg_q[0][31:16];
                            clip_y_q  <= arg_q[0][15:0];
                            clip_w_q  <= arg_q[1][31:16];
                            clip_h_q  <= arg_q[1][15:0];
                            clip_en_q <= 1'b1;
                        end
                        OP_CLEAR_CLIP: clip_en_q <= 1'b0;
                        default:  ;
                    endcase

                    head_q <= (head_q + retire_advance) & head_mask;
                    state  <= S_IDLE;
                end

                S_HALT: ;

            endcase
        end
    end

endmodule
