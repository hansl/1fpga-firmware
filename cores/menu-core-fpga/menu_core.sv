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
//  Licensed under GPLv2 — see ./LICENSE in this directory (this subtree
//  only, because
//  the vendored sys/ framework is GPLv2). The Rust host in src/ remains
//  Apache-2.0 as the overall project.
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

	// Second DDR3 port (ram2) — dedicated to blit_engine_1.
	input         DDRAM2_BUSY,
	output  [7:0] DDRAM2_BURSTCNT,
	output [28:0] DDRAM2_ADDR,
	input  [63:0] DDRAM2_DOUT,
	input         DDRAM2_DOUT_READY,
	output        DDRAM2_RD,
	output [63:0] DDRAM2_DIN,
	output  [7:0] DDRAM2_BE,
	output        DDRAM2_WE,

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

	// Scanout-compositor config (custom ports — the compositor lives in
	// sys_top, but its layer config is decoded here in the LW_H2F regs).
	output [31:0] COMP_WP_BASE,   // wallpaper layer base address
	output        COMP_EN,        // composite enable (content over wallpaper)
	output [31:0] COMP_MASK_BASE, // content coverage mask base address
	output        COMP_MASK_EN,   // content mask enable (tile read-skip)
	output [31:0] COMP_BOXART_BASE,   // boxart overlay FB base address
	output [31:0] COMP_BOXART_POS,    // {y:i16, x:i16}
	output [31:0] COMP_BOXART_SIZE,   // {_, h:12, _, w:12}
	output [31:0] COMP_BOXART_STRIDE, // bytes per boxart row
	output        COMP_BOXART_EN,     // boxart layer enable
	input  [15:0] COMP_UNDERRUN,      // scanout underrun count (clk_100m domain)
	input         SCANOUT_PRESSURE,   // producer read-ahead low (clk_100m domain)

	input         OSD_STATUS
);

////////////////////////////////////////////////////////////////////////////
// Inactive / pass-through defaults for ports we don't drive yet.
////////////////////////////////////////////////////////////////////////////

assign ADC_BUS = 'Z;
assign USER_OUT = '1;
assign {UART_RTS, UART_TXD, UART_DTR} = 0;
assign {SD_SCK, SD_MOSI, SD_CS} = 'Z;

// DDRAM2 driven directly by blit_engine_1 (instantiated below as
// u_blit_engine_1). No arbiter — engine1 is the sole consumer.
assign DDRAM2_ADDR     = blit1_addr;
assign DDRAM2_BURSTCNT = blit1_burstcnt;
assign DDRAM2_RD       = blit1_rd_g;   // via u_blit1_gate (backpressure)
assign DDRAM2_DIN      = blit1_din;
assign DDRAM2_BE       = blit1_be;
assign DDRAM2_WE       = blit1_we_g;   // via u_blit1_gate (backpressure)
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
//   clk_sys (50 MHz)   — blit engines, ring fetcher, regs, arbiter
//   clk_video (100 MHz) — compositor (VGA timing only), also exposed
//                          to the framework as CLK_VIDEO so ASCAL
//                          captures at the native-1080p pixel rate.
// The two are related clocks (same PLL); the one remaining CDC path
// (compositor reset sync) is marked false_path in menu_core.sdc.
////////////////////////////////////////////////////////////////////////////

wire clk_sys;     // 50 MHz: blit engines, ring fetcher, regs, arbiter
wire clk_video;   // 100 MHz: compositor (VGA timing)
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
// Scanout compositor.
//
// The compositor now lives in the framework half (sys/sys_top.v), where it
// replaces ASCAL: it reads the framebuffer from DDR over the vbuf 128-bit
// port and drives the HDMI pixel stream directly at clk_hdmi (148.5 MHz).
// This core half no longer drives any analog/VGA video path — the
// DE10-Nano has no VGA output, and the HDMI tail (shadowmask → OSD →
// HDMI_TX) is fed by the sys_top compositor. VGA_* are tied benign.
//
// Framebuffer geometry still reaches the compositor through the MISTER_FB
// ports (FB_BASE / FB_STRIDE / FB_WIDTH / FB_HEIGHT) driven below; sys_top
// synchronises them into the compositor's clk_100m read domain.
////////////////////////////////////////////////////////////////////////////

