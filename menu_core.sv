//============================================================================
//
//  1FPGA menu-core — glue module (`emu`).
//
//  This is the core-specific half of a MiSTer FPGA core. It configures the
//  framework's built-in DDR3 framebuffer scanout (MISTER_FB) for 1920x1080
//  @ 32bpp BGRA and points it at the reserved carve-out at 0x30000000 —
//  the same region defined in cores/menu-core/PROTOCOL.md §2.
//
//  For the M1 "black screen" milestone this module does NOT yet instantiate
//  a blit engine, command ring fetcher, or custom control registers. The
//  ARM side writes BGRA pixels directly into 0x30000000..0x30800000 and the
//  framework's ASCAL scales them out to HDMI. Later revisions will add the
//  blit engine, LW_H2F control register block, and command ring consumer.
//
//  Derived from MiSTer-devel/Template_MiSTer/Template.sv
//  (commit cce023f4ea34a5088a5ce5b45c90ad2a4493c6ac).
//
//  Licensed under GPLv2 — see ../../LICENSE (this subtree only, because
//  the vendored sys/ framework is GPLv2). The Rust host in src/menu-core
//  remains Apache-2.0 as the overall project.
//
//============================================================================

module emu
(
	// Master input clock.
	input         CLK_50M,

	// Async reset from sys_top.
	input         RESET,

	// Framework HPS bus (passed straight through to hps_io).
	inout  [48:0] HPS_BUS,

	// Base video clock, should match CLK_SYS.
	output        CLK_VIDEO,

	// Pixel clock enable on CLK_VIDEO.
	output        CE_PIXEL,

	// Aspect ratio hints (ignored here because MISTER_FB drives scanout).
	output [12:0] VIDEO_ARX,
	output [12:0] VIDEO_ARY,

	// Analog / pre-ASCAL video inputs — unused in a pure MISTER_FB core.
	output  [7:0] VGA_R,
	output  [7:0] VGA_G,
	output  [7:0] VGA_B,
	output        VGA_HS,
	output        VGA_VS,
	output        VGA_DE,
	output        VGA_F1,
	output [1:0]  VGA_SL,
	output        VGA_SCALER,
	output        VGA_DISABLE,

	// Framework tells us the current HDMI output dimensions.
	input  [11:0] HDMI_WIDTH,
	input  [11:0] HDMI_HEIGHT,
	output        HDMI_FREEZE,
	output        HDMI_BLACKOUT,
	output        HDMI_BOB_DEINT,

`ifdef MISTER_FB
	// DDR3-backed framebuffer (see Template.sv comments for FB_FORMAT bits).
	output        FB_EN,
	output  [4:0] FB_FORMAT,
	output [11:0] FB_WIDTH,
	output [11:0] FB_HEIGHT,
	output [31:0] FB_BASE,
	output [13:0] FB_STRIDE,
	input         FB_VBL,
	input         FB_LL,
	output        FB_FORCE_BLANK,

`ifdef MISTER_FB_PALETTE
	output        FB_PAL_CLK,
	output  [7:0] FB_PAL_ADDR,
	output [23:0] FB_PAL_DOUT,
	input  [23:0] FB_PAL_DIN,
	output        FB_PAL_WR,
`endif
`endif

	output        LED_USER,
	output  [1:0] LED_POWER,
	output  [1:0] LED_DISK,

	output  [1:0] BUTTONS,

	input         CLK_AUDIO,     // 24.576 MHz
	output [15:0] AUDIO_L,
	output [15:0] AUDIO_R,
	output        AUDIO_S,
	output  [1:0] AUDIO_MIX,

	inout   [3:0] ADC_BUS,

	output        SD_SCK,
	output        SD_MOSI,
	input         SD_MISO,
	output        SD_CS,
	input         SD_CD,

	// Framework DDRAM port — unused for now; the blit engine will hook in
	// here once implemented. For M1 we leave it idle; the framework's own
	// path into DDR3 handles FB_BASE scanout independently.
	output        DDRAM_CLK,
	input         DDRAM_BUSY,
	output  [7:0] DDRAM_BURSTCNT,
	output [28:0] DDRAM_ADDR,
	input  [63:0] DDRAM_DOUT,
	input         DDRAM_DOUT_READY,
	output        DDRAM_RD,
	output [63:0] DDRAM_DIN,
	output  [7:0] DDRAM_BE,
	output        DDRAM_WE,

	output        SDRAM_CLK,
	output        SDRAM_CKE,
	output [12:0] SDRAM_A,
	output  [1:0] SDRAM_BA,
	inout  [15:0] SDRAM_DQ,
	output        SDRAM_DQML,
	output        SDRAM_DQMH,
	output        SDRAM_nCS,
	output        SDRAM_nCAS,
	output        SDRAM_nRAS,
	output        SDRAM_nWE,

`ifdef MISTER_DUAL_SDRAM
	input         SDRAM2_EN,
	output        SDRAM2_CLK,
	output [12:0] SDRAM2_A,
	output  [1:0] SDRAM2_BA,
	inout  [15:0] SDRAM2_DQ,
	output        SDRAM2_nCS,
	output        SDRAM2_nCAS,
	output        SDRAM2_nRAS,
	output        SDRAM2_nWE,
`endif

	input         UART_CTS,
	output        UART_RTS,
	input         UART_RXD,
	output        UART_TXD,
	output        UART_DTR,
	input         UART_DSR,

	input   [6:0] USER_IN,
	output  [6:0] USER_OUT,

	input         OSD_STATUS
);

