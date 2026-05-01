//============================================================================
//
//  LW_H2F bridge wrapper.
//
//  Instantiates the Cyclone V HPS hard-IP "lightweight HPS-to-FPGA" bridge
//  primitive (`cyclonev_hps_interface_hps2fpga_light_weight`) and converts
//  its AXI3 master into a small single-beat read/write request interface
//  for consumption by `menu_core_regs`.
//
//  The MiSTer framework in `sys/` does not instantiate this primitive, so
//  we own it entirely. Address window: 0xFF200000-0xFF3FFFFF (2 MiB).
//
//  Linux user-space `/dev/mem` mmap accesses produce single-beat 32-bit
//  transactions with arlen=awlen=0 — that is exactly what we serve. We
//  always reply OKAY (resp=2'b00); we do not honour multi-beat bursts
//  (rlast is tied to 1 on every read response, no pipelined writes).
//
//============================================================================

module lwh2f_bridge (
    input  logic        clk,
    input  logic        rst_n,

    // Single-beat request interface to the register slave.
    output logic [20:0] req_addr,
    output logic        req_read,        // pulsed for one cycle on read
    output logic        req_write,       // pulsed for one cycle on write
    output logic [31:0] req_writedata,
    output logic [3:0]  req_byteenable,
    input  logic [31:0] req_readdata     // combinational from slave
);

    // ---- Internal AXI3 wires -----------------------------------------
    logic        ar_ready, aw_ready, b_valid, r_last, r_valid, w_ready;
    logic [11:0] b_id, r_id;
    logic [1:0]  b_resp, r_resp;
    logic [31:0] r_data;

    logic        ar_valid, aw_valid, b_ready, r_ready, w_last, w_valid;
    logic [20:0] ar_addr, aw_addr;
    logic [1:0]  ar_burst, ar_lock, aw_burst, aw_lock;
    logic [3:0]  ar_cache, ar_len, aw_cache, aw_len, w_strb;
    logic [11:0] ar_id, aw_id, w_id;
    logic [2:0]  ar_prot, ar_size, aw_prot, aw_size;
    logic [31:0] w_data;

    cyclonev_hps_interface_hps2fpga_light_weight lwh2f (
        .clk        (clk),

        .arvalid    (ar_valid),
        .arready    (ar_ready),
        .araddr     (ar_addr),
        .arid       (ar_id),
        .arlen      (ar_len),
        .arsize     (ar_size),
        .arburst    (ar_burst),
        .arlock     (ar_lock),
        .arcache    (ar_cache),
        .arprot     (ar_prot),

        .rvalid     (r_valid),
        .rready     (r_ready),
        .rdata      (r_data),
        .rid        (r_id),
        .rresp      (r_resp),
        .rlast      (r_last),

        .awvalid    (aw_valid),
        .awready    (aw_ready),
        .awaddr     (aw_addr),
        .awid       (aw_id),
        .awlen      (aw_len),
        .awsize     (aw_size),
        .awburst    (aw_burst),
        .awlock     (aw_lock),
        .awcache    (aw_cache),
        .awprot     (aw_prot),

        .wvalid     (w_valid),
        .wready     (w_ready),
        .wdata      (w_data),
        .wid        (w_id),
        .wstrb      (w_strb),
        .wlast      (w_last),

        .bvalid     (b_valid),
        .bready     (b_ready),
        .bid        (b_id),
        .bresp      (b_resp)
    );

    // ---- Read FSM -----------------------------------------------------
    // R_IDLE: arready high, waiting for AR.
    // R_RESP: rvalid high, presenting captured address to the slave.
    typedef enum logic { R_IDLE, R_RESP } r_state_e;
    r_state_e    r_state;
    logic [20:0] r_addr_q;
    logic [11:0] r_id_q;

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            r_state  <= R_IDLE;
            r_addr_q <= '0;
            r_id_q   <= '0;
        end else begin
            unique case (r_state)
                R_IDLE: if (ar_valid) begin
                    r_addr_q <= ar_addr;
                    r_id_q   <= ar_id;
                    r_state  <= R_RESP;
                end
                R_RESP: if (r_ready) begin
                    r_state <= R_IDLE;
                end
            endcase
        end
    end

    assign ar_ready = (r_state == R_IDLE);
    assign r_valid  = (r_state == R_RESP);
    assign r_data   = req_readdata;
    assign r_id     = r_id_q;
    assign r_resp   = 2'b00;
    assign r_last   = 1'b1;

    // ---- Write FSM ----------------------------------------------------
    // W_IDLE: latch AW and W as they arrive (in any order).
    // W_DO:   present captured address+data to slave for one cycle.
    // W_RESP: hold bvalid until accepted.
    typedef enum logic [1:0] { W_IDLE, W_DO, W_RESP } w_state_e;
    w_state_e    w_state;
    logic        aw_seen, w_seen;
    logic [20:0] w_addr_q;
    logic [31:0] w_data_q;
    logic [3:0]  w_strb_q;
    logic [11:0] w_id_q;

    wire aw_take = aw_valid & aw_ready;
    wire w_take  = w_valid  & w_ready;

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            w_state  <= W_IDLE;
            aw_seen  <= 1'b0;
            w_seen   <= 1'b0;
            w_addr_q <= '0;
            w_data_q <= '0;
            w_strb_q <= '0;
            w_id_q   <= '0;
        end else begin
            unique case (w_state)
                W_IDLE: begin
                    if (aw_take) begin
                        w_addr_q <= aw_addr;
                        w_id_q   <= aw_id;
                        aw_seen  <= 1'b1;
                    end
                    if (w_take) begin
                        w_data_q <= w_data;
                        w_strb_q <= w_strb;
                        w_seen   <= 1'b1;
                    end
                    if ((aw_seen | aw_take) & (w_seen | w_take)) begin
                        w_state <= W_DO;
                    end
                end
                W_DO: if (writing) w_state <= W_RESP;
                W_RESP: if (b_ready) begin
                    w_state <= W_IDLE;
                    aw_seen <= 1'b0;
                    w_seen  <= 1'b0;
                end
            endcase
        end
    end

    assign aw_ready = (w_state == W_IDLE) & ~aw_seen;
    assign w_ready  = (w_state == W_IDLE) & ~w_seen;
    assign b_valid  = (w_state == W_RESP);
    assign b_id     = w_id_q;
    assign b_resp   = 2'b00;

    // ---- Slave request multiplexing ----------------------------------
    // Reads have priority on the shared address bus; if a write fires
    // in the same cycle as a read response, the write waits one cycle.
    wire reading = (r_state == R_RESP);
    wire writing = (w_state == W_DO) & ~reading;

    assign req_read       = reading;
    assign req_write      = writing;
    assign req_addr       = reading ? r_addr_q : w_addr_q;
    assign req_writedata  = w_data_q;
    assign req_byteenable = w_strb_q;

endmodule