assign VGA_R    = 8'd0;
assign VGA_G    = 8'd0;
assign VGA_B    = 8'd0;
assign VGA_HS   = 1'b0;
assign VGA_VS   = 1'b0;
assign VGA_DE   = 1'b0;
assign CE_PIXEL = 1'b1;  // CLK_VIDEO == pixel rate for the framework

// Compositor layer config out to sys_top (wallpaper base + blend enable).
assign COMP_WP_BASE   = reg_wallpaper_addr;
assign COMP_EN        = reg_composite_en;
assign COMP_MASK_BASE = reg_content_mask_addr;
assign COMP_MASK_EN   = reg_content_mask_en;
assign COMP_BOXART_BASE   = reg_boxart_addr;
assign COMP_BOXART_POS    = reg_boxart_pos;
assign COMP_BOXART_SIZE   = reg_boxart_size;
assign COMP_BOXART_STRIDE = reg_boxart_stride;
assign COMP_BOXART_EN     = reg_boxart_en;

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
wire [31:0] reg_wallpaper_addr;
wire        reg_composite_en;
wire [31:0] reg_content_mask_addr;
wire        reg_content_mask_en;
wire [31:0] reg_boxart_addr;
wire [31:0] reg_boxart_pos;
wire [31:0] reg_boxart_size;
wire [31:0] reg_boxart_stride;
wire        reg_boxart_en;
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

// Scanout underrun counter sync (clk_100m -> clk_sys, per-bit 2FF;
// approximate reads acceptable -- diagnostic counter only).
(* preserve *) logic [16:0] comp_underrun_sync0;
logic [16:0] comp_underrun_sync1;
always_ff @(posedge clk_sys) begin
    comp_underrun_sync0 <= {SCANOUT_PRESSURE, COMP_UNDERRUN};
    comp_underrun_sync1 <= comp_underrun_sync0;
end

// Blit backpressure: while the scanout producer is low on read-ahead
// (level signal, hysteretic in the compositor), both blit engines
// stop issuing NEW DDR commands; in-flight write bursts complete and
// read responses land. Gating goes through blit_pressure_gate — NOT
// a bare OR into the engines' busy inputs, which desynced the
// command/response accounting and wedged the fetcher (see the gate's
// header). The ring FETCHER stays ungated so command fetch keeps
// work queued for when pressure lifts. This is what makes scanout
// underruns structurally impossible under total DDR oversubscription
// — MPFE priority alone cannot guarantee the producer's throughput
// once the blitters saturate the device.
wire scanout_pressure_s = comp_underrun_sync1[16];