////////////////////////////////////////////////////////////////////////////
// Inactive / pass-through defaults for ports we don't drive yet.
////////////////////////////////////////////////////////////////////////////

assign ADC_BUS = 'Z;
assign USER_OUT = '1;
assign {UART_RTS, UART_TXD, UART_DTR} = 0;
assign {SD_SCK, SD_MOSI, SD_CS} = 'Z;
assign {SDRAM_DQ, SDRAM_A, SDRAM_BA, SDRAM_CLK, SDRAM_CKE, SDRAM_DQML, SDRAM_DQMH, SDRAM_nWE, SDRAM_nCAS, SDRAM_nRAS, SDRAM_nCS} = 'Z;
// DDRAM_* is driven by the M2b ring fetcher (see instantiation below).
assign DDRAM_CLK = clk_sys;

// Analog-side video.
//
// The compositor (rtl/compositor/compositor.sv) drives VGA_R/G/B/HS/
// VS/DE at the 148.5 MHz pixel clock; the framework's HDMI pipeline
// adapts to whatever timing we declare via CLK_VIDEO + CE_PIXEL.
// VIDEO_ARX/ARY are 16:9 because we always render at 1920×1080
// regardless of the user's HDMI mode (the framework scales / lets it
// out at native if matched).
//
// MISTER_FB stays compiled in (the framework expects the FB_* ports)
// but is held idle — see the FB_EN block below.
assign VGA_F1       = 1'b0;
assign VGA_SL       = 2'b00;
// ASCAL is the path from VGA_* to HDMI on DE10-Nano. Setting
// VGA_SCALER=1 routes our compositor pixels through it; with =0 the
// stream goes to the analog VGA output which the DE10-Nano doesn't
// have, so HDMI shows black. CE_PIXEL is driven by the compositor
// (always 1 in Phase 1; CLK_VIDEO runs at pixel rate).
assign VGA_SCALER   = 1'b1;
assign VGA_DISABLE  = 1'b0;  // VGA_* path is active
assign VIDEO_ARX    = 13'd16;
assign VIDEO_ARY    = 13'd9;
assign HDMI_FREEZE    = 1'b0;
assign HDMI_BLACKOUT  = 1'b0;
assign HDMI_BOB_DEINT = 1'b0;

// No audio in v0.
assign AUDIO_L   = 16'd0;
assign AUDIO_R   = 16'd0;
assign AUDIO_S   = 1'b0;
assign AUDIO_MIX = 2'b00;

// LEDs and buttons inactive.
assign LED_POWER = 2'b00;
assign LED_DISK  = 2'b00;
assign BUTTONS   = 2'b00;

////////////////////////////////////////////////////////////////////////////
// System clock.
//
// Cyclone V's clock-select blocks in sys_top require a PLL output on
// inclk[3] (synthesis error 15836 if driven by a raw input pin). Two
// outputs from the core PLL:
//   clk_sys (50 MHz)   — blit engine, ring fetcher, regs, layer_dma
//   clk_video (100 MHz) — compositor + scanline_filter, also exposed
//                          to the framework as CLK_VIDEO so ASCAL
//                          captures at the native-1080p pixel rate.
// The two are related clocks (same PLL); CDC paths are confined to
// the synchronisers below, which menu_core.sdc marks as false_paths.
////////////////////////////////////////////////////////////////////////////

wire clk_sys;     // 50 MHz: blit engine, ring fetcher, regs, layer_dma
wire clk_video;   // 100 MHz: compositor + scanline_filter
wire pll_locked;

