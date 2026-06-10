//============================================================================
//
//  Scanout compositor — Phase B (2-layer: wallpaper + content blend).
//
//  Replaces the framework's ASCAL block for the self-contained menu core.
//  Reads up to two BGRA8888 framebuffers straight from DDR3 over the
//  `vbuf` 128-bit Avalon-MM port and drives the HDMI pixel stream
//  (hdmi_d / hs / vs / de / vbl) at the true pixel clock, feeding the
//  framework's existing shadowmask -> OSD -> HDMI_TX tail untouched.
//
//  Layers (z-order, bottom to top):
//    * Layer 0 = wallpaper (fb base = wallpaper_base), OPAQUE base.
//    * Layer 1 = content   (fb base = fb_base), premultiplied alpha.
//  Per pixel: out.rgb = content.rgb + wallpaper.rgb * (255 - content.a) / 256
//  (premultiplied "over"). When composite_en=0 the wallpaper layer is not
//  read or blended and content is emitted directly (Phase A behaviour) —
//  the bisection fallback. Phase D generalises this to 4 mask-gated layers.
//
//  Two clock domains:
//    * avl_clk  (clk_100m, 100 MHz) — Avalon read master + line-buffer
//      WRITE side. Per scanline it fills the wallpaper line then the
//      content line into two rings of line buffers, staying LINE_BUFS
//      lines ahead of the beam (read-ahead that rides out HPS DDR3
//      contention — the historical scanout-jitter source).
//    * hdmi_clk (148.5 MHz for 1080p60) — raster timing generator +
//      line-buffer READ side + premultiplied blend + pixel output.
//
//  Frame handshake: the hdmi side is the timing master. It emits a
//  per-frame frame_tgl toggle at the top of vertical blank; this crosses
//  to avl_clk and resets the producer to line 0 so it can prefill before
//  active video begins. Line-level flow control uses Gray-coded
//  produced/consumed line counters across the boundary.
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

    // Number of whole-line buffers per layer for DDR read-ahead. >=2;
    // deeper rides out DDR contention.
    parameter int LINE_BUFS = 4,

    // Synthesis-time bring-up aid: 1 = ignore DDR, emit a colour-bar test
    // pattern (proves clk_hdmi + timing + HDMI tail in isolation from the
    // vbuf read path). 0 = real framebuffer scanout.
    parameter bit TEST_PATTERN = 1'b0
) (
    // ---- HDMI pixel domain -------------------------------------------
    input  logic               hdmi_clk,
    input  logic               hdmi_rst_n,
    output logic [23:0]        hdmi_d,      // {R[23:16], G[15:8], B[7:0]}
    output logic               hdmi_hs,
    output logic               hdmi_vs,
    output logic               hdmi_de,
    output logic               hdmi_vbl,    // vertical blank (-> FB_VBL pacing)
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

    // ---- Layer config, avl_clk domain --------------------------------
    // Byte base addresses + shared byte stride, host-stable (the host only
    // changes these at PRESENT, between frames). Latched at frame start.
    input  logic [31:0]        fb_base,         // layer 1 (content) base
    input  logic [13:0]        fb_stride,       // shared stride (both layers)
    input  logic [31:0]        wallpaper_base,  // layer 0 (wallpaper) base
    input  logic               composite_en,    // 1 = blend content over wallpaper
    // Content coverage mask: 1 bit per 64x64 tile (30x17 = 510 bits, packed
    // 30 bits/row in 17 consecutive 32-bit words). A read-skip HINT: the
    // producer reads only set tiles of the content layer and the consumer
    // forces unset tiles transparent (-> wallpaper shows). Host keeps it
    // conservative (covers all in-flight triple-buffer slots). When
    // content_mask_en=0 the mask is ignored (full content read) = pre-mask
    // behaviour. content_mask_base is host-stable, latched at frame start.
    input  logic [31:0]        content_mask_base,
    input  logic               content_mask_en
);

    // ---- Derived timing ----------------------------------------------
    localparam int H_TOTAL = H_ACTIVE + H_FP + H_SYNC + H_BP; // 2200
    localparam int V_TOTAL = V_ACTIVE + V_FP + V_SYNC + V_BP; // 1125

    localparam int PIX_PER_WORD = N_DW / 32;                  // 4
    localparam int WORDS_PER_LINE = H_ACTIVE / PIX_PER_WORD;  // 480
    localparam int LBW = $clog2(WORDS_PER_LINE);              // line-buf word addr bits (9)
    localparam int LB_SLOT = 2**LBW;                          // slot stride (512, power-of-2)
    localparam int SLOTW = $clog2(LINE_BUFS);

    // ---- content coverage mask geometry ------------------------------
    localparam int TILE        = 64;                         // tile edge (px)
    localparam int N_TX        = H_ACTIVE / TILE;            // tiles across (30)
    localparam int N_TY        = (V_ACTIVE + TILE-1) / TILE; // tile rows (17)
    localparam int WORDS_PER_TILE = TILE / PIX_PER_WORD;     // 16 (128-bit words / tile)
    localparam int WPT_LOG     = $clog2(WORDS_PER_TILE);     // 4
    localparam int BURST_TILES = BURST / WORDS_PER_TILE;     // 8 (tiles per max burst)
    localparam int MASK_BEATS  = (N_TY + PIX_PER_WORD-1) / PIX_PER_WORD; // 32-bit rows packed 4/beat -> 5
    localparam int TXW         = $clog2(N_TX);               // 5
    localparam int TYW         = $clog2(N_TY);               // 5
    localparam int TSHIFT      = $clog2(TILE);               // 6 (px -> tile)

    // write side never used; byteenable all-ones (full-word reads).
    assign avl_write     = 1'b0;
    assign avl_writedata = '0;
    assign avl_byteenable= '1;
    assign hdmi_brd      = 1'b0;

    // ================================================================
    //  Line buffers — two rings (content + wallpaper), each
    //  LINE_BUFS x LB_SLOT x N_DW. Slot stride is a power of two so
    //  {slot,word} concatenation is a valid flat index (WORDS_PER_LINE=480
    //  isn't a power of two; we waste the tail). Dual-clock simple
    //  dual-port: write @avl_clk, read @hdmi_clk.
    // ================================================================
    (* ramstyle = "no_rw_check, M10K" *)
    logic [N_DW-1:0] linebuf_ct [LINE_BUFS*LB_SLOT-1:0];
    (* ramstyle = "no_rw_check, M10K" *)
    logic [N_DW-1:0] linebuf_wp [LINE_BUFS*LB_SLOT-1:0];

    // write ports (driven by the producer)
    logic                  lb_we_ct, lb_we_wp;
    logic [SLOTW+LBW-1:0]  lb_waddr;       // shared (one layer fills at a time)
    logic [N_DW-1:0]       lb_wdata;
    // read ports (driven by the consumer, shared address)
    logic [SLOTW+LBW-1:0]  lb_raddr;
    logic [N_DW-1:0]       ct_rdata, wp_rdata;

    always_ff @(posedge avl_clk) begin
        if (lb_we_ct) linebuf_ct[lb_waddr] <= lb_wdata;
        if (lb_we_wp) linebuf_wp[lb_waddr] <= lb_wdata;
    end
    always_ff @(posedge hdmi_clk) begin
        ct_rdata <= linebuf_ct[lb_raddr];
        wp_rdata <= linebuf_wp[lb_raddr];
    end

    // ================================================================
    //  Content coverage mask storage. Written by the producer at frame
    //  start (avl_clk) from DDR; sampled by the consumer (hdmi_clk). The
    //  value is stable for the whole frame after the load (only rewritten
    //  at the next frame start, during vblank), so a 2-flop array sync is
    //  safe — any skew during the brief frame-start write settles long
    //  before active scanout uses it. Reset all-1s = "cover everything"
    //  so nothing is hidden before the first load / when masking is off.
    // ================================================================
    logic [N_TX-1:0] mask_avl [0:N_TY-1];           // avl domain (producer)
    logic [N_TX-1:0] mask_s0  [0:N_TY-1];           // hdmi sync flop 0
    logic [N_TX-1:0] mask_hdmi[0:N_TY-1];           // hdmi domain (consumer)
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin
            for (int i = 0; i < N_TY; i++) begin
                mask_s0[i]   <= '1;
                mask_hdmi[i] <= '1;
            end
        end else
        for (int i = 0; i < N_TY; i++) begin
            mask_s0[i]   <= mask_avl[i];
            mask_hdmi[i] <= mask_s0[i];
        end
    end

    // content_mask_en into the hdmi domain (quasi-static; 2-flop).
    logic cmask_en_h0, cmask_en_h1;
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin cmask_en_h0 <= 1'b0; cmask_en_h1 <= 1'b0; end
        else begin cmask_en_h0 <= content_mask_en; cmask_en_h1 <= cmask_en_h0; end
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

    // frame toggle at the top of vertical blank (start of the first blank
    // line) so the producer gets a head start prefilling during vblank.
    logic frame_tgl;
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) frame_tgl <= 1'b0;
        else if (hcount == 12'd0 && vcount == V_ACTIVE) frame_tgl <= ~frame_tgl;
    end

    // composite_en into the hdmi domain (quasi-static config; 2-flop).
    logic comp_en_h0, comp_en_h1;
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin comp_en_h0 <= 1'b0; comp_en_h1 <= 1'b0; end
        else begin comp_en_h0 <= composite_en; comp_en_h1 <= comp_en_h0; end
    end

    // ---- consumer line bookkeeping (hdmi domain) ----------------------
    logic [SLOTW-1:0] rd_slot;        // line-buffer slot for the active line

    // Read address = the NEXT pixel's {slot, word} (1-cycle BRAM read
    // latency). At a line boundary the next pixel is column 0 of the next
    // line, which lives in the next slot — so the slot must advance
    // combinationally here, one cycle ahead of the rd_slot register.
    logic [SLOTW-1:0] rd_slot_next;
    always_comb begin
        if (hcount == H_TOTAL-1) begin
            if (vcount == V_TOTAL-1)       rd_slot_next = '0;                 // frame wrap -> line 0
            else if (vcount < V_ACTIVE-1)  rd_slot_next = (rd_slot==LINE_BUFS-1) ? '0 : rd_slot+1'b1;
            else                           rd_slot_next = rd_slot;            // within vblank
        end else begin
            rd_slot_next = rd_slot;
        end
    end
    wire [11:0]    nx      = (hcount == H_TOTAL-1) ? 12'd0 : hcount + 12'd1;
    wire [LBW-1:0] rd_word = nx[LBW+1:2];   // next-column / 4
    assign lb_raddr = {rd_slot_next, rd_word};

    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin
            rd_slot <= '0;
        end else if (hcount == H_TOTAL-1) begin
            if (vcount == V_TOTAL-1)      rd_slot <= '0;
            else if (vcount < V_ACTIVE)   rd_slot <= (rd_slot == LINE_BUFS-1) ? '0 : rd_slot + 1'b1;
        end
    end

    // Mask tile-row for the active line, latched at the line boundary for
    // the NEXT line (one 17:1 mux off the critical pixel path, leaving just
    // a 30:1 bit select per pixel). nv = next vcount.
    wire [11:0] nv = (hcount == H_TOTAL-1)
                     ? ((vcount == V_TOTAL-1) ? 12'd0 : vcount + 12'd1)
                     : vcount;
    logic [N_TX-1:0] cur_mask_row;
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n)              cur_mask_row <= '1;
        else if (hcount == H_TOTAL-1) cur_mask_row <= mask_hdmi[nv[TSHIFT+TYW-1 -: TYW]];
    end
    // covered bit for the current column (tile_x = hcount>>6).
    wire [TXW-1:0] tile_x   = hcount[TSHIFT+TXW-1 -: TXW];
    wire           tile_cov = cur_mask_row[tile_x];
    // force content transparent on unset tiles when masking is enabled.
    wire           ct_drop  = cmask_en_h1 && !tile_cov;

    // selected 32-bit pixel from each registered 128-bit word. The read
    // word is in-phase with hcount: ct_rdata/wp_rdata at cycle c hold
    // word(c>>2); the lane for column c is c[1:0] = hcount[1:0].
    logic [31:0] ct_pixel, wp_pixel;
    always_comb begin
        unique case (hcount[1:0])
            2'd0: ct_pixel = ct_rdata[31:0];
            2'd1: ct_pixel = ct_rdata[63:32];
            2'd2: ct_pixel = ct_rdata[95:64];
            2'd3: ct_pixel = ct_rdata[127:96];
        endcase
        unique case (hcount[1:0])
            2'd0: wp_pixel = wp_rdata[31:0];
            2'd1: wp_pixel = wp_rdata[63:32];
            2'd2: wp_pixel = wp_rdata[95:64];
            2'd3: wp_pixel = wp_rdata[127:96];
        endcase
    end

    // test pattern: vertical colour bars (stage-0 comb)
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

    // ---- 3-stage pixel pipeline --------------------------------------
    // The premultiplied blend has an 8x8 multiply per channel; folding it
    // into the same cycle as the BRAM read + lane mux only reached
    // ~109 MHz, so the multiply gets its own register-to-register stage.
    // ct_pixel/wp_pixel are in-phase with hcount (pixel(c) at cycle c).
    //   stage 1: register the selected layer pixels + sync
    //   stage 2: premultiplied blend -> pix_q2 + sync
    //   stage 3: HDMI output register
    logic [31:0] ct_pix_q, wp_pix_q;
    logic [23:0] test_q1;
    logic        h_act_q1, v_act_q1, h_sync_q1, v_sync_q1;
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin
            ct_pix_q <= 32'd0; wp_pix_q <= 32'd0; test_q1 <= 24'd0;
            h_act_q1 <= 1'b0; v_act_q1 <= 1'b0; h_sync_q1 <= 1'b0; v_sync_q1 <= 1'b0;
        end else begin
            ct_pix_q <= ct_drop ? 32'd0 : ct_pixel; wp_pix_q <= wp_pixel; test_q1 <= test_rgb;
            h_act_q1 <= h_act;   v_act_q1 <= v_act;
            h_sync_q1 <= h_sync_r; v_sync_q1 <= v_sync_r;
        end
    end

    // stage 2 combinational: premultiplied "over" blend.
    // out.rgb = content.rgb + wallpaper.rgb * (255 - content.a) / 256.
    // Content is premultiplied so this never exceeds 255 for valid inputs;
    // saturate anyway as belt-and-suspenders.
    wire [7:0]  c_a  = ct_pix_q[31:24];
    wire [7:0]  c_r  = ct_pix_q[23:16];
    wire [7:0]  c_g  = ct_pix_q[15:8];
    wire [7:0]  c_b  = ct_pix_q[7:0];
    wire [7:0]  ia   = 8'd255 - c_a;
    wire [15:0] wr_m = wp_pix_q[23:16] * ia;
    wire [15:0] wg_m = wp_pix_q[15:8]  * ia;
    wire [15:0] wb_m = wp_pix_q[7:0]   * ia;
    wire [8:0]  sr   = c_r + wr_m[15:8];
    wire [8:0]  sg   = c_g + wg_m[15:8];
    wire [8:0]  sb   = c_b + wb_m[15:8];
    wire [7:0]  or_  = sr[8] ? 8'hFF : sr[7:0];
    wire [7:0]  og   = sg[8] ? 8'hFF : sg[7:0];
    wire [7:0]  ob   = sb[8] ? 8'hFF : sb[7:0];
    wire [23:0] content_rgb = {c_r, c_g, c_b};
    wire [23:0] blend_rgb   = comp_en_h1 ? {or_, og, ob} : content_rgb;
    wire [23:0] pix_sel     = TEST_PATTERN ? test_q1 : blend_rgb;

    logic [23:0] pix_q2;
    logic        h_act_q2, v_act_q2, h_sync_q2, v_sync_q2;
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin
            pix_q2 <= 24'd0;
            h_act_q2 <= 1'b0; v_act_q2 <= 1'b0; h_sync_q2 <= 1'b0; v_sync_q2 <= 1'b0;
        end else begin
            pix_q2 <= pix_sel;
            h_act_q2 <= h_act_q1;   v_act_q2 <= v_act_q1;
            h_sync_q2 <= h_sync_q1; v_sync_q2 <= v_sync_q1;
        end
    end

    // stage 3: HDMI output register
    always_ff @(posedge hdmi_clk or negedge hdmi_rst_n) begin
        if (!hdmi_rst_n) begin
            hdmi_d   <= 24'd0;
            hdmi_hs  <= 1'b0;
            hdmi_vs  <= 1'b0;
            hdmi_de  <= 1'b0;
            hdmi_vbl <= 1'b1;
        end else begin
            hdmi_d   <= (h_act_q2 && v_act_q2) ? pix_q2 : 24'd0;
            hdmi_hs  <= h_sync_q2;
            hdmi_vs  <= v_sync_q2;
            hdmi_de  <= h_act_q2 && v_act_q2;
            hdmi_vbl <= ~v_act_q2;
        end
    end

    // ================================================================
    //  CDC: frame_start (hdmi -> avl) and consumed-line count (hdmi -> avl)
    // ================================================================
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

    logic [11:0] prod_line;          // avl: next line to fill (0..V_ACTIVE)
    logic [11:0] cons_line_bin;      // hdmi: lines consumed this frame

    function automatic [11:0] bin2gray(input [11:0] b); bin2gray = b ^ (b >> 1); endfunction

    // Reset at top of vblank so the producer is free to pre-fill before
    // active video; increment once per completed active line.
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
    logic [11:0] cons_line_a;
    always_comb begin
        cons_line_a[11] = cons_gray_a1[11];
        for (int i = 10; i >= 0; i--)
            cons_line_a[i] = cons_line_a[i+1] ^ cons_gray_a1[i];
    end

    // ================================================================
    //  Avalon read producer (avl_clk domain). At frame start it loads the
    //  content coverage mask (when enabled), then fills wallpaper (full)
    //  then content per line. With masking on, the content layer reads
    //  only its set 64x64 tiles (WORDS_PER_TILE-word bursts), skipping the
    //  transparent majority — the bandwidth win. P_DECIDE picks the next
    //  burst (or skips a tile) between line bursts.
    // ================================================================
    typedef enum logic [2:0] {
        P_IDLE, P_DECIDE, P_REQ, P_RX, P_MREQ, P_MRX
    } pstate_t;
    pstate_t pstate;

    logic             prod_layer;       // 0 = wallpaper, 1 = content
    logic [SLOTW-1:0] prod_slot;
    logic [LBW:0]     prod_word;        // 0..WORDS_PER_LINE
    logic [31:0]      ct_line_base;     // byte addr of current content line
    logic [31:0]      wp_line_base;     // byte addr of current wallpaper line
    logic [13:0]      fb_stride_l;
    logic             comp_en_l;        // composite_en latched per frame
    logic             mask_rd_l;        // content_mask_en latched per frame
    logic [31:0]      mask_base_l;      // content_mask_base latched per frame
    logic [7:0]       burst_left;
    logic [2:0]       mask_beat;        // beat index during the mask read

    // masked-content tile bookkeeping
    wire [TYW-1:0] prod_tile_y = prod_line[TSHIFT+TYW-1 -: TYW];
    wire [5:0]     prod_tile_x = prod_word[LBW:WPT_LOG];            // 0..30
    wire           is_mct      = (prod_layer == 1'b1) && mask_rd_l; // masked content
    // Tile-row mask, latched once per line (in P_IDLE below) so the run
    // scan below isn't behind the 17:1 mask_avl[prod_tile_y] select on the
    // clk_100m burst-decision path.
    logic [N_TX-1:0] mask_row_p;
    wire           cur_tile_set= mask_row_p[prod_tile_x[TXW-1:0]];

    // Coalesce a run of consecutive set tiles from prod_tile_x into one
    // burst (capped at BURST_TILES) — reading only set tiles but in LARGE
    // bursts so the DDR isn't fragmented into per-tile transactions (that
    // fragmentation tanks effective bandwidth -> producer underrun/tearing).
    // Zero-padded above N_TX so the +j index never reads past the row.
    wire [N_TX+BURST_TILES-1:0] mask_row_ext = {{BURST_TILES{1'b0}}, mask_row_p};
    logic [3:0] run_tiles;       // 0..BURST_TILES contiguous set tiles
    always_comb begin
        run_tiles = 4'd0;
        for (int j = 0; j < BURST_TILES; j++)
            if (run_tiles == j[3:0] && mask_row_ext[prod_tile_x + j[5:0]])
                run_tiles = run_tiles + 4'd1;
    end

    wire [LBW:0] words_rem  = WORDS_PER_LINE[LBW:0] - prod_word;
    wire [7:0]   full_burst = (words_rem >= BURST[LBW:0]) ? BURST[7:0] : words_rem[7:0];
    wire [7:0]   mct_burst  = {run_tiles, 4'b0};                 // run_tiles * 16
    wire [7:0]   this_burst = is_mct ? mct_burst : full_burst;

    wire prod_ahead_ok = ((prod_line - cons_line_a) < LINE_BUFS[11:0]);
    wire prod_more     = (prod_line < V_ACTIVE[11:0]);
    wire line_done     = (prod_word >= WORDS_PER_LINE[LBW:0]);

    wire [31:0] cur_line_base = (prod_layer == 1'b0) ? wp_line_base : ct_line_base;
    assign avl_address    = (pstate == P_MREQ)
                            ? mask_base_l[N_AW+3:4]
                            : cur_line_base[N_AW+3:4] + {{(N_AW-LBW){1'b0}}, prod_word[LBW-1:0]};
    assign avl_burstcount = (pstate == P_REQ)  ? this_burst
                          : (pstate == P_MREQ) ? MASK_BEATS[7:0] : 8'd0;
    assign avl_read       = (pstate == P_REQ) || (pstate == P_MREQ);

    // route captured *line* beats to the layer being filled (not mask beats)
    assign lb_we_wp = (pstate == P_RX) && avl_readdatavalid && (prod_layer == 1'b0);
    assign lb_we_ct = (pstate == P_RX) && avl_readdatavalid && (prod_layer == 1'b1);
    assign lb_waddr = {prod_slot, prod_word[LBW-1:0]};
    assign lb_wdata = avl_readdata;

    always_ff @(posedge avl_clk or negedge avl_rst_n) begin
        if (!avl_rst_n) begin
            pstate       <= P_IDLE;
            prod_layer   <= 1'b1;
            prod_line    <= '0;
            prod_slot    <= '0;
            prod_word    <= '0;
            ct_line_base <= '0;
            wp_line_base <= '0;
            fb_stride_l  <= '0;
            comp_en_l    <= 1'b0;
            mask_rd_l    <= 1'b0;
            mask_base_l  <= '0;
            burst_left   <= '0;
            mask_beat    <= '0;
            mask_row_p   <= '0;
            for (int i = 0; i < N_TY; i++) mask_avl[i] <= '1; // all covered
        end else begin
            if (frame_start_a) begin
                // new frame: latch geometry, reset producer to line 0.
                fb_stride_l  <= fb_stride;
                comp_en_l    <= composite_en;
                mask_rd_l    <= content_mask_en;
                mask_base_l  <= content_mask_base;
                prod_line    <= '0;
                prod_slot    <= '0;
                prod_word    <= '0;
                ct_line_base <= fb_base;
                wp_line_base <= wallpaper_base;
                prod_layer   <= composite_en ? 1'b0 : 1'b1; // wallpaper first if compositing
                mask_beat    <= '0;
                pstate       <= content_mask_en ? P_MREQ : P_IDLE; // load mask first
            end else begin
                unique case (pstate)
                    // ---- mask load (once per frame) ----
                    P_MREQ: begin
                        if (!avl_waitrequest) begin
                            burst_left <= MASK_BEATS[7:0];
                            mask_beat  <= '0;
                            pstate     <= P_MRX;
                        end
                    end
                    P_MRX: begin
                        if (avl_readdatavalid) begin
                            // each beat carries PIX_PER_WORD packed 32-bit
                            // rows; low N_TX bits of each are the tile bits.
                            for (int k = 0; k < PIX_PER_WORD; k++) begin
                                if ((mask_beat * PIX_PER_WORD + k) < N_TY)
                                    mask_avl[mask_beat * PIX_PER_WORD + k]
                                        <= avl_readdata[k*32 +: N_TX];
                            end
                            mask_beat  <= mask_beat + 3'd1;
                            burst_left <= burst_left - 8'd1;
                            if (burst_left == 8'd1) pstate <= P_IDLE;
                        end
                    end
                    // ---- per-line fill ----
                    P_IDLE: begin
                        if (prod_more && prod_ahead_ok) begin
                            prod_word  <= '0;
                            prod_layer <= comp_en_l ? 1'b0 : 1'b1;
                            // latch this line's tile-row mask (stable for
                            // the whole line; mask_avl is frame-stable).
                            mask_row_p <= mask_avl[prod_tile_y];
                            pstate     <= P_DECIDE;
                        end
                    end
                    P_DECIDE: begin
                        if (line_done) begin
                            // current layer's line finished
                            if (comp_en_l && prod_layer == 1'b0) begin
                                prod_layer <= 1'b1;     // wallpaper -> content
                                prod_word  <= '0;       // re-evaluate next cycle
                            end else begin
                                prod_line    <= prod_line + 12'd1;
                                prod_slot    <= (prod_slot == LINE_BUFS-1) ? '0 : prod_slot + 1'b1;
                                ct_line_base <= ct_line_base + {18'd0, fb_stride_l};
                                wp_line_base <= wp_line_base + {18'd0, fb_stride_l};
                                pstate       <= P_IDLE;
                            end
                        end else if (is_mct && !cur_tile_set) begin
                            prod_word <= prod_word + WORDS_PER_TILE[LBW:0]; // skip empty tile
                        end else begin
                            pstate <= P_REQ;            // issue a burst
                        end
                    end
                    P_REQ: begin
                        if (!avl_waitrequest) begin
                            burst_left <= this_burst;
                            pstate     <= P_RX;
                        end
                    end
                    P_RX: begin
                        if (avl_readdatavalid) begin
                            prod_word  <= prod_word + 1'b1;
                            burst_left <= burst_left - 8'd1;
                            if (burst_left == 8'd1) pstate <= P_DECIDE;
                        end
                    end
                    default: pstate <= P_IDLE;
                endcase
            end
        end
    end

endmodule
