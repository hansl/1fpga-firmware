//============================================================================
//
//  Scanout compositor — Phase A (single-layer DDR framebuffer scanout).
//
//  Replaces the framework's ASCAL block for the self-contained menu core.
//  Reads a BGRA8888 framebuffer straight from DDR3 over the `vbuf` 128-bit
//  Avalon-MM port (the one ASCAL used) and drives the HDMI pixel stream
//  (hdmi_d / hs / vs / de / vbl) at the true pixel clock, feeding the
//  framework's existing shadowmask → OSD → HDMI_TX tail untouched.
//
//  This is the foundation for the up-to-4-layer mask-gated compositor
//  (see /home/hansl/.claude/plans/...): Phase A proves the clk_hdmi
//  timing generator, the DDR read-ahead line-buffer pipeline, and the
//  HDMI tail integration with a SINGLE opaque layer (image identical to
//  today's MISTER_FB scanout). Phases B+ replicate the line reader,
//  add the per-layer coverage mask, and add the back-to-front blend.
//
//  Two clock domains:
//    * avl_clk  (clk_100m, 100 MHz) — Avalon read master + line-buffer
//      WRITE side. Fills whole scanlines into a ring of line buffers,
//      staying LINE_BUFS lines ahead of the beam (the read-ahead that
//      rides out HPS DDR3 contention — the historical scanout-jitter
//      source, cf. ASCAL's N_BURST=2048 deepening).
//    * hdmi_clk (148.5 MHz for 1080p60) — raster timing generator +
//      line-buffer READ side + pixel output.
//
//  Frame handshake: the hdmi side is the timing master. It emits a
//  per-frame `frame_start` toggle (at the top of vertical blank); this
//  crosses to avl_clk and resets the producer to line 0 so it can
//  prefill before active video begins. Line-level flow control uses
//  Gray-coded produced/consumed line counters across the boundary.
//
//  HSYNC / VSYNC polarity: CEA-861 1080p60 is positive on both.
//
//============================================================================