pll pll_inst (
	.refclk   (CLK_50M),
	.rst      (1'b0),
	.outclk_0 (clk_sys),
	.outclk_1 (clk_video),
	.locked   (pll_locked)
);

assign CLK_VIDEO = clk_video;

////////////////////////////////////////////////////////////////////////////
// Compositor scanout → VGA_* → ASCAL → HDMI.
//
// Runs on clk_video (100 MHz) for native-1080p output. Walks the
// on-chip layer cache (filled by layer_dma on clk_sys each frame)
// and paints solid-colour layers over a black background, one
// pixel per clk_video cycle. The cache read port is owned by the
// compositor; during each HBlank it builds the active list for the
// next scanline (see compositor.sv + scanline_filter.sv), and during
// the active region a parallel comparator picks the topmost match.
//
// We deliberately bypass the framework's video_mixer (its always-
// synthesised scandoubler + 4×-pixel CLK_VIDEO rule made timing
// closure impossible at the rates we want for native 1080p).
////////////////////////////////////////////////////////////////////////////

wire [7:0] comp_r, comp_g, comp_b;
wire       comp_hs, comp_vs, comp_hb, comp_vb;
wire       comp_ce_pix;
wire [7:0]   comp_cache_slot;
wire [255:0] comp_cache_data;
// Texture sampler interface (Phase 2b step 2). Compositor (clk_video)
// produces these; texture_unit (clk_sys) consumes them via the CDC
// synchronisers below.
wire         comp_tex_kick;
wire [15:0]  comp_tex_id;
wire [15:0]  comp_tex_src_x;
wire [15:0]  comp_tex_ty;
wire [11:0]  comp_tex_dst_w;
// Line buffer read port (clk_video).
wire [9:0]   comp_line_buf_addr;
wire [63:0]  comp_line_buf_data;

// Reset bridge: pll_locked is asynchronous to clk_video, so feeding
// it raw as rst_n would risk recovery/removal violations at the
// faster clock. Standard pattern: assert async (low), deassert sync.
(* preserve *) logic comp_rst_n_sync_0;
(* preserve *) logic comp_rst_n_sync_1;
always_ff @(posedge clk_video or negedge pll_locked) begin
    if (!pll_locked) begin
        comp_rst_n_sync_0 <= 1'b0;
        comp_rst_n_sync_1 <= 1'b0;
    end else begin
        comp_rst_n_sync_0 <= 1'b1;
        comp_rst_n_sync_1 <= comp_rst_n_sync_0;
    end
end
wire comp_rst_n = comp_rst_n_sync_1;

// CDC: clk_sys-owned `reg_layer_count` (9 bits) into the clk_video
// domain. layer_count only changes on `LAYER_COMMIT` writes, which
// are bursty (once per frame, separated by millions of clk_video
// cycles), so the two-flop synchronisers see a stable value with
// vanishing probability of mid-transition bit-mixing. Marked false-
// path in menu_core.sdc so Quartus doesn't try to time the
// inter-domain leg.
(* preserve *) logic [8:0] layer_count_sync_0;
(* preserve *) logic [8:0] layer_count_sync_1;
always_ff @(posedge clk_video) begin
    layer_count_sync_0 <= reg_layer_count;
    layer_count_sync_1 <= layer_count_sync_0;
end

compositor u_compositor (
	.clk             (clk_video),
	.rst_n           (comp_rst_n),
	.ce_pix          (comp_ce_pix),
	.r               (comp_r),
	.g               (comp_g),
	.b               (comp_b),
	.hsync           (comp_hs),
	.vsync           (comp_vs),
	.hblank          (comp_hb),
	.vblank          (comp_vb),
	.cache_slot_o    (comp_cache_slot),
	.cache_data_i    (comp_cache_data),
	.layer_count_i   (layer_count_sync_1),
	.tex_kick_o      (comp_tex_kick),
	.tex_id_o        (comp_tex_id),
	.tex_src_x_o     (comp_tex_src_x),
	.tex_ty_o        (comp_tex_ty),
	.tex_dst_w_o     (comp_tex_dst_w),
	.line_buf_addr_o (comp_line_buf_addr),
	.line_buf_data_i (comp_line_buf_data)
);

// Compositor drives VGA_* directly. CE_PIXEL is held high because
// CLK_VIDEO == pixel rate; ASCAL captures every clock.
assign VGA_R   = comp_r;
assign VGA_G   = comp_g;
assign VGA_B   = comp_b;
assign VGA_HS  = comp_hs;
assign VGA_VS  = comp_vs;
assign VGA_DE  = ~(comp_hb | comp_vb);
assign CE_PIXEL = comp_ce_pix; // currently always 1; kept symbolic

////////////////////////////////////////////////////////////////////////////
// HPS I/O.
//
// Minimal CONF_STR: the core has no user-visible settings yet. The ARM-side
// host driver (src/menu-core) configures everything via the custom control
// register block (to be added in a future revision); for M1 the framework
// just needs hps_io present so the OSD/core infrastructure is wired up.
////////////////////////////////////////////////////////////////////////////

`include "build_id.v"
localparam CONF_STR = {
	"MENU_CORE;;",
	"-;",
	"V,v", `BUILD_DATE
};

wire [127:0] status;
wire   [1:0] buttons_hps;
wire         forced_scandoubler;

hps_io #(.CONF_STR(CONF_STR)) hps_io
(
	.clk_sys(clk_sys),
	.HPS_BUS(HPS_BUS),
	.EXT_BUS(),
	.gamma_bus(),

	.forced_scandoubler(forced_scandoubler),

	.buttons(buttons_hps),
	.status(status)
);

