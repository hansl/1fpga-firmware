//============================================================================
//
//  Command ring fetcher (M2b + M2c1).
//
//  Single-producer/single-consumer ring per PROTOCOL.md §4. The host
//  writes commands at byte offset RING_TAIL within a ring of RING_SIZE
//  bytes anchored at RING_BASE (HPS-physical, must lie inside the
//  reserved DDR3 carve-out at 0x3000_0000). We retire commands in
//  order, advancing RING_HEAD as we go.
//
//  Supported opcodes:
//    NOP       (0x00) — advance HEAD by 4 + length_w*4
//    PRESENT   (0x01) — bump FRAME_COUNT, advance HEAD
//    FENCE     (0x02) — fetch 1 arg word, write to FENCE_VALUE
//    FILL_RECT (0x10) — fetch 3 arg words, dispatch to blit engine
//
//  Drawing opcodes other than FILL_RECT (COPY_RECT, SET_CLIP,
//  CLEAR_CLIP) trip ERR_UNKNOWN_OPCODE and halt; M2c2+ adds them.
//
//  Address mapping for DDRAM_*:
//    DDRAM_ADDR = byte_addr >> 3  (29-bit word-address, 8-byte beats)
//    DDRAM_BE   = 8'b1111_0000  if byte_addr[2] = 1  (upper word)
//                 8'b0000_1111  if byte_addr[2] = 0  (lower word)
//    DDRAM_DOUT[31:0]  is the lower-half word
//    DDRAM_DOUT[63:32] is the upper-half word
//
//  One 4-byte command word per DDR3 transaction: simple, slow but
//  unambiguously correct. Bursting is M2c5+ work.
//
//============================================================================

