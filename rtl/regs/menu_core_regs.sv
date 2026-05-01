//============================================================================
//
//  menu-core control register file (M2a).
//
//  Decodes accesses on the LW_H2F window mapped to PROTOCOL.md §3.1.
//  Register block base is 0xFF210000 from the host's perspective; this
//  module sees offsets within the 2 MiB LW_H2F window — we react only to
//  hits in the 0x10000..0x100FF range and otherwise return zero.
//
//  M2a behaviour:
//    - ID            (0x00) read-only constant 32'h1FFA_0001
//    - STATUS        (0x04) read-only constant 0
//    - CONTROL       (0x08) R/W; CONTROL[0] is exposed as `enable_o`
//                           for downstream modules (e.g. heartbeat)
//    - ERROR_INFO    (0x0C) read-only 0
//    - all other 4-byte slots in 0x00..0xFF: R/W scratch (default 0)
//
//  Real semantics for the remaining registers (RING_*, FB*_ADDR,
//  FENCE_VALUE, perf counters, etc.) are added in M2b/M2c. Treating them
//  as scratch in M2a lets the host probe drive a write/read pattern over
//  the entire window to validate the LW_H2F path end-to-end.
//
//============================================================================

module menu_core_regs (
    input  logic        clk,
    input  logic        rst_n,

    // Decoded request from `lwh2f_bridge`.
    input  logic [20:0] req_addr,
    input  logic        req_read,
    input  logic        req_write,
    input  logic [31:0] req_writedata,
    input  logic [3:0]  req_byteenable,
    output logic [31:0] req_readdata,

    // Sideband: software-controlled enable bit (CONTROL[0]).
    output logic        enable_o
);

    // The LW_H2F window is 2 MiB (21-bit address). Our register block
    // sits at host physical 0xFF210000, which is offset 0x10000 within
    // the window.
    localparam logic [20:0] BLOCK_BASE = 21'h10000;
    localparam logic [20:0] BLOCK_MASK = 21'hFFF00;     // top bits must match

    wire in_block = ((req_addr & BLOCK_MASK) == BLOCK_BASE);

    // Lower 8 bits index into the 64-word register file.
    wire [5:0] reg_idx = req_addr[7:2];

    // 64x32 scratch RAM holds R/W slots. Read-only or constant slots
    // override readdata/writedata behaviour below.
    logic [31:0] scratch [0:63];

    // ---- Decode read ---------------------------------------------------
    // Combinational so `lwh2f_bridge` can latch readdata in the same
    // cycle it asserts `req_read`.
    always_comb begin
        if (!in_block) begin
            req_readdata = 32'h0000_0000;
        end else begin
            unique case (reg_idx)
                6'h00:   req_readdata = 32'h1FFA_0001;          // ID
                6'h01:   req_readdata = 32'h0000_0000;          // STATUS
                6'h03:   req_readdata = 32'h0000_0000;          // ERROR_INFO
                default: req_readdata = scratch[reg_idx];
            endcase
        end
    end

    // ---- Decode write --------------------------------------------------
    // ID, STATUS, ERROR_INFO drop writes (read-only). All other slots
    // honour byte-enables. CONTROL takes a side-exit to drive
    // `enable_o`.
    integer i;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            for (i = 0; i < 64; i = i + 1) scratch[i] <= 32'b0;
        end else if (req_write & in_block) begin
            unique case (reg_idx)
                6'h00, 6'h01, 6'h03: ;   // ID, STATUS, ERROR_INFO: read-only
                default: begin
                    if (req_byteenable[0]) scratch[reg_idx][7:0]   <= req_writedata[7:0];
                    if (req_byteenable[1]) scratch[reg_idx][15:8]  <= req_writedata[15:8];
                    if (req_byteenable[2]) scratch[reg_idx][23:16] <= req_writedata[23:16];
                    if (req_byteenable[3]) scratch[reg_idx][31:24] <= req_writedata[31:24];
                end
            endcase
        end
    end

    // CONTROL register lives in scratch[6'h02]. Bit 0 is the enable.
    assign enable_o = scratch[6'h02][0];

    // Suppress "unused" warnings on always-true / unused inputs.
    wire _unused = &{1'b0, req_read, 1'b0};

endmodule