// `status` and `buttons_hps` are currently unused beyond the hps_io wiring.
// Suppress "unused" warnings explicitly for clarity when Quartus lints.
wire _unused_ok = &{1'b0, status, buttons_hps, forced_scandoubler,
                    FB_LL, OSD_STATUS,
                    UART_CTS, UART_RXD, UART_DSR, USER_IN, SD_MISO, SD_CD,
                    RESET, pll_locked, 1'b0};

////////////////////////////////////////////////////////////////////////////
// LW_H2F control register file + ring fetcher (M2a + M2b).
//
// We instantiate the LW_H2F HPS hard-IP primitive ourselves — the MiSTer
// `sys/` framework does not. The bridge module exposes a small single-
// beat request interface to `menu_core_regs`, which decodes PROTOCOL.md
// §3.1 register offsets. The fetcher reads the command ring via DDRAM_*
// and updates RING_HEAD / FENCE_VALUE / FRAME_COUNT through the
// register file's sideband ports.
////////////////////////////////////////////////////////////////////////////

wire [20:0] reg_addr;
wire        reg_read;
wire        reg_write;
wire [31:0] reg_writedata;
wire [3:0]  reg_byteenable;
wire [31:0] reg_readdata;

wire        reg_enable;
wire        reg_clear_error;
wire [31:0] reg_ring_base;
wire [31:0] reg_ring_size;
wire [31:0] reg_ring_tail;
wire        reg_ring_kick;
wire [31:0] reg_fb0_addr;
wire [31:0] reg_fb1_addr;
wire [31:0] reg_fb2_addr;
wire [11:0] reg_fb_width;
wire [11:0] reg_fb_height;
wire [13:0] reg_fb_stride;
wire [31:0] reg_tex_table_addr;
wire [31:0] reg_layer_table_base;
wire        reg_layer_active;
wire [8:0]  reg_layer_count;
wire [31:0] fetcher_ring_head;
wire [31:0] fetcher_fence_value;
wire [31:0] fetcher_error_info;
wire        fetcher_status_busy;
wire        fetcher_status_error;
wire        fetcher_present_pulse;

wire [1:0]  swap_display_idx;
wire [1:0]  swap_render_idx;
wire [31:0] swap_fb_state;
wire [31:0] swap_frame_count;
wire [31:0] swap_vsync_count;

// CONTROL.CE (clear-error pulse) re-arms the fetcher: it leaves S_HALT
// and clears the error bit. We OR this into the fetcher's reset.
wire fetcher_rst_n = (~RESET) & ~reg_clear_error;

lwh2f_bridge u_lwh2f_bridge (
    .clk            (clk_sys),
    .rst_n          (~RESET),

    .req_addr       (reg_addr),
    .req_read       (reg_read),
    .req_write      (reg_write),
    .req_writedata  (reg_writedata),
    .req_byteenable (reg_byteenable),
    .req_readdata   (reg_readdata)
);

menu_core_regs u_menu_core_regs (
    .clk            (clk_sys),
    .rst_n          (~RESET),

    .req_addr       (reg_addr),
    .req_read       (reg_read),
    .req_write      (reg_write),
    .req_writedata  (reg_writedata),
    .req_byteenable (reg_byteenable),
    .req_readdata   (reg_readdata),

    .enable_o       (reg_enable),
    .clear_error_o  (reg_clear_error),
    .ring_base_o    (reg_ring_base),
    .ring_size_o    (reg_ring_size),
    .ring_tail_o    (reg_ring_tail),
    .ring_kick_o    (reg_ring_kick),

    .fb0_addr_o     (reg_fb0_addr),
    .fb1_addr_o     (reg_fb1_addr),
    .fb2_addr_o     (reg_fb2_addr),
    .fb_width_o     (reg_fb_width),
    .fb_height_o    (reg_fb_height),
    .fb_stride_o    (reg_fb_stride),
    .tex_table_addr_o (reg_tex_table_addr),

    .layer_table_base_o (reg_layer_table_base),
    .layer_active_o     (reg_layer_active),
    .layer_count_o      (reg_layer_count),

    .layer_descriptors_i (layer_dma_descriptors),

    .ring_head_i    (fetcher_ring_head),
    .fence_value_i  (fetcher_fence_value),
    .frame_count_i  (swap_frame_count),
    .vsync_count_i  (swap_vsync_count),
    .fb_state_i     (swap_fb_state),
    .error_info_i   (fetcher_error_info),
    .status_busy_i  (fetcher_status_busy),
    .status_error_i (fetcher_status_error),
    .hdmi_width_i   (HDMI_WIDTH),
    .hdmi_height_i  (HDMI_HEIGHT)
);

// Fetcher's read-side DDRAM signals.
wire [28:0] fetch_addr;
wire [7:0]  fetch_burstcnt;
wire [7:0]  fetch_be;
wire        fetch_rd;

// Blit engine's DDRAM signals (now both read + write since COPY_RECT
// reads source pixels through this same port).
wire [28:0] blit_addr;
wire [7:0]  blit_burstcnt;
wire [7:0]  blit_be;
wire [63:0] blit_din;
wire        blit_we;
wire        blit_rd;
wire        blit_busy;

// Blit dispatch from fetcher.
wire        blit_start;
wire        blit_mode;
wire [1:0]  blit_blend;
wire [15:0] blit_dst_x, blit_dst_y, blit_dst_w, blit_dst_h;
wire [15:0] blit_src_x, blit_src_y;
wire [31:0] blit_src_addr, blit_src_pitch;
wire [31:0] blit_color;
wire        blit_format;
wire        blit_tint_en;
wire [31:0] blit_tint_color;
wire        blit_clip_en;
wire [15:0] blit_clip_x, blit_clip_y, blit_clip_w, blit_clip_h;
wire        blit_ignore_clip;
wire        blit_done;

// Active render target — driven by ring_fetcher, fed into blit_engine.
// Defaults to the framebuffer geometry; SET_RENDER_TARGET (PROTOCOL.md
// §5.6) re-points it at a texture's pixel data.
wire [31:0] target_base;
wire [31:0] target_pitch;
wire [15:0] target_width;
wire [15:0] target_height;

ring_fetcher u_ring_fetcher (
    .clk             (clk_sys),
    .rst_n           (fetcher_rst_n),

    .enable_i        (reg_enable),
    .ring_base_i     (reg_ring_base),
    .ring_size_i     (reg_ring_size),
    .ring_tail_i     (reg_ring_tail),
    .ring_head_o     (fetcher_ring_head),
    .fence_value_o   (fetcher_fence_value),
    .error_info_o    (fetcher_error_info),
    .status_busy_o   (fetcher_status_busy),
    .status_error_o  (fetcher_status_error),

    .present_pulse_o (fetcher_present_pulse),

    .tex_table_addr_i (reg_tex_table_addr),
    .fb_base_i       (blit_fb_base),
    .fb_stride_i     ({18'd0, reg_fb_stride}),
    .fb_width_i      ({4'd0,  reg_fb_width}),
    .fb_height_i     ({4'd0,  reg_fb_height}),

    .target_base_o   (target_base),
    .target_pitch_o  (target_pitch),
    .target_width_o  (target_width),
    .target_height_o (target_height),

    .blit_start_o    (blit_start),
    .blit_mode_o     (blit_mode),
    .blit_blend_o    (blit_blend),
    .blit_dst_x_o    (blit_dst_x),
    .blit_dst_y_o    (blit_dst_y),
    .blit_dst_w_o    (blit_dst_w),
    .blit_dst_h_o    (blit_dst_h),
    .blit_color_o    (blit_color),
    .blit_src_x_o    (blit_src_x),
    .blit_src_y_o    (blit_src_y),
    .blit_src_addr_o (blit_src_addr),
    .blit_src_pitch_o(blit_src_pitch),
    .blit_format_o     (blit_format),
    .blit_tint_en_o    (blit_tint_en),
    .blit_tint_color_o (blit_tint_color),
    .blit_clip_en_o    (blit_clip_en),
    .blit_clip_x_o     (blit_clip_x),
    .blit_clip_y_o     (blit_clip_y),
    .blit_clip_w_o     (blit_clip_w),
    .blit_clip_h_o     (blit_clip_h),
    .blit_ignore_clip_o(blit_ignore_clip),
    .blit_done_i     (blit_done),

    .ddram_addr_o       (fetch_addr),
    .ddram_burstcnt_o   (fetch_burstcnt),
    .ddram_be_o         (fetch_be),
    .ddram_rd_o         (fetch_rd),
    .ddram_busy_i       (DDRAM_BUSY),
    .ddram_dout_i       (DDRAM_DOUT),
    .ddram_dout_valid_i (DDRAM_DOUT_READY)
);

// Address selection: scanout reads from FB[display_idx], blit writes to
// FB[render_idx]. Each takes one of three host-programmed addresses;
// the indices come from fb_swapper.
function automatic logic [31:0] fb_addr_select(
    input logic [1:0] idx,
    input logic [31:0] addr0,
    input logic [31:0] addr1,
    input logic [31:0] addr2
);
    unique case (idx)
        2'd0:    fb_addr_select = addr0;
        2'd1:    fb_addr_select = addr1;
        2'd2:    fb_addr_select = addr2;
        default: fb_addr_select = addr0;
    endcase
endfunction

wire [31:0] scanout_fb_base = fb_addr_select(swap_display_idx, reg_fb0_addr, reg_fb1_addr, reg_fb2_addr);
wire [31:0] blit_fb_base    = fb_addr_select(swap_render_idx,  reg_fb0_addr, reg_fb1_addr, reg_fb2_addr);

// Vsync pulse: rising edge of FB_VBL, double-flop synchronised.
logic fb_vbl_d0, fb_vbl_d1;
always_ff @(posedge clk_sys) begin
    fb_vbl_d0 <= FB_VBL;
    fb_vbl_d1 <= fb_vbl_d0;
end
wire vsync_pulse = fb_vbl_d0 & ~fb_vbl_d1;

fb_swapper u_fb_swapper (
    .clk             (clk_sys),
    .rst_n           (fetcher_rst_n),

    .present_pulse_i (fetcher_present_pulse),
    .vsync_pulse_i   (vsync_pulse),

    .display_idx_o   (swap_display_idx),
    .render_idx_o    (swap_render_idx),
    .ready_idx_o     (),
    .fb_state_o      (swap_fb_state),
    .frame_count_o   (swap_frame_count),
    .vsync_count_o   (swap_vsync_count)
);

blit_engine u_blit_engine (
    .clk        (clk_sys),
    .rst_n      (fetcher_rst_n),

    .start_i    (blit_start),
    .mode_i     (blit_mode),
    .blend_i    (blit_blend),
    .dst_x_i    (blit_dst_x),
    .dst_y_i    (blit_dst_y),
    .dst_w_i    (blit_dst_w),
    .dst_h_i    (blit_dst_h),
    .color_i    (blit_color),
    .src_x_i    (blit_src_x),
    .src_y_i    (blit_src_y),
    .src_addr_i (blit_src_addr),
    .src_pitch_i(blit_src_pitch),
    .format_i      (blit_format),
    .tint_en_i     (blit_tint_en),
    .tint_color_i  (blit_tint_color),

    .target_width_i  (target_width),
    .target_height_i (target_height),
    .clip_en_i     (blit_clip_en),
    .clip_x_i      (blit_clip_x),
    .clip_y_i      (blit_clip_y),
    .clip_w_i      (blit_clip_w),
    .clip_h_i      (blit_clip_h),
    .ignore_clip_i (blit_ignore_clip),

    .target_base_i  (target_base),
    .target_pitch_i (target_pitch),

    .busy_o     (blit_busy),
    .done_o     (blit_done),

    .ddram_addr_o       (blit_addr),
    .ddram_burstcnt_o   (blit_burstcnt),
    .ddram_be_o         (blit_be),
    .ddram_din_o        (blit_din),
    .ddram_we_o         (blit_we),
    .ddram_rd_o         (blit_rd),
    .ddram_busy_i       (DDRAM_BUSY),
    .ddram_dout_i       (DDRAM_DOUT),
    .ddram_dout_valid_i (DDRAM_DOUT_READY)
);

////////////////////////////////////////////////////////////////////////////
// Layer-cache + DMA (Phase 2a step 2/3).
//
// On every vsync rising edge, layer_dma fetches `layer_count`
// descriptors starting at the active half of `layer_table_base`
// (PROTOCOL.md §11.2) and writes them into layer_cache. The
// compositor reads the cache through its `cache_slot_o` /
// `cache_data_i` port during each HBlank to build a per-scanline
// active list (see scanline_filter.sv).
////////////////////////////////////////////////////////////////////////////

// CDC: clk_video-owned `comp_vs` sampled into clk_sys for layer_dma's
// start_i pulse. Two-flop synchroniser + a third register to detect
// the rising edge in the destination domain. comp_vs holds high for
// V_SYNC × H_TOTAL clk_video cycles (= ~4400 cycles for 1080p
// timing = ~2200 clk_sys cycles), so the slower clock catches every
// rising edge with comfortable margin.
(* preserve *) logic comp_vs_sync_0;
(* preserve *) logic comp_vs_sync_1;
logic comp_vs_sync_2;
always_ff @(posedge clk_sys) begin
    comp_vs_sync_0 <= comp_vs;
    comp_vs_sync_1 <= comp_vs_sync_0;
    comp_vs_sync_2 <= comp_vs_sync_1;
end
wire vsync_rising = comp_vs_sync_1 & ~comp_vs_sync_2;

// Active-table base = layer_table_base + (active ? 0x2000 : 0x0).
// 0x2000 = LAYER_TABLE_SIZE = 256 * 32 bytes. All clk_sys-domain
// signals — no CDC needed.
wire [31:0] active_layer_base = reg_layer_table_base
                              + (reg_layer_active ? 32'h0000_2000 : 32'd0);

// layer_dma (clk_sys) → layer_cache write port (clk_sys).
// Compositor (clk_video) → layer_cache read port (clk_video).
// The BRAM straddles both domains; see layer_cache.sv for the
// independent-clock dual-port arrangement.
wire [7:0]   cache_wr_slot;
wire [255:0] cache_wr_data;
wire         cache_wr_en;

layer_cache u_layer_cache (
    .wr_clk     (clk_sys),
    .wr_slot_i  (cache_wr_slot),
    .wr_data_i  (cache_wr_data),
    .wr_en_i    (cache_wr_en),
    .rd_clk     (clk_video),
    .rd_slot_i  (comp_cache_slot),
    .rd_data_o  (comp_cache_data)
);

////////////////////////////////////////////////////////////////////////////
// Texture unit + line buffer (Phase 2b step 2).
//
// The compositor (clk_video) identifies the topmost textured layer in
// the active list after the scanline_filter completes, and pulses
// `comp_tex_kick` with the descriptor params held stable for ~60
// clk_video cycles. The texture_unit on clk_sys samples this via a
// 2-flop synchroniser, edge-detects the rising edge, and runs its
// own state machine: fetch the texture descriptor, then burst a row
// of pixels into the line_buffer. The painter then reads the line
// buffer during active scanout.
////////////////////////////////////////////////////////////////////////////

// CDC: kick pulse from clk_video to clk_sys.
(* preserve *) logic tex_kick_sync_0;
(* preserve *) logic tex_kick_sync_1;
logic tex_kick_sync_2;
always_ff @(posedge clk_sys) begin
    tex_kick_sync_0 <= comp_tex_kick;
    tex_kick_sync_1 <= tex_kick_sync_0;
    tex_kick_sync_2 <= tex_kick_sync_1;
end
wire tex_kick_rising = tex_kick_sync_1 & ~tex_kick_sync_2;

// CDC: multi-bit params (stable while comp_tex_kick is high, which
// is at least 60 clk_video cycles = 30 clk_sys cycles).
(* preserve *) logic [15:0] tex_id_sync_0,    tex_id_sync_1;
(* preserve *) logic [15:0] tex_src_x_sync_0, tex_src_x_sync_1;
(* preserve *) logic [15:0] tex_ty_sync_0,    tex_ty_sync_1;
(* preserve *) logic [11:0] tex_dst_w_sync_0, tex_dst_w_sync_1;
always_ff @(posedge clk_sys) begin
    tex_id_sync_0    <= comp_tex_id;    tex_id_sync_1    <= tex_id_sync_0;
    tex_src_x_sync_0 <= comp_tex_src_x; tex_src_x_sync_1 <= tex_src_x_sync_0;
    tex_ty_sync_0    <= comp_tex_ty;    tex_ty_sync_1    <= tex_ty_sync_0;
    tex_dst_w_sync_0 <= comp_tex_dst_w; tex_dst_w_sync_1 <= tex_dst_w_sync_0;
end

// Line buffer: wr_clk = clk_sys (texture_unit), rd_clk = clk_video
// (painter). 64-bit wide (2 pixels per entry).
wire [9:0]  line_buf_wr_addr;
wire [63:0] line_buf_wr_data;
wire        line_buf_we;

line_buffer u_line_buffer (
    .wr_clk    (clk_sys),
    .wr_addr_i (line_buf_wr_addr),
    .wr_data_i (line_buf_wr_data),
    .wr_en_i   (line_buf_we),
    .rd_clk    (clk_video),
    .rd_addr_i (comp_line_buf_addr),
    .rd_data_o (comp_line_buf_data)
);

// Texture unit: shares the DDR3 bus through the 4-way arbiter below.
wire [28:0] tex_unit_addr;
wire [7:0]  tex_unit_burstcnt;
wire [7:0]  tex_unit_be;
wire        tex_unit_rd;
wire        tex_unit_busy;
wire        tex_unit_done;

texture_unit u_texture_unit (
    .clk              (clk_sys),
    .rst_n            (fetcher_rst_n),
    .kick_i           (tex_kick_rising),
    .tex_id_i         (tex_id_sync_1),
    .ty_i             (tex_ty_sync_1),
    .src_x_i          (tex_src_x_sync_1),
    .dst_w_i          (tex_dst_w_sync_1),
    .tex_table_addr_i (reg_tex_table_addr),
    .line_buf_addr_o  (line_buf_wr_addr),
    .line_buf_data_o  (line_buf_wr_data),
    .line_buf_we_o    (line_buf_we),
    .ddram_addr_o     (tex_unit_addr),
    .ddram_burstcnt_o (tex_unit_burstcnt),
    .ddram_be_o       (tex_unit_be),
    .ddram_rd_o       (tex_unit_rd),
    .ddram_busy_i     (DDRAM_BUSY),
    .ddram_dout_i     (DDRAM_DOUT),
    .ddram_dout_valid_i (DDRAM_DOUT_READY),
    .busy_o           (tex_unit_busy),
    .done_pulse_o     (tex_unit_done)
);

// layer_dma → DDRAM master signals.
wire [28:0] layer_dma_addr;
wire [7:0]  layer_dma_burstcnt;
wire [7:0]  layer_dma_be;
wire        layer_dma_rd;
wire        layer_dma_busy;
wire        layer_dma_done;
wire [31:0] layer_dma_descriptors;

layer_dma u_layer_dma (
    .clk          (clk_sys),
    .rst_n        (fetcher_rst_n),
    .start_i      (vsync_rising),
    .base_i       (active_layer_base),
    .count_i      (reg_layer_count),
    .cache_slot_o (cache_wr_slot),
    .cache_data_o (cache_wr_data),
    .cache_we_o   (cache_wr_en),
    .ddram_addr_o       (layer_dma_addr),
    .ddram_burstcnt_o   (layer_dma_burstcnt),
    .ddram_be_o         (layer_dma_be),
    .ddram_rd_o         (layer_dma_rd),
    .ddram_busy_i       (DDRAM_BUSY),
    .ddram_dout_i       (DDRAM_DOUT),
    .ddram_dout_valid_i (DDRAM_DOUT_READY),
    .busy_o             (layer_dma_busy),
    .done_pulse_o       (layer_dma_done),
    .descriptors_o      (layer_dma_descriptors)
);

// DDRAM_* mux. Priority: blit_engine > layer_dma > texture_unit >
// ring_fetcher. blit_engine has hard real-time deadlines through the
// fence pipeline; layer_dma fires once per frame in VBlank;
// texture_unit fires once per scanline in HBlank. The fetcher is the
// background polling master that owns the bus whenever nothing else
// needs it. Reads return on a shared dout bus — each consumer only
// samples valid pulses while it owns the bus, so cross-talk is not
// possible.
wire layer_dma_owns_bus = layer_dma_busy & ~blit_busy;
wire tex_unit_owns_bus  = tex_unit_busy & ~blit_busy & ~layer_dma_busy;

assign DDRAM_ADDR     = blit_busy          ? blit_addr
                      : layer_dma_owns_bus ? layer_dma_addr
                      : tex_unit_owns_bus  ? tex_unit_addr
                      : fetch_addr;
assign DDRAM_BURSTCNT = blit_busy          ? blit_burstcnt
                      : layer_dma_owns_bus ? layer_dma_burstcnt
                      : tex_unit_owns_bus  ? tex_unit_burstcnt
                      : fetch_burstcnt;
assign DDRAM_BE       = blit_busy          ? blit_be
                      : layer_dma_owns_bus ? layer_dma_be
                      : tex_unit_owns_bus  ? tex_unit_be
                      : fetch_be;
assign DDRAM_DIN      = blit_busy ? blit_din : 64'd0;
assign DDRAM_RD       = blit_busy          ? blit_rd
                      : layer_dma_owns_bus ? layer_dma_rd
                      : tex_unit_owns_bus  ? tex_unit_rd
                      : fetch_rd;
assign DDRAM_WE       = blit_busy ? blit_we : 1'b0;

// reg_ring_kick is currently advisory — the fetcher polls RING_TAIL
// every cycle anyway. Wire-suppress to avoid unused warnings until
// M2c+ lets it gate a low-power idle.
wire _unused_kick = reg_ring_kick;

// layer_dma_done and tex_unit_done aren't consumed (we rely on the
// HBlank budget being wide enough to guarantee completion by the
// start of active scanout). Wire-suppress.
wire _unused_layer = &{1'b0, layer_dma_done, tex_unit_done, 1'b0};

////////////////////////////////////////////////////////////////////////////
// MISTER_FB configuration. FB_FORMAT selects BGR 32bpp so the framework
// reads our BGRA8888 framebuffer directly (PROTOCOL.md §2.1, §7.1).
// All other geometry comes from the host via control registers
// (FB0_ADDR / FB_WIDTH / FB_HEIGHT / FB_STRIDE) so the menu can render
// pixel-perfect for the active HDMI mode (read from VIDEO_INFO).
////////////////////////////////////////////////////////////////////////////

`ifdef MISTER_FB
// Compositor scanout drives HDMI via VGA_* → ASCAL; MISTER_FB stays idle.
// Sentinel zero values keep the framework from latching stale FB
// geometry.
//
// FB_FORCE_BLANK MUST be 0 even though MISTER_FB is unused. In
// sys_top.v the signal ANDs into the HDMI shadowmask (`dis_output`,
// line 1134/1155): `dis <= fb_force_blank & ~LFB_EN; .din(dis_output
// ? 24'd0 : hdmi_data)`. With LFB_EN low (Linux fb not in use) and
// FB_FORCE_BLANK=1, the framework masks every HDMI pixel to zero —
// even when our compositor is driving VGA_* correctly through ASCAL.
// 0 lets ASCAL's pixels reach the HDMI transmitter.
assign FB_EN          = 1'b0;
assign FB_FORMAT      = 5'b00000;
assign FB_WIDTH       = 12'd0;
assign FB_HEIGHT      = 12'd0;
assign FB_BASE        = 32'd0;
assign FB_STRIDE      = 14'd0;
assign FB_FORCE_BLANK = 1'b0;
`endif

////////////////////////////////////////////////////////////////////////////
// Activity LED — heartbeat that doubles as visual confirmation that the
// host has written CONTROL[0]. When `reg_enable` is low we use the slow
// breathing pattern; when high we switch to a faster fixed-rate blink.
////////////////////////////////////////////////////////////////////////////

reg [26:0] act_cnt;
always @(posedge clk_sys) act_cnt <= act_cnt + 27'd1;

wire breathe = act_cnt[26] ? (act_cnt[25:18] > act_cnt[7:0])
                           : (act_cnt[25:18] <= act_cnt[7:0]);
wire fast_blink = act_cnt[22];

// Diagnostic: solid LED_USER means the video PLL is locked (so the
// compositor is producing pixels). If the screen is black AND the
// LED is the breathe/blink pattern, PLL never locked → check PLL
// params. If LED is solid but screen still black, the timing is
// reaching ASCAL but ASCAL/HDMI sink isn't accepting it.
assign LED_USER = pll_locked ? 1'b1
                             : (reg_enable ? fast_blink : breathe);

endmodule