module compositor #(
    // CEA-861 1080p60 (pixel clock 148.5 MHz). Matches the framework's
    // default WIDTH/HFP/HS/HBP/HEIGHT/VFP/VS/VBP regs in sys_top.v.
    parameter int H_ACTIVE = 1920,
    parameter int H_FP     = 88,
    parameter int H_SYNC   = 44,
    parameter int H_BP     = 148,
    parameter int V_ACTIVE = 1080,
    parameter int V_FP     = 4,
    parameter int V_SYNC   = 5,
    parameter int V_BP     = 36,

    // Avalon-MM (vbuf) geometry.
    parameter int N_DW     = 128,          // data width
    parameter int N_AW     = 28,           // address width (16-byte words)
    parameter int BURST    = 128,          // beats per burst (<=255; ASCAL uses 128)

    // Number of whole-line buffers for DDR read-ahead. >=2; deeper rides
    // out DDR contention. 4 × (480×128b) ≈ 28 M10K.
    parameter int LINE_BUFS = 4,

    // Synthesis-time bring-up aid: 1 = ignore DDR, emit a colour-bar test
    // pattern (proves clk_hdmi + timing + HDMI tail in isolation from the
    // vbuf read path). 0 = real framebuffer scanout.
    // Colour bars confirmed on hardware (2026-06-09); now scanning out the
    // real DDR framebuffer.
    parameter bit TEST_PATTERN = 1'b0
) (
    // ---- HDMI pixel domain -------------------------------------------
    input  logic               hdmi_clk,
    input  logic               hdmi_rst_n,
    output logic [23:0]        hdmi_d,      // {R[23:16], G[15:8], B[7:0]}
    output logic               hdmi_hs,
    output logic               hdmi_vs,
    output logic               hdmi_de,
    output logic               hdmi_vbl,    // vertical blank (→ FB_VBL pacing)
    output logic               hdmi_brd,    // border (unused; tied 0)

    // ---- Avalon-MM read master (vbuf, clk_100m) ----------------------
    input  logic               avl_clk,
    input  logic               avl_rst_n,
    output logic [N_AW-1:0]    avl_address,    // 16-byte-word address
    output logic [7:0]         avl_burstcount,
    output logic               avl_read,
    input  logic               avl_waitrequest,
    input  logic [N_DW-1:0]    avl_readdata,
    input  logic               avl_readdatavalid,
    // write side unused — scanout is read-only.
    output logic               avl_write,
    output logic [N_DW-1:0]    avl_writedata,
    output logic [N_DW/8-1:0]  avl_byteenable,

    // ---- Layer 0 (content) framebuffer, avl_clk domain ---------------
    // Byte base address + byte stride, host-stable (the host only changes
    // these at PRESENT, between frames). Latched at frame_start.
    input  logic [31:0]        fb_base,
    input  logic [13:0]        fb_stride
);

    // ---- Derived timing ----------------------------------------------
    localparam int H_TOTAL = H_ACTIVE + H_FP + H_SYNC + H_BP; // 2200
    localparam int V_TOTAL = V_ACTIVE + V_FP + V_SYNC + V_BP; // 1125

    localparam int PIX_PER_WORD = N_DW / 32;                  // 4
    localparam int WORDS_PER_LINE = H_ACTIVE / PIX_PER_WORD;  // 480
    localparam int LBW = $clog2(WORDS_PER_LINE);              // line-buf word addr bits (9)
    localparam int LB_SLOT = 2**LBW;                          // slot stride (512, power-of-2)
    localparam int SLOTW = $clog2(LINE_BUFS);

    // write side never used; byteenable all-ones (full-word reads).
    assign avl_write     = 1'b0;
    assign avl_writedata = '0;
    assign avl_byteenable= '1;
    assign hdmi_brd      = 1'b0;

    // ================================================================
    //  Line buffers — a ring of LINE_BUFS × LB_SLOT × N_DW. Slot stride
    //  is a power of two so {slot,word} concatenation is a valid flat
    //  index (WORDS_PER_LINE=480 isn't a power of two; we waste the tail).
    //  Dual-clock simple dual-port: write port @avl_clk, read port @hdmi_clk.
    // ================================================================
    // no_rw_check: producer/consumer never touch the same slot at once
    // (line-level flow control guarantees it), so the dual-clock RAM needs
    // no read-during-write coherency logic.
    (* ramstyle = "no_rw_check, M10K" *)
    logic [N_DW-1:0] linebuf [LINE_BUFS*LB_SLOT-1:0];

    // write port
    logic                       lb_we;
    logic [SLOTW+LBW-1:0]       lb_waddr;
    logic [N_DW-1:0]            lb_wdata;
    // read port
    logic [SLOTW+LBW-1:0]       lb_raddr;
    logic [N_DW-1:0]            lb_rdata;

    always_ff @(posedge avl_clk) begin
        if (lb_we) linebuf[lb_waddr] <= lb_wdata;
    end
    always_ff @(posedge hdmi_clk) begin
        lb_rdata <= linebuf[lb_raddr];
    end

    // ================================================================
    //  HDMI timing generator (hdmi_clk domain)
    // ================================================================
    logic [11:0] hcount, vcount;
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin
            hcount <= '0;
            vcount <= '0;
        end else if (hcount == H_TOTAL-1) begin
            hcount <= '0;
            vcount <= (vcount == V_TOTAL-1) ? 12'd0 : vcount + 12'd1;
        end else begin
            hcount <= hcount + 12'd1;
        end
    end

    wire h_act = (hcount < H_ACTIVE);
    wire v_act = (vcount < V_ACTIVE);
    wire h_sync_r = (hcount >= H_ACTIVE+H_FP) && (hcount < H_ACTIVE+H_FP+H_SYNC);
    wire v_sync_r = (vcount >= V_ACTIVE+V_FP) && (vcount < V_ACTIVE+V_FP+V_SYNC);

    // frame_start toggle: pulse at the very start of the frame (h==0,v==0)
    // top of the active region. We actually want the producer to begin
    // prefilling during vertical blank, so toggle at the start of the LAST
    // line of the frame (one line before v wraps to 0) to give a head start.
    logic frame_tgl;
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) frame_tgl <= 1'b0;
        else if (hcount == 12'd0 && vcount == V_ACTIVE) frame_tgl <= ~frame_tgl;
    end

    // ---- consumer line bookkeeping (hdmi domain) ----------------------
    // rd_line counts active lines 0..V_ACTIVE-1; the line buffer slot is
    // rd_line mod LINE_BUFS. We require the producer to have filled the
    // slot before we read it (it always does — see flow control).
    logic [11:0]      rd_line;       // active line index 0..V_ACTIVE-1
    logic [SLOTW-1:0] rd_slot;
    // produced-line count synced from avl domain (Gray) for flow control.

    // Read address = the NEXT pixel's {slot, word} (the 1-cycle BRAM read
    // latency means we must address column c at cycle c-1). At a line
    // boundary the next pixel is column 0 of the next line, which lives in
    // the next slot — so the slot must advance combinationally here, one
    // cycle ahead of the rd_slot register, or column 0 reads stale data.
    logic [SLOTW-1:0] rd_slot_next;
    always_comb begin
        if (hcount == H_TOTAL-1) begin
            if (vcount == V_TOTAL-1)       rd_slot_next = '0;                 // frame wrap → line 0
            else if (vcount < V_ACTIVE-1)  rd_slot_next = (rd_slot==LINE_BUFS-1) ? '0 : rd_slot+1'b1;
            else                           rd_slot_next = rd_slot;            // within vblank (don't care)
        end else begin
            rd_slot_next = rd_slot;
        end
    end
    wire [11:0]    nx      = (hcount == H_TOTAL-1) ? 12'd0 : hcount + 12'd1;
    wire [LBW-1:0] rd_word = nx[LBW+1:2];   // next-column / 4
    assign lb_raddr = {rd_slot_next, rd_word};

    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin
            rd_line <= '0;
            rd_slot <= '0;
        end else begin
            // advance line at end of each active line
            if (hcount == H_TOTAL-1) begin
                if (vcount == V_TOTAL-1) begin
                    rd_line <= '0;
                    rd_slot <= '0;
                end else if (vcount < V_ACTIVE) begin
                    rd_line <= rd_line + 12'd1;
                    rd_slot <= (rd_slot == LINE_BUFS-1) ? '0 : rd_slot + 1'b1;
                end
            end
        end
    end

    // selected 32-bit pixel from the registered 128-bit word. lb_rdata at
    // cycle c holds word(c>>2); the lane for column c is c[1:0] = current
    // hcount[1:0] (combinational — same cycle as lb_rdata is valid).
    logic [31:0] fb_pixel;
    always_comb begin
        unique case (hcount[1:0])
            2'd0: fb_pixel = lb_rdata[31:0];
            2'd1: fb_pixel = lb_rdata[63:32];
            2'd2: fb_pixel = lb_rdata[95:64];
            2'd3: fb_pixel = lb_rdata[127:96];
        endcase
    end

    // test pattern: SMPTE-ish vertical colour bars + a 1px-moving marker
    logic [23:0] test_rgb;
    always_comb begin
        unique case (hcount[10:8])
            3'd0: test_rgb = 24'hFFFFFF;
            3'd1: test_rgb = 24'hFFFF00;
            3'd2: test_rgb = 24'h00FFFF;
            3'd3: test_rgb = 24'h00FF00;
            3'd4: test_rgb = 24'hFF00FF;
            3'd5: test_rgb = 24'hFF0000;
            3'd6: test_rgb = 24'h0000FF;
            3'd7: test_rgb = 24'h202020;
        endcase
    end

    // ---- output registers --------------------------------------------
    // fb_pixel/live_rgb are in-phase with hcount (the lb_raddr look-ahead
    // is cancelled by the lb_rdata register: fb_pixel(c)=pixel(c)). So the
    // pixel reaches the output through ONE register (hdmi_d), and the sync
    // set must take exactly one register too — registered straight from the
    // combinational h_act/v_act/sync so DE/HS/VS line up with hdmi_d.
    wire [23:0] live_rgb = TEST_PATTERN ? test_rgb
                                        : {fb_pixel[23:16], fb_pixel[15:8], fb_pixel[7:0]};

    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin
            hdmi_d   <= 24'd0;
            hdmi_hs  <= 1'b0;
            hdmi_vs  <= 1'b0;
            hdmi_de  <= 1'b0;
            hdmi_vbl <= 1'b1;
        end else begin
            hdmi_d   <= (h_act && v_act) ? live_rgb : 24'd0;
            hdmi_hs  <= h_sync_r;
            hdmi_vs  <= v_sync_r;
            hdmi_de  <= h_act && v_act;
            hdmi_vbl <= ~v_act;
        end
    end

    // ================================================================
    //  CDC: frame_start (hdmi → avl) and line counters (both ways)
    // ================================================================
    // frame_tgl 2-flop sync into avl domain + edge detect.
    logic frame_tgl_a0, frame_tgl_a1, frame_tgl_a2;
    always_ff @(posedge avl_clk or negedge avl_rst_n) begin
        if (!avl_rst_n) begin
            frame_tgl_a0 <= 1'b0; frame_tgl_a1 <= 1'b0; frame_tgl_a2 <= 1'b0;
        end else begin
            frame_tgl_a0 <= frame_tgl;
            frame_tgl_a1 <= frame_tgl_a0;
            frame_tgl_a2 <= frame_tgl_a1;
        end
    end
    wire frame_start_a = frame_tgl_a1 ^ frame_tgl_a2;

    // Produced/consumed line counts (binary, in their own domains), Gray
    // synced across for flow control. Counts are per-frame (reset at frame).
    logic [11:0] prod_line;          // avl: next line to fill (0..V_ACTIVE)
    logic [11:0] cons_line_bin;      // hdmi: lines consumed this frame

    // hdmi-domain consumed-line binary → Gray → sync to avl
    function automatic [11:0] bin2gray(input [11:0] b); bin2gray = b ^ (b >> 1); endfunction

    // Reset at the top of vertical blank (same instant as frame_tgl) so
    // that throughout vblank cons_line=0 and the producer is free to
    // pre-fill lines 0..LINE_BUFS-1 before active video begins. Increment
    // once per completed active line.
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) cons_line_bin <= '0;
        else if (hcount == 12'd0 && vcount == V_ACTIVE) cons_line_bin <= '0;
        else if (hcount == H_TOTAL-1 && vcount < V_ACTIVE) cons_line_bin <= cons_line_bin + 12'd1;
    end

    logic [11:0] cons_gray_h, cons_gray_a0, cons_gray_a1;
    always_ff @(posedge hdmi_clk) cons_gray_h <= bin2gray(cons_line_bin);
    always_ff @(posedge avl_clk) begin
        cons_gray_a0 <= cons_gray_h;
        cons_gray_a1 <= cons_gray_a0;
    end
    // gray2bin in avl domain
    logic [11:0] cons_line_a;
    always_comb begin
        cons_line_a[11] = cons_gray_a1[11];
        for (int i = 10; i >= 0; i--)
            cons_line_a[i] = cons_line_a[i+1] ^ cons_gray_a1[i];
    end

    // ================================================================
    //  Avalon read producer (avl_clk domain)
    // ================================================================
    typedef enum logic [1:0] { P_IDLE, P_REQ, P_RX } pstate_t;
    pstate_t pstate;

    logic [SLOTW-1:0] prod_slot;
    logic [LBW:0]     prod_word;        // 0..WORDS_PER_LINE
    logic [31:0]      line_byte_base;   // byte addr of current line start
    logic [31:0]      fb_base_l;        // latched at frame_start
    logic [13:0]      fb_stride_l;
    logic [7:0]       burst_left;       // beats remaining in current burst

    // words remaining in the line at the start of a burst
    wire [LBW:0] words_rem = WORDS_PER_LINE[LBW:0] - prod_word;
    wire [7:0]   this_burst = (words_rem >= BURST[LBW:0]) ? BURST[7:0] : words_rem[7:0];

    // may we start filling prod_line? Only if it is < LINE_BUFS ahead of
    // the consumer (so we never overwrite a slot still being displayed),
    // and prod_line hasn't reached V_ACTIVE (whole frame fetched).
    wire prod_ahead_ok = ((prod_line - cons_line_a) < LINE_BUFS[11:0]);
    wire prod_more     = (prod_line < V_ACTIVE[11:0]);

    assign avl_address    = line_byte_base[N_AW+3:4] + {{(N_AW-LBW){1'b0}}, prod_word[LBW-1:0]};
    assign avl_burstcount = (pstate == P_REQ) ? this_burst : 8'd0;
    assign avl_read       = (pstate == P_REQ);

    // line-buffer write driven by readdatavalid during RX
    assign lb_we    = (pstate == P_RX) && avl_readdatavalid;
    assign lb_waddr = {prod_slot, prod_word[LBW-1:0]};
    assign lb_wdata = avl_readdata;

    always_ff @(posedge avl_clk or negedge avl_rst_n) begin
        if (!avl_rst_n) begin
            pstate         <= P_IDLE;
            prod_line      <= '0;
            prod_slot      <= '0;
            prod_word      <= '0;
            line_byte_base <= '0;
            fb_base_l      <= '0;
            fb_stride_l    <= '0;
            burst_left     <= '0;
        end else begin
            if (frame_start_a) begin
                // new frame: latch geometry, reset producer to line 0.
                fb_base_l      <= fb_base;
                fb_stride_l    <= fb_stride;
                prod_line      <= '0;
                prod_slot      <= '0;
                prod_word      <= '0;
                line_byte_base <= fb_base;
                pstate         <= P_IDLE;
            end else begin
                unique case (pstate)
                    P_IDLE: begin
                        if (prod_more && prod_ahead_ok) begin
                            prod_word <= '0;
                            pstate    <= P_REQ;
                        end
                    end
                    P_REQ: begin
                        // hold read+addr+burst until accepted
                        if (!avl_waitrequest) begin
                            burst_left <= this_burst;
                            pstate     <= P_RX;
                        end
                    end
                    P_RX: begin
                        if (avl_readdatavalid) begin
                            prod_word  <= prod_word + 1'b1;
                            burst_left <= burst_left - 8'd1;
                            if (burst_left == 8'd1) begin
                                // burst complete
                                if (prod_word + 1'b1 >= WORDS_PER_LINE[LBW:0]) begin
                                    // line complete → advance to next line/slot
                                    prod_line      <= prod_line + 12'd1;
                                    prod_slot      <= (prod_slot == LINE_BUFS-1) ? '0 : prod_slot + 1'b1;
                                    line_byte_base <= line_byte_base + {18'd0, fb_stride_l};
                                    pstate         <= P_IDLE;
                                end else begin
                                    pstate <= P_REQ; // next burst, same line
                                end
                            end
                        end
                    end
                    default: pstate <= P_IDLE;
                endcase
            end
        end
    end

endmodule
