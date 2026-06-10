//============================================================================
//
//  menu-core control register file.
//
//  Decodes accesses on the LW_H2F window mapped to PROTOCOL.md §3.1.
//  Register block base is 0xFF210000 from the host's perspective; this
//  module sees offsets within the 2 MiB LW_H2F window — we react only to
//  hits in the 0x10000..0x100FF range and otherwise return zero.
//
//  Behaviour by register (M2b):
//    - ID            (0x00) read-only constant 32'h1FFA_0001
//    - STATUS        (0x04) read-only, bit 0 = ER, bit 1 = BZ; driven
//                           by ring_fetcher sideband inputs
//    - CONTROL       (0x08) R/W; CONTROL[0] is `enable_o`. CONTROL[2]
//                           is a self-clearing pulse on `clear_error_o`
//                           (host writes 1 to clear ER + reset fetcher).
//    - ERROR_INFO    (0x0C) read-only, fed by `error_info_i`
//    - VSYNC_COUNT   (0x10) read-only, M2b returns 0 (real in M2c)
//    - FRAME_COUNT   (0x14) read-only, fed by `frame_count_i`
//    - RING_BASE     (0x30) R/W; surfaced as `ring_base_o`
//    - RING_SIZE     (0x34) R/W; surfaced as `ring_size_o`
//    - RING_HEAD     (0x38) read-only, fed by `ring_head_i`
//    - RING_TAIL     (0x3C) R/W; surfaced as `ring_tail_o`
//    - RING_KICK     (0x40) write-only pulse → `ring_kick_o`
//    - FENCE_VALUE   (0x48) read-only, fed by `fence_value_i`
//    - LAYER_TABLE_BASE (0x68) R/W; physical address of the 16 KB
//                              layer region (two back-to-back 8 KB
//                              tables, A at +0 and B at +0x2000).
//    - LAYER_COMMIT  (0x6C) R/W; bit 31 = active table (0 = A, 1 = B),
//                              bits 8..0 = valid layer count (0..256).
//                              Single-write atomic frame-swap.
//    - LAYER_DEBUG   (0x70) R-only; free-running count of layer
//                              descriptors the DMA has shipped into
//                              the cache (sideband from `layer_dma`).
//                              Diagnostic only.
//    - all other slots in 0x00..0xFF: R/W scratch
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

    // Sideband out: software-controlled bits.
    output logic        enable_o,         // CONTROL[0]
    output logic        clear_error_o,    // 1-cycle pulse from CONTROL[2]
    output logic [31:0] ring_base_o,
    output logic [31:0] ring_size_o,
    output logic [31:0] ring_tail_o,
    output logic        ring_kick_o,      // 1-cycle pulse on RING_KICK write
    output logic [31:0] fb0_addr_o,
    output logic [31:0] fb1_addr_o,
    output logic [31:0] fb2_addr_o,
    output logic [11:0] fb_width_o,
    output logic [11:0] fb_height_o,
    output logic [13:0] fb_stride_o,
    output logic [31:0] tex_table_addr_o,

    // Layer-table sideband. The host writes LAYER_TABLE_BASE once at
    // init to point at the 16 KB layer region (two back-to-back 8 KB
    // tables, A at +0 and B at +0x2000). LAYER_COMMIT carries the
    // active-table selector and the valid layer count and is written
    // atomically each frame to swap which table the compositor reads.
    //   bit 31    = active table index (0 = A, 1 = B)
    //   bits 8..0 = valid layer count (0..256)
    output logic [31:0] layer_table_base_o,
    output logic        layer_active_o,
    output logic [8:0]  layer_count_o,

    // Scanout-compositor config (Phase B). WALLPAPER_ADDR (0x74) is the
    // base of the opaque wallpaper layer; composite_en is CONTROL[8] and
    // turns on the content-over-wallpaper blend in the sys_top compositor.
    output logic [31:0] wallpaper_addr_o,
    output logic        composite_en_o,

    // Content coverage mask (task #15). CONTENT_MASK_ADDR (0x78) points at
    // the host's 1-bit-per-64x64-tile mask (read-skip hint); content_mask_en
    // is CONTROL[9] and gates the compositor's tile-skip + drop.
    output logic [31:0] content_mask_addr_o,
    output logic        content_mask_en_o,

    // Compositor observability — surfaces through LAYER_DEBUG (0x70).
    // descriptors_i is a free-running count of layer descriptors the
    // DMA has written into the cache, so the host can verify it is
    // ticking at frame rate (count × frames per second).
    input  logic [31:0] layer_descriptors_i,

    // Sideband in: FPGA-driven views.
    input  logic [31:0] ring_head_i,
    input  logic [31:0] fence_value_i,
    input  logic [31:0] frame_count_i,
    input  logic [31:0] vsync_count_i,
    input  logic [31:0] fb_state_i,
    input  logic [31:0] error_info_i,
    input  logic        status_busy_i,
    input  logic        status_error_i,
    input  logic [11:0] hdmi_width_i,
    input  logic [11:0] hdmi_height_i
);

    // Register-file index constants matching PROTOCOL.md §3.1 offsets.
    localparam logic [5:0] IDX_ID          = 6'h00;
    localparam logic [5:0] IDX_STATUS      = 6'h01;
    localparam logic [5:0] IDX_CONTROL     = 6'h02;
    localparam logic [5:0] IDX_ERROR_INFO  = 6'h03;
    localparam logic [5:0] IDX_VSYNC_COUNT = 6'h04;
    localparam logic [5:0] IDX_FRAME_COUNT = 6'h05;
    localparam logic [5:0] IDX_VIDEO_INFO  = 6'h07;   // 0x1C / 4
    localparam logic [5:0] IDX_FB_STATE    = 6'h08;   // 0x20 / 4
    localparam logic [5:0] IDX_FB_WIDTH    = 6'h09;   // 0x24 / 4
    localparam logic [5:0] IDX_FB_HEIGHT   = 6'h0A;   // 0x28 / 4
    localparam logic [5:0] IDX_FB_STRIDE   = 6'h0B;   // 0x2C / 4
    localparam logic [5:0] IDX_RING_BASE   = 6'h0C;   // 0x30 / 4
    localparam logic [5:0] IDX_RING_SIZE   = 6'h0D;   // 0x34 / 4
    localparam logic [5:0] IDX_RING_HEAD   = 6'h0E;   // 0x38 / 4
    localparam logic [5:0] IDX_RING_TAIL   = 6'h0F;   // 0x3C / 4
    localparam logic [5:0] IDX_RING_KICK   = 6'h10;   // 0x40 / 4
    localparam logic [5:0] IDX_FENCE_VALUE = 6'h12;   // 0x48 / 4
    localparam logic [5:0] IDX_FB0_ADDR       = 6'h14;   // 0x50 / 4
    localparam logic [5:0] IDX_FB1_ADDR       = 6'h15;   // 0x54 / 4
    localparam logic [5:0] IDX_FB2_ADDR       = 6'h16;   // 0x58 / 4
    localparam logic [5:0] IDX_TEX_TABLE_ADDR  = 6'h18;   // 0x60 / 4
    localparam logic [5:0] IDX_LAYER_TABLE_BASE = 6'h1A;  // 0x68 / 4
    localparam logic [5:0] IDX_LAYER_COMMIT     = 6'h1B;  // 0x6C / 4
    localparam logic [5:0] IDX_LAYER_DEBUG      = 6'h1C;  // 0x70 / 4
    localparam logic [5:0] IDX_WALLPAPER_ADDR   = 6'h1D;  // 0x74 / 4
    localparam logic [5:0] IDX_CONTENT_MASK_ADDR= 6'h1E;  // 0x78 / 4

    // The LW_H2F window is 2 MiB (21-bit address). Our register block
    // sits at host physical 0xFF210000, which is offset 0x10000 within
    // the window.
    localparam logic [20:0] BLOCK_BASE = 21'h10000;
    localparam logic [20:0] BLOCK_MASK = 21'hFFF00;

    wire in_block = ((req_addr & BLOCK_MASK) == BLOCK_BASE);
    wire [5:0] reg_idx = req_addr[7:2];

    // Backing scratch RAM for R/W-only slots. Read-only / sideband-fed
    // slots override the read mux below.
    logic [31:0] scratch [0:63];

    // ---- Read mux ---------------------------------------------------
    always_comb begin
        if (!in_block) begin
            req_readdata = 32'h0000_0000;
        end else begin
            unique case (reg_idx)
                IDX_ID:          req_readdata = 32'h1FFA_0001;
                IDX_STATUS:      req_readdata = {28'd0, 2'b00, status_busy_i, status_error_i};
                IDX_ERROR_INFO:  req_readdata = error_info_i;
                IDX_VSYNC_COUNT: req_readdata = vsync_count_i;
                IDX_FRAME_COUNT: req_readdata = frame_count_i;
                IDX_VIDEO_INFO:  req_readdata = {4'd0, hdmi_height_i, 4'd0, hdmi_width_i};
                IDX_FB_STATE:    req_readdata = fb_state_i;
                IDX_RING_HEAD:   req_readdata = ring_head_i;
                IDX_FENCE_VALUE: req_readdata = fence_value_i;
                IDX_LAYER_DEBUG: req_readdata = layer_descriptors_i;
                default:         req_readdata = scratch[reg_idx];
            endcase
        end
    end

    // ---- Write decode ----------------------------------------------
    integer i;
    logic clear_error_q;
    logic ring_kick_q;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            for (i = 0; i < 64; i = i + 1) scratch[i] <= 32'b0;
            // Default framebuffer params so the framework's scanout has
            // valid timing immediately at reset (otherwise FB_WIDTH=0
            // produces no HDMI signal). The host overrides these via
            // configure_framebuffer() once it's read VIDEO_INFO.
            scratch[IDX_FB_WIDTH]  <= 32'd1920;
            scratch[IDX_FB_HEIGHT] <= 32'd1080;
            scratch[IDX_FB_STRIDE] <= 32'd7680;     // 1920 × 4
            scratch[IDX_FB0_ADDR]  <= 32'h3000_0000;
            scratch[IDX_FB1_ADDR]  <= 32'h3080_0000;
            scratch[IDX_FB2_ADDR]  <= 32'h3100_0000;
            scratch[IDX_WALLPAPER_ADDR] <= 32'h3200_0000; // tex-pool region; host overrides
            scratch[IDX_CONTENT_MASK_ADDR] <= 32'h0; // host sets before enabling CONTROL[9]
            clear_error_q <= 1'b0;
            ring_kick_q   <= 1'b0;
        end else begin
            // Pulses default to deasserted each cycle.
            clear_error_q <= 1'b0;
            ring_kick_q   <= 1'b0;

            if (req_write & in_block) begin
                unique case (reg_idx)
                    // Read-only / sideband-fed slots: drop writes.
                    IDX_ID, IDX_STATUS, IDX_ERROR_INFO,
                    IDX_VSYNC_COUNT, IDX_FRAME_COUNT,
                    IDX_VIDEO_INFO, IDX_FB_STATE,
                    IDX_RING_HEAD, IDX_FENCE_VALUE,
                    IDX_LAYER_DEBUG: ;

                    IDX_CONTROL: begin
                        if (req_byteenable[0]) begin
                            // Bit 0 = EN: latched into scratch.
                            scratch[IDX_CONTROL][0] <= req_writedata[0];
                            // Bit 2 = CE: pulse, do not latch.
                            if (req_writedata[2]) clear_error_q <= 1'b1;
                            // Bit 1 = SE (soft reset) — M2c+; leave low.
                        end
                        if (req_byteenable[1]) scratch[IDX_CONTROL][15:8]  <= req_writedata[15:8];
                        if (req_byteenable[2]) scratch[IDX_CONTROL][23:16] <= req_writedata[23:16];
                        if (req_byteenable[3]) scratch[IDX_CONTROL][31:24] <= req_writedata[31:24];
                    end

                    IDX_RING_KICK: begin
                        // Pulse only — value written is ignored per spec.
                        ring_kick_q <= 1'b1;
                    end

                    default: begin
                        if (req_byteenable[0]) scratch[reg_idx][7:0]   <= req_writedata[7:0];
                        if (req_byteenable[1]) scratch[reg_idx][15:8]  <= req_writedata[15:8];
                        if (req_byteenable[2]) scratch[reg_idx][23:16] <= req_writedata[23:16];
                        if (req_byteenable[3]) scratch[reg_idx][31:24] <= req_writedata[31:24];
                    end
                endcase
            end
        end
    end

    // ---- Sideband outs ---------------------------------------------
    assign enable_o      = scratch[IDX_CONTROL][0];
    assign clear_error_o = clear_error_q;
    assign ring_base_o   = scratch[IDX_RING_BASE];
    assign ring_size_o   = scratch[IDX_RING_SIZE];
    assign ring_tail_o   = scratch[IDX_RING_TAIL];
    assign ring_kick_o   = ring_kick_q;
    assign fb0_addr_o    = scratch[IDX_FB0_ADDR];
    assign fb1_addr_o    = scratch[IDX_FB1_ADDR];
    assign fb2_addr_o    = scratch[IDX_FB2_ADDR];
    assign fb_width_o      = scratch[IDX_FB_WIDTH][11:0];
    assign fb_height_o     = scratch[IDX_FB_HEIGHT][11:0];
    assign fb_stride_o     = scratch[IDX_FB_STRIDE][13:0];
    assign tex_table_addr_o = scratch[IDX_TEX_TABLE_ADDR];
    assign layer_table_base_o = scratch[IDX_LAYER_TABLE_BASE];
    assign layer_active_o     = scratch[IDX_LAYER_COMMIT][31];
    assign layer_count_o      = scratch[IDX_LAYER_COMMIT][8:0];
    assign wallpaper_addr_o   = scratch[IDX_WALLPAPER_ADDR];
    assign composite_en_o     = scratch[IDX_CONTROL][8];
    assign content_mask_addr_o = scratch[IDX_CONTENT_MASK_ADDR];
    assign content_mask_en_o   = scratch[IDX_CONTROL][9];

    // Suppress unused-input warnings.
    wire _unused = &{1'b0, req_read, 1'b0};

endmodule
