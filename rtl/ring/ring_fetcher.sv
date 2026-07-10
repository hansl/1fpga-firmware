//============================================================================
//
//  Command ring fetcher (M2b + M2c1 + M2c3.1).
//
//  SPSC ring per PROTOCOL.md §4. We retire commands in order, advancing
//  RING_HEAD as we go.
//
//  Supported opcodes:
//    NOP             (0x00) — advance HEAD by 4 + length_w*4
//    PRESENT         (0x01) — pulse fb_swapper, advance HEAD
//    FENCE           (0x02) — fetch 1 arg word, write to FENCE_VALUE
//    SET_CLIP        (0x03) — fetch 2 arg words, latch user clip
//    CLEAR_CLIP      (0x04) — disable user clip
//    SET_RENDER_TGT  (0x05) — fetch 1 arg word (tex_id). 0xFFFF = FB;
//                             else fetch descriptor and latch
//                             active target (PROTOCOL.md §5.6).
//    FILL_RECT       (0x10) — fetch 3 arg words, dispatch blit (FILL)
//    COPY_RECT       (0x11) — fetch 5 arg words + 4 descriptor words,
//                             dispatch blit (COPY mode).
//
//  Active render target: defaults to the framebuffer. SET_RENDER_TGT
//  re-points the blit engine's destination at a texture's pixel data.
//  Selection persists across commands until another SET_RENDER_TGT.
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
    // Default render target = framebuffer geometry sourced from regs.
    input  logic [31:0] fb_base_i,
    input  logic [31:0] fb_stride_i,
    input  logic [15:0] fb_width_i,
    input  logic [15:0] fb_height_i,
    output logic [31:0] ring_head_o,
    output logic [31:0] fence_value_o,
    output logic [31:0] error_info_o,
    output logic        status_busy_o,
    output logic        status_error_o,

    // Triple-buffer dispatch.
    output logic        present_pulse_o,

    // Active render target (combined: framebuffer when no RTT active,
    // texture's data_addr / pitch / dims after SET_RENDER_TARGET).
    output logic [31:0] target_base_o,
    output logic [31:0] target_pitch_o,
    output logic [15:0] target_width_o,
    output logic [15:0] target_height_o,

    // Blit engine dispatch — two engines on independent DDR3 ports.
    // active_engine_q flips on each OP_PRESENT so frames N and N+1
    // run on different engines and their DDR3 traffic overlaps.
    output logic        blit0_start_o,      // engine 0 (ram1)
    output logic        blit1_start_o,      // engine 1 (ram2)
    input  logic        blit0_done_i,
    input  logic        blit1_done_i,
    output logic [1:0]  blit_mode_o,        // 0 = FILL, 1 = COPY, 2 = AFFINE
    output logic [1:0]  blit_blend_o,       // header.flags[1:0]
    output logic [15:0] blit_dst_x_o,
    output logic [15:0] blit_dst_y_o,
    output logic [15:0] blit_dst_w_o,
    output logic [15:0] blit_dst_h_o,
    output logic [31:0] blit_color_o,
    output logic [15:0] blit_src_x_o,
    output logic [15:0] blit_src_y_o,
    output logic [15:0] blit_src_w_o,
    output logic [15:0] blit_src_h_o,
    output logic [31:0] blit_src_addr_o,
    output logic [31:0] blit_src_pitch_o,
    output logic        blit_format_o,      // 0 = RGBA8888, 1 = A8
    output logic        blit_tint_en_o,
    output logic [31:0] blit_tint_color_o,
    // AFFINE inverse 2x3 matrix (Q16.16), valid when blit_mode_o == 2.
    output logic [31:0] blit_aff_m00_o,
    output logic [31:0] blit_aff_m01_o,
    output logic [31:0] blit_aff_m10_o,
    output logic [31:0] blit_aff_m11_o,
    output logic [31:0] blit_aff_tx_o,
    output logic [31:0] blit_aff_ty_o,
    output logic        blit_clip_en_o,
    output logic [15:0] blit_clip_x_o,
    output logic [15:0] blit_clip_y_o,
    output logic [15:0] blit_clip_w_o,
    output logic [15:0] blit_clip_h_o,
    output logic        blit_ignore_clip_o,

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
    localparam logic [7:0] OP_SET_TARGET = 8'h05;
    localparam logic [7:0] OP_FILL_RECT  = 8'h10;
    localparam logic [7:0] OP_COPY_RECT  = 8'h11;
    localparam logic [7:0] OP_BLIT_AFFINE = 8'h12;

    // SET_RENDER_TARGET: tex_id == 0xFFFF means "framebuffer".
    localparam logic [15:0] TARGET_FB     = 16'hFFFF;

    // ---- Error codes (PROTOCOL.md §8.1) ------------------------------
    localparam logic [7:0] ERR_UNKNOWN_OPCODE   = 8'h01;
    localparam logic [7:0] ERR_BAD_LENGTH       = 8'h02;
    localparam logic [7:0] ERR_BAD_FORMAT       = 8'h04;
    localparam logic [7:0] ERR_AFFINE_TOO_LARGE = 8'h09;

    // ---- Blit modes (matches blit_engine.sv) -------------------------
    localparam logic [1:0] MODE_FILL   = 2'd0;
    localparam logic [1:0] MODE_COPY   = 2'd1;
    localparam logic [1:0] MODE_AFFINE = 2'd2;

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
    logic        active_engine_q;        // 0 = blit_engine_0 (ram1), 1 = blit_engine_1 (ram2)
    logic [31:0] head_q;
    logic [31:0] header_q;
    logic [31:0] arg_q  [0:10];      // up to 11 arg words (BLIT_AFFINE)
    logic [31:0] desc_q [0:3];       // 4 descriptor words for COPY_RECT
    logic [3:0]  arg_idx;
    logic [3:0]  arg_total;
    logic [1:0]  desc_idx;
    logic [31:0] fetch_addr;
    logic [31:0] retire_advance;
    logic [31:0] fence_value_q;
    logic [31:0] error_info_q;
    logic [7:0]  pending_opcode;
    // Persistent user clip state, updated on SET_CLIP / CLEAR_CLIP.
    logic        clip_en_q;
    logic [15:0] clip_x_q, clip_y_q, clip_w_q, clip_h_q;

    // Active render target (PROTOCOL.md §5.6). Default = FB.
    // When target_is_fb_q is high, the target_*_o outputs come from
    // the FB geometry inputs. Otherwise they come from the latched
    // texture-descriptor fields.
    logic        target_is_fb_q;
    logic [31:0] target_base_q;
    logic [31:0] target_pitch_q;
    logic [15:0] target_width_q;
    logic [15:0] target_height_q;

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
    wire is_affine = (pending_opcode == OP_BLIT_AFFINE);
    // COPY_RECT and BLIT_AFFINE share the base layout: dst.xy = arg[3],
    // dst.wh = arg[4], src.xy = arg[1], src.wh = arg[2].
    wire is_copy_or_aff = is_copy | is_affine;
    wire [31:0] dst_xy_word = is_copy_or_aff ? arg_q[3] : arg_q[0];
    wire [31:0] dst_wh_word = is_copy_or_aff ? arg_q[4] : arg_q[1];

    // Dual blit dispatch. The current frame's blits go to the engine
    // selected by active_engine_q. active_engine_q flips on PRESENT
    // (in S_RETIRE below).
    assign blit0_start_o     = (state == S_BLIT_DISPATCH) & ~active_engine_q;
    assign blit1_start_o     = (state == S_BLIT_DISPATCH) &  active_engine_q;
    wire   blit_done_mux     = active_engine_q ? blit1_done_i : blit0_done_i;
    assign blit_mode_o       = is_affine ? MODE_AFFINE
                             : is_copy   ? MODE_COPY
                             :             MODE_FILL;
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
    // src.wh is arg[2] per PROTOCOL.md §5.3 #COPY_RECT. Driven for
    // FILL_RECT too but blit_engine ignores it when mode == FILL.
    assign blit_src_w_o      = arg_q[2][31:16];
    assign blit_src_h_o      = arg_q[2][15:0];
    assign blit_src_addr_o   = desc_q[0];     // descriptor §6.1: data_addr
    assign blit_src_pitch_o  = desc_q[1];     // descriptor §6.1: pitch_bytes
    assign blit_format_o     = desc_q[3][0];  // descriptor §6.1 format byte: 0=RGBA, 1=A8
    // header.flags bit 4 = tint_en for COPY_RECT (PROTOCOL.md §5.3 #COPY_RECT).
    assign blit_tint_en_o    = is_copy & header_q[4];
    assign blit_tint_color_o = arg_q[5];      // optional 6th arg, valid only when tint_en
    // AFFINE matrix words 5..10 (BLIT_AFFINE only).
    assign blit_aff_m00_o    = arg_q[5];
    assign blit_aff_m01_o    = arg_q[6];
    assign blit_aff_m10_o    = arg_q[7];
    assign blit_aff_m11_o    = arg_q[8];
    assign blit_aff_tx_o     = arg_q[9];
    assign blit_aff_ty_o     = arg_q[10];
    // Clip state forwarded to the blit engine. ignore_clip is a
    // per-FILL_RECT flag (header bit 2); COPY_RECT always honours
    // the user clip rect.
    assign blit_clip_en_o    = clip_en_q;
    assign blit_clip_x_o     = clip_x_q;
    assign blit_clip_y_o     = clip_y_q;
    assign blit_clip_w_o     = clip_w_q;
    assign blit_clip_h_o     = clip_h_q;
    assign blit_ignore_clip_o = (pending_opcode == OP_FILL_RECT) & header_q[2];

    // Active target mux.
    assign target_base_o   = target_is_fb_q ? fb_base_i   : target_base_q;
    assign target_pitch_o  = target_is_fb_q ? fb_stride_i : target_pitch_q;
    assign target_width_o  = target_is_fb_q ? fb_width_i  : target_width_q;
    assign target_height_o = target_is_fb_q ? fb_height_i : target_height_q;

    // Texture descriptor base = tex_table_addr + tex_id * 32. tex_id
    // is the LOW HALF of the word only — the sentinel checks already
    // use [15:0], and an unmasked shift would let stray upper bits
    // (future flags, host bugs) fetch a descriptor from a wild
    // address and blit garbage.
    wire [31:0] desc_base = tex_table_addr_i + ({16'd0, arg_q[0][15:0]} <<< 5);

    // ---- FSM transitions ---------------------------------------------
    integer i;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state          <= S_IDLE;
            active_engine_q <= 1'b0;
            head_q         <= 32'd0;
            header_q       <= 32'd0;
            for (i = 0; i < 11; i = i + 1) arg_q[i]  <= 32'd0;
            for (i = 0; i < 4; i = i + 1) desc_q[i] <= 32'd0;
            arg_idx        <= 4'd0;
            arg_total      <= 4'd0;
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
            target_is_fb_q   <= 1'b1;
            target_base_q    <= 32'd0;
            target_pitch_q   <= 32'd0;
            target_width_q   <= 16'd0;
            target_height_q  <= 16'd0;
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
                    arg_idx        <= 4'd0;
                    desc_idx       <= 2'd0;

                    unique case (opcode)
                        OP_NOP, OP_PRESENT, OP_CLEAR_CLIP: begin
                            arg_total <= 4'd0;
                            state     <= S_RETIRE;
                        end
                        OP_FENCE: begin
                            arg_total  <= 4'd1;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_SET_CLIP: begin
                            arg_total  <= 4'd2;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_SET_TARGET: begin
                            arg_total  <= 4'd1;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_FILL_RECT: begin
                            arg_total  <= 4'd3;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_COPY_RECT: begin
                            // length_w distinguishes 5 (no tint) vs 6
                            // (tint_en) per PROTOCOL.md §5.3.
                            arg_total  <= (header_q[23:16] == 8'd6) ? 4'd6 : 4'd5;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_BLIT_AFFINE: begin
                            // Fixed 11-word layout (PROTOCOL.md §5.3).
                            // Strictly validate length_w — this is the
                            // first command with required length checking
                            // (§5.4). Detail packs (expected<<8)|got.
                            if (len_w != 8'd11) begin
                                error_info_q <= {8'd0, 8'd11, len_w, ERR_BAD_LENGTH};
                                state        <= S_HALT;
                            end else begin
                                arg_total  <= 4'd11;
                                fetch_addr <= fetch_addr + 32'd4;
                                state      <= S_FETCH_ARG;
                            end
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
                    automatic logic [31:0] arg_word;
                    arg_word = pick_word(ddram_dout_i, fetch_addr[2]);
                    arg_q[arg_idx] <= arg_word;
                    if (arg_idx + 4'd1 == arg_total) begin
                        // All args fetched. COPY_RECT, BLIT_AFFINE, and
                        // the texture-id form of SET_RENDER_TARGET also
                        // need the descriptor; FB-sentinel
                        // SET_RENDER_TARGET, FILL_RECT, etc. dispatch
                        // straight to retire / blit.
                        unique case (pending_opcode)
                            OP_FILL_RECT: state <= S_BLIT_DISPATCH;
                            OP_COPY_RECT, OP_BLIT_AFFINE: begin
                                // desc_base uses arg_q[0] = tex_id.
                                state <= S_FETCH_DESC;
                            end
                            OP_SET_TARGET: begin
                                // Just-fetched arg word holds tex_id
                                // in the low 16 bits.
                                if (arg_word[15:0] == TARGET_FB) begin
                                    state <= S_RETIRE;
                                end else begin
                                    state <= S_FETCH_DESC;
                                end
                            end
                            default: state <= S_RETIRE;
                        endcase
                    end else begin
                        arg_idx    <= arg_idx + 4'd1;
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
                    // All automatics declared up front (Quartus 17).
                    automatic logic [31:0] dword;
                    automatic logic [15:0] sw_a;
                    automatic logic [15:0] sh_a;
                    dword = pick_word(ddram_dout_i, fetch_addr[2]);
                    sw_a  = arg_q[2][31:16];
                    sh_a  = arg_q[2][15:0];
                    desc_q[desc_idx] <= dword;
                    if (desc_idx == 2'd3) begin
                        // dword is descriptor word 3 (format in bit 0).
                        // COPY_RECT continues into the blit pipeline;
                        // SET_RENDER_TARGET just retires (descriptor is
                        // latched in S_RETIRE); BLIT_AFFINE is guarded
                        // for source size (§5.7) and RGBA-only format.
                        if (pending_opcode == OP_SET_TARGET) begin
                            state <= S_RETIRE;
                        end else if (is_affine) begin
                            if (dword[0] != 1'b0) begin
                                // A8 source unsupported for affine.
                                error_info_q <= {16'd0, dword[7:0], ERR_BAD_FORMAT};
                                state        <= S_HALT;
                            end else if ((sw_a > 16'd128) || (sh_a > 16'd128)) begin
                                error_info_q <= {sw_a[11:0], sh_a[11:0], ERR_AFFINE_TOO_LARGE};
                                state        <= S_HALT;
                            end else begin
                                state <= S_BLIT_DISPATCH;
                            end
                        end else begin
                            state <= S_BLIT_DISPATCH;
                        end
                    end else begin
                        desc_idx <= desc_idx + 2'd1;
                        state    <= S_FETCH_DESC;
                    end
                end

                S_BLIT_DISPATCH: state <= S_BLIT_WAIT;

                S_BLIT_WAIT: if (blit_done_mux) state <= S_RETIRE;

                S_RETIRE: begin
                    unique case (pending_opcode)
                        OP_FENCE: fence_value_q <= arg_q[0];
                        OP_PRESENT: active_engine_q <= ~active_engine_q;
                        OP_SET_CLIP: begin
                            // Word 0: x|y, Word 1: w|h (PROTOCOL.md §5.3).
                            clip_x_q  <= arg_q[0][31:16];
                            clip_y_q  <= arg_q[0][15:0];
                            clip_w_q  <= arg_q[1][31:16];
                            clip_h_q  <= arg_q[1][15:0];
                            clip_en_q <= 1'b1;
                        end
                        OP_CLEAR_CLIP: clip_en_q <= 1'b0;
                        OP_SET_TARGET: begin
                            // arg[0][15:0] = tex_id; FB sentinel resets
                            // to the framebuffer geometry. Otherwise
                            // latch fields read from the descriptor:
                            //   desc[0] = data_addr
                            //   desc[1] = pitch_bytes
                            //   desc[2] = (height << 16) | width  (LE)
                            if (arg_q[0][15:0] == TARGET_FB) begin
                                target_is_fb_q <= 1'b1;
                            end else begin
                                target_is_fb_q  <= 1'b0;
                                target_base_q   <= desc_q[0];
                                target_pitch_q  <= desc_q[1];
                                target_width_q  <= desc_q[2][15:0];
                                target_height_q <= desc_q[2][31:16];
                            end
                        end
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