module ring_fetcher (
    input  logic        clk,
    input  logic        rst_n,

    // Sideband to the register file.
    input  logic        enable_i,         // CONTROL.EN
    input  logic [31:0] ring_base_i,      // RING_BASE
    input  logic [31:0] ring_size_i,      // RING_SIZE (power of 2)
    input  logic [31:0] ring_tail_i,      // RING_TAIL
    output logic [31:0] ring_head_o,      // RING_HEAD (visible to host)
    output logic [31:0] fence_value_o,    // FENCE_VALUE
    output logic [31:0] frame_count_o,    // FRAME_COUNT
    output logic [31:0] error_info_o,     // ERROR_INFO
    output logic        status_busy_o,    // STATUS.BZ
    output logic        status_error_o,   // STATUS.ER

    // Blit engine dispatch.
    output logic        blit_start_o,
    output logic [15:0] blit_dst_x_o,
    output logic [15:0] blit_dst_y_o,
    output logic [15:0] blit_dst_w_o,
    output logic [15:0] blit_dst_h_o,
    output logic [31:0] blit_color_o,
    input  logic        blit_done_i,

    // DDRAM_* read-master interface (the top-level mux owns the actual
    // DDRAM_* pins; the blit engine drives them while busy).
    output logic [28:0] ddram_addr_o,
    output logic [7:0]  ddram_burstcnt_o,
    output logic [7:0]  ddram_be_o,
    output logic        ddram_rd_o,
    input  logic        ddram_busy_i,
    input  logic [63:0] ddram_dout_i,
    input  logic        ddram_dout_valid_i
);

    // ---- Opcode table (PROTOCOL.md §5.2) -----------------------------
    localparam logic [7:0] OP_NOP       = 8'h00;
    localparam logic [7:0] OP_PRESENT   = 8'h01;
    localparam logic [7:0] OP_FENCE     = 8'h02;
    localparam logic [7:0] OP_FILL_RECT = 8'h10;

    // ---- Error codes (PROTOCOL.md §8.1) ------------------------------
    localparam logic [7:0] ERR_UNKNOWN_OPCODE  = 8'h01;

    // ---- FSM ---------------------------------------------------------
    typedef enum logic [3:0] {
        S_IDLE,
        S_FETCH_HEADER,
        S_WAIT_HEADER,
        S_DECODE,
        S_FETCH_ARG,
        S_WAIT_ARG,
        S_BLIT_DISPATCH,
        S_BLIT_WAIT,
        S_RETIRE,
        S_HALT
    } state_e;

    state_e      state;
    logic [31:0] head_q;
    logic [31:0] header_q;
    logic [31:0] arg_q [0:2];        // up to 3 arg words (FILL_RECT)
    logic [1:0]  arg_idx;            // current arg being fetched (0..2)
    logic [1:0]  arg_total;          // number of args needed (1 for FENCE, 3 for FILL_RECT)
    logic [31:0] fetch_addr;
    logic [31:0] retire_advance;
    logic [31:0] fence_value_q;
    logic [31:0] frame_count_q;
    logic [31:0] error_info_q;
    logic [7:0]  pending_opcode;     // latched at S_DECODE for the dispatch decision

    wire [31:0] head_mask = ring_size_i - 32'd1;

    function automatic logic [31:0] pick_word(input logic [63:0] beat,
                                              input logic        upper);
        return upper ? beat[63:32] : beat[31:0];
    endfunction

    function automatic logic [7:0] be_for(input logic upper);
        return upper ? 8'b1111_0000 : 8'b0000_1111;
    endfunction

    // ---- Combinational outputs ---------------------------------------
    assign ring_head_o    = head_q;
    assign fence_value_o  = fence_value_q;
    assign frame_count_o  = frame_count_q;
    assign error_info_o   = error_info_q;
    assign status_busy_o  = (state != S_IDLE) & (state != S_HALT);
    assign status_error_o = (state == S_HALT);

    // Blit engine dispatch outputs are valid only during S_BLIT_DISPATCH;
    // start_o is asserted for one cycle as we transition to S_BLIT_WAIT.
    assign blit_start_o = (state == S_BLIT_DISPATCH);
    assign blit_dst_x_o = arg_q[0][31:16];
    assign blit_dst_y_o = arg_q[0][15:0];
    assign blit_dst_w_o = arg_q[1][31:16];
    assign blit_dst_h_o = arg_q[1][15:0];
    assign blit_color_o = arg_q[2];

    // ---- FSM transitions ---------------------------------------------
    integer i;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state          <= S_IDLE;
            head_q         <= 32'd0;
            header_q       <= 32'd0;
            for (i = 0; i < 3; i = i + 1) arg_q[i] <= 32'd0;
            arg_idx        <= 2'd0;
            arg_total      <= 2'd0;
            fetch_addr     <= 32'd0;
            retire_advance <= 32'd0;
            fence_value_q  <= 32'd0;
            frame_count_q  <= 32'd0;
            error_info_q   <= 32'd0;
            pending_opcode <= 8'd0;
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
                    arg_idx        <= 2'd0;

                    unique case (opcode)
                        OP_NOP, OP_PRESENT: begin
                            arg_total <= 2'd0;
                            state     <= S_RETIRE;
                        end
                        OP_FENCE: begin
                            arg_total  <= 2'd1;
                            fetch_addr <= fetch_addr + 32'd4;
                            state      <= S_FETCH_ARG;
                        end
                        OP_FILL_RECT: begin
                            arg_total  <= 2'd3;
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
                    if (arg_idx + 2'd1 == arg_total) begin
                        // All args fetched — dispatch.
                        if (pending_opcode == OP_FILL_RECT) begin
                            state <= S_BLIT_DISPATCH;
                        end else begin
                            state <= S_RETIRE;
                        end
                    end else begin
                        arg_idx    <= arg_idx + 2'd1;
                        fetch_addr <= fetch_addr + 32'd4;
                        state      <= S_FETCH_ARG;
                    end
                end

                S_BLIT_DISPATCH: begin
                    // Pulse blit_start_o (combinational) and move on.
                    state <= S_BLIT_WAIT;
                end

                S_BLIT_WAIT: if (blit_done_i) begin
                    state <= S_RETIRE;
                end

                S_RETIRE: begin
                    unique case (pending_opcode)
                        OP_PRESENT: frame_count_q <= frame_count_q + 32'd1;
                        OP_FENCE:   fence_value_q <= arg_q[0];
                        default:    ;        // NOP / FILL_RECT — no reg update here
                    endcase

                    head_q <= (head_q + retire_advance) & head_mask;
                    state  <= S_IDLE;
                end

                S_HALT: begin
                    // Stay halted until host clears the error via CONTROL.CE.
                end

            endcase
        end
    end

endmodule