// CONTROL.CE (clear-error pulse) re-arms the fetcher: it leaves S_HALT
// and clears the error bit. We OR this into the fetcher's reset —
// and ONLY the fetcher's. It used to reset fb_swapper, both blit
// engines, and the DDRAM arbiter too; a CE while a read burst was
// in flight zeroed the arbiter's outstanding-beat counter, the
// stragglers then underflowed it to 0xFFFF and the bus wedged
// permanently (and the display/render/ready rotation snapped to
// reset, visibly glitching the screen on every error recovery).
wire fetcher_rst_n = (~RESET) & ~reg_clear_error;
wire engine_rst_n  = ~RESET;

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

    .wallpaper_addr_o   (reg_wallpaper_addr),
    .composite_en_o     (reg_composite_en),
    .content_mask_addr_o (reg_content_mask_addr),
    .content_mask_en_o   (reg_content_mask_en),
    .boxart_addr_o       (reg_boxart_addr),
    .boxart_pos_o        (reg_boxart_pos),
    .boxart_size_o       (reg_boxart_size),
    .boxart_stride_o     (reg_boxart_stride),
    .boxart_en_o         (reg_boxart_en),

    // LAYER_DEBUG[15:0] = compositor scanout underrun counter,
    // LAYER_DEBUG[16] = live scanout-pressure (blit backpressure)
    // level (see menu_core_regs header). 2FF per bit from clk_100m;
    // reads racing an increment may be off-by-one, which is fine for
    // diagnostics.
    .layer_descriptors_i ({15'd0, comp_underrun_sync1}),

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

// Blit dispatch from fetcher — dual engines, ring fetcher gates
// start signals on its active_engine toggle.
wire        blit0_start;
wire        blit1_start;
wire [1:0]  blit_mode;
wire [1:0]  blit_blend;
wire [15:0] blit_dst_x, blit_dst_y, blit_dst_w, blit_dst_h;
wire [15:0] blit_src_x, blit_src_y, blit_src_w, blit_src_h;
wire [31:0] blit_src_addr, blit_src_pitch;
wire [31:0] blit_color;
wire        blit_format;
wire        blit_tint_en;
wire [31:0] blit_tint_color;
wire [31:0] blit_aff_m00, blit_aff_m01, blit_aff_m10, blit_aff_m11;
wire [31:0] blit_aff_tx,  blit_aff_ty;
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

    .blit0_start_o   (blit0_start),
    .blit1_start_o   (blit1_start),
    .blit0_done_i    (blit_done),     // engine 0 (u_blit_engine on ram1)
    .blit1_done_i    (blit1_done),    // engine 1 (u_blit_engine_1 on ram2)
    .blit_mode_o     (blit_mode),
    .blit_blend_o    (blit_blend),
    .blit_dst_x_o    (blit_dst_x),
    .blit_dst_y_o    (blit_dst_y),
    .blit_dst_w_o    (blit_dst_w),
    .blit_dst_h_o    (blit_dst_h),
    .blit_color_o    (blit_color),
    .blit_src_x_o    (blit_src_x),
    .blit_src_y_o    (blit_src_y),
    .blit_src_w_o    (blit_src_w),
    .blit_src_h_o    (blit_src_h),
    .blit_src_addr_o (blit_src_addr),
    .blit_src_pitch_o(blit_src_pitch),
    .blit_format_o     (blit_format),
    .blit_tint_en_o    (blit_tint_en),
    .blit_tint_color_o (blit_tint_color),
    .blit_aff_m00_o (blit_aff_m00),
    .blit_aff_m01_o (blit_aff_m01),
    .blit_aff_m10_o (blit_aff_m10),
    .blit_aff_m11_o (blit_aff_m11),
    .blit_aff_tx_o  (blit_aff_tx),
    .blit_aff_ty_o  (blit_aff_ty),
    .blit_clip_en_o    (blit_clip_en),
    .blit_clip_x_o     (blit_clip_x),
    .blit_clip_y_o     (blit_clip_y),
    .blit_clip_w_o     (blit_clip_w),
    .blit_clip_h_o     (blit_clip_h),
    .blit_ignore_clip_o(blit_ignore_clip),

    .ddram_addr_o       (fetch_addr),
    .ddram_burstcnt_o   (fetch_burstcnt),
    .ddram_be_o         (fetch_be),
    .ddram_rd_o         (fetch_rd),
    .ddram_busy_i       (DDRAM_BUSY),
    .ddram_dout_i       (DDRAM_DOUT),
    .ddram_dout_valid_i (fetch_dout_valid)
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
logic fb_vbl_d0, fb_vbl_d1, fb_vbl_d2;
always_ff @(posedge clk_sys) begin
    fb_vbl_d0 <= FB_VBL;
    fb_vbl_d1 <= fb_vbl_d0;
    fb_vbl_d2 <= fb_vbl_d1;
end
// Edge-detect on the SECOND and THIRD flops: fb_vbl_d0 is the
// metastability-capture stage and must not feed logic — using it
// directly could double-pulse or drop a vsync (double-rotate or
// missed swap = one-frame glitch).
wire vsync_pulse = fb_vbl_d1 & ~fb_vbl_d2;

fb_swapper u_fb_swapper (
    .clk             (clk_sys),
    .rst_n           (engine_rst_n),

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
    .rst_n      (engine_rst_n),

    .start_i    (blit0_start),
    .mode_i     (blit_mode),
    .blend_i    (blit_blend),
    .dst_x_i    (blit_dst_x),
    .dst_y_i    (blit_dst_y),
    .dst_w_i    (blit_dst_w),
    .dst_h_i    (blit_dst_h),
    .color_i    (blit_color),
    .src_x_i    (blit_src_x),
    .src_y_i    (blit_src_y),
    .src_w_i    (blit_src_w),
    .src_h_i    (blit_src_h),
    .src_addr_i (blit_src_addr),
    .src_pitch_i(blit_src_pitch),
    .format_i      (blit_format),
    .tint_en_i     (blit_tint_en),
    .tint_color_i  (blit_tint_color),
    .aff_m00_i  (blit_aff_m00),
    .aff_m01_i  (blit_aff_m01),
    .aff_m10_i  (blit_aff_m10),
    .aff_m11_i  (blit_aff_m11),
    .aff_tx_i   (blit_aff_tx),
    .aff_ty_i   (blit_aff_ty),

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
    .ddram_busy_i       (blit0_busy_gated),
    .ddram_dout_i       (DDRAM_DOUT),
    .ddram_dout_valid_i (blit_dout_valid)
);

// Scanout-backpressure gate for engine0 (sits between the engine and
// the ram1 arbiter mux below; the fetcher shares ram1 but is exempt
// by design — it parks in S_BLIT_WAIT during blits anyway).
wire blit0_busy_gated, blit_rd_g, blit_we_g;
blit_pressure_gate u_blit0_gate (
    .clk            (clk_sys),
    .rst_n          (engine_rst_n),
    .pressure_i     (scanout_pressure_s),
    .eng_rd_i       (blit_rd),
    .eng_we_i       (blit_we),
    .eng_burstcnt_i (blit_burstcnt),
    .eng_busy_o     (blit0_busy_gated),
    .port_rd_o      (blit_rd_g),
    .port_we_o      (blit_we_g),
    .port_busy_i    (DDRAM_BUSY)
);

////////////////////////////////////////////////////////////////////////////
// Second blit engine — drives DDRAM2 (ram2, dedicated port, no arbiter).
//
// Shares all parameter inputs with u_blit_engine — the ring_fetcher
// drives the same parameter bus, and a future step will route
// blit_start_o to engine0 or engine1 based on an active_engine
// toggle that flips on each PRESENT.
//
// Step 2: start_i tied to 0 (engine permanently idle). Just verifies
// that the second engine synthesizes + fits without affecting
// existing single-blit behavior.
////////////////////////////////////////////////////////////////////////////

wire [28:0] blit1_addr;
wire [7:0]  blit1_burstcnt;
wire [7:0]  blit1_be;
wire [63:0] blit1_din;
wire        blit1_we;
wire        blit1_rd;
wire        blit1_busy;
wire        blit1_done;

blit_engine u_blit_engine_1 (
    .clk        (clk_sys),
    .rst_n      (engine_rst_n),

    .start_i    (blit1_start),           // Step 3: gated by active_engine in fetcher.
    .mode_i     (blit_mode),
    .blend_i    (blit_blend),
    .dst_x_i    (blit_dst_x),
    .dst_y_i    (blit_dst_y),
    .dst_w_i    (blit_dst_w),
    .dst_h_i    (blit_dst_h),
    .color_i    (blit_color),
    .src_x_i    (blit_src_x),
    .src_y_i    (blit_src_y),
    .src_w_i    (blit_src_w),
    .src_h_i    (blit_src_h),
    .src_addr_i (blit_src_addr),
    .src_pitch_i(blit_src_pitch),
    .format_i      (blit_format),
    .tint_en_i     (blit_tint_en),
    .tint_color_i  (blit_tint_color),
    .aff_m00_i  (blit_aff_m00),
    .aff_m01_i  (blit_aff_m01),
    .aff_m10_i  (blit_aff_m10),
    .aff_m11_i  (blit_aff_m11),
    .aff_tx_i   (blit_aff_tx),
    .aff_ty_i   (blit_aff_ty),

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

    .busy_o     (blit1_busy),
    .done_o     (blit1_done),

    // Dedicated DDR3 port (ram2). No arbiter — engine1 is the sole
    // consumer of DDRAM2.
    .ddram_addr_o       (blit1_addr),
    .ddram_burstcnt_o   (blit1_burstcnt),
    .ddram_be_o         (blit1_be),
    .ddram_din_o        (blit1_din),
    .ddram_we_o         (blit1_we),
    .ddram_rd_o         (blit1_rd),
    .ddram_busy_i       (blit1_busy_gated),
    .ddram_dout_i       (DDRAM2_DOUT),
    .ddram_dout_valid_i (DDRAM2_DOUT_READY)
);

// Scanout-backpressure gate for engine1 (sole consumer of ram2).
wire blit1_busy_gated, blit1_rd_g, blit1_we_g;
blit_pressure_gate u_blit1_gate (
    .clk            (clk_sys),
    .rst_n          (engine_rst_n),
    .pressure_i     (scanout_pressure_s),
    .eng_rd_i       (blit1_rd),
    .eng_we_i       (blit1_we),
    .eng_burstcnt_i (blit1_burstcnt),
    .eng_busy_o     (blit1_busy_gated),
    .port_rd_o      (blit1_rd_g),
    .port_we_o      (blit1_we_g),
    .port_busy_i    (DDRAM2_BUSY)
);

// Layer registers are not consumed in this core half yet — the multi-layer
// compositor reads the LAYER_TABLE path directly in sys_top in a later
// phase. reg_tex_table_addr IS used by the ring fetcher for COPY_RECT
// descriptors, so it is not listed here.
wire _unused_layer_regs = &{1'b0, reg_layer_count, reg_layer_table_base,
                            reg_layer_active, 1'b0};

// DDRAM_* arbiter. Two masters (blit engine 0 + ring fetcher) share one
// read response bus (DDRAM_DOUT/DDRAM_DOUT_READY); the HPS-to-FPGA bridge
// doesn't tag responses with the requester. The earlier "samples-only-
// while-owner" scheme broke whenever ownership transferred while reads
// were still draining — the new owner captured the previous owner's
// beats, causing fetcher BadOpcode halts.
//
// Fix: serialise. owner_q latches at the cycle a new read is accepted
// while the response pipe is idle. outstanding_beats_q tracks in-flight
// beats (incr on accept by burstcnt, decr on each DDRAM_DOUT_READY). New
// owners can't be granted while beats are draining for the previous
// owner. Per-consumer dout_valid is masked by owner_q so cross-talk is
// impossible. Priority: blit > fetcher.
localparam logic [1:0] TAG_FETCH = 2'd0;
localparam logic [1:0] TAG_BLIT  = 2'd1;

logic [1:0]  owner_q;
logic [15:0] outstanding_beats_q;
wire         pipe_idle     = (outstanding_beats_q == 16'd0);
wire         read_accepted = DDRAM_RD & ~DDRAM_BUSY;
wire         beat_arrived  = DDRAM_DOUT_READY;

wire [1:0] next_owner = blit_busy ? TAG_BLIT : TAG_FETCH;

// Grant the bus only when the pipe is idle (any requester wins) OR the
// requester matches the current owner (drains its own burst). This is
// what serialises across ownership transfers.
wire owner_grant_ok = pipe_idle | (next_owner == owner_q);

wire blit_owns_bus  = blit_busy  & owner_grant_ok;
wire fetch_owns_bus = ~blit_busy & owner_grant_ok;

always_ff @(posedge clk_sys or negedge engine_rst_n) begin
    if (!engine_rst_n) begin
        owner_q             <= TAG_FETCH;
        outstanding_beats_q <= 16'd0;
    end else begin
        case ({read_accepted, beat_arrived})
            2'b10:   outstanding_beats_q <= outstanding_beats_q + {8'd0, DDRAM_BURSTCNT};
            2'b01:   outstanding_beats_q <= outstanding_beats_q - 16'd1;
            2'b11:   outstanding_beats_q <= outstanding_beats_q + {8'd0, DDRAM_BURSTCNT} - 16'd1;
            default: outstanding_beats_q <= outstanding_beats_q;
        endcase
        if (read_accepted & pipe_idle) owner_q <= next_owner;
    end
end

// Per-consumer dout_valid: a beat is delivered only to the current
// owner. The other consumer sees a constant 0, so it cannot capture
// foreign data even if its own FSM happens to be in a wait state.
wire fetch_dout_valid = DDRAM_DOUT_READY & (owner_q == TAG_FETCH);
wire blit_dout_valid  = DDRAM_DOUT_READY & (owner_q == TAG_BLIT);

assign DDRAM_ADDR     = blit_owns_bus  ? blit_addr
                      : fetch_owns_bus ? fetch_addr
                      : 29'd0;
assign DDRAM_BURSTCNT = blit_owns_bus  ? blit_burstcnt
                      : fetch_owns_bus ? fetch_burstcnt
                      : 8'd0;
assign DDRAM_BE       = blit_owns_bus  ? blit_be
                      : fetch_owns_bus ? fetch_be
                      : 8'd0;
assign DDRAM_DIN      = blit_owns_bus ? blit_din : 64'd0;
assign DDRAM_RD       = blit_owns_bus  ? blit_rd_g
                      : fetch_owns_bus ? fetch_rd
                      : 1'b0;
assign DDRAM_WE       = blit_owns_bus ? blit_we_g : 1'b0;

// reg_ring_kick is currently advisory — the fetcher polls RING_TAIL
// every cycle anyway. Wire-suppress to avoid unused warnings until
// M2c+ lets it gate a low-power idle.
wire _unused_kick = reg_ring_kick;

// blit_engine_1's busy_o still unused — only done_o is consumed by
// the ring fetcher (via active_engine_q muxing in S_BLIT_WAIT).
wire _unused_blit1 = &{1'b0, blit1_busy, 1'b0};

////////////////////////////////////////////////////////////////////////////
// MISTER_FB configuration. FB_FORMAT selects BGR 32bpp so the framework
// reads our BGRA8888 framebuffer directly (PROTOCOL.md §2.1, §7.1).
// All other geometry comes from the host via control registers
// (FB0_ADDR / FB_WIDTH / FB_HEIGHT / FB_STRIDE) so the menu can render
// pixel-perfect for the active HDMI mode (read from VIDEO_INFO).
////////////////////////////////////////////////////////////////////////////

`ifdef MISTER_FB
// MISTER_FB scanout active: the framework reads BGRA8888 pixels from
// DDR3 at FB_BASE and drives them out via ASCAL → HDMI. The host
// writes pixels into FB0/FB1/FB2 (one of three) and bumps the
// display index via PRESENT; `scanout_fb_base` picks whichever slot
// the fb_swapper says is currently the display.
//
// FB_FORMAT = 5'b10110 = BGR, 32bpp (matches our BGRA8888 buffers).
//
// FB_EN=0: the framework's MISTER_FB hardware scanout is disabled — our
// own compositor in sys_top owns the vbuf port and drives HDMI. We still
// publish the framebuffer geometry (BASE/WIDTH/HEIGHT/STRIDE) because the
// sys_top compositor reads FB_BASE/FB_STRIDE for its scanout, and the
// framework latches them into its FB_* regs regardless of FB_EN.
assign FB_EN          = 1'b0;
assign FB_FORMAT      = 5'b10110;
assign FB_WIDTH       = reg_fb_width;
assign FB_HEIGHT      = reg_fb_height;
assign FB_BASE        = scanout_fb_base;
assign FB_STRIDE      = reg_fb_stride;
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
