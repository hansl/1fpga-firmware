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

// Analog-side video: driven to zero because MISTER_FB supplies HDMI pixels.
assign VGA_R        = '0;
assign VGA_G        = '0;
assign VGA_B        = '0;
assign VGA_HS       = 1'b0;
assign VGA_VS       = 1'b0;
assign VGA_DE       = 1'b0;
assign VGA_F1       = 1'b0;
assign VGA_SL       = 2'b00;
assign VGA_SCALER   = 1'b0;
assign VGA_DISABLE  = 1'b1;  // disable analog output entirely
assign VIDEO_ARX    = 13'd0;
assign VIDEO_ARY    = 13'd0;
assign CE_PIXEL     = 1'b0;
// HDMI_FREEZE pulsing at frame rate seems to alias against ASCAL's
// scaler phase, producing a per-frame sub-pixel shift of the
// just-blitted region. Leave it tied off while we investigate; M2c4
// (triple-buffer swap) avoids the underlying issue by writing to a
// non-displayed FB.
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
// inclk[3] (synthesis error 15836 if driven by a raw input pin). We
// therefore instantiate a 50→50 MHz pass-through PLL here. When the blit
// engine and command fetcher land, retune the PLL parameters (or add
// additional outputs) for the blit and pixel clocks; see rtl/pll/pll.v.
////////////////////////////////////////////////////////////////////////////

wire clk_sys;
wire pll_locked;

pll pll_inst (
	.refclk   (CLK_50M),
	.rst      (1'b0),
	.outclk_0 (clk_sys),
	.locked   (pll_locked)
);

assign CLK_VIDEO = clk_sys;

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
                    HDMI_WIDTH, HDMI_HEIGHT, FB_VBL, FB_LL, OSD_STATUS,
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
wire [31:0] fetcher_ring_head;
wire [31:0] fetcher_fence_value;
wire [31:0] fetcher_frame_count;
wire [31:0] fetcher_error_info;
wire        fetcher_status_busy;
wire        fetcher_status_error;

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

    .ring_head_i    (fetcher_ring_head),
    .fence_value_i  (fetcher_fence_value),
    .frame_count_i  (fetcher_frame_count),
    .error_info_i   (fetcher_error_info),
    .status_busy_i  (fetcher_status_busy),
    .status_error_i (fetcher_status_error)
);

// Fetcher's read-side DDRAM signals.
wire [28:0] fetch_addr;
wire [7:0]  fetch_burstcnt;
wire [7:0]  fetch_be;
wire        fetch_rd;

// Blit engine's write-side DDRAM signals.
wire [28:0] blit_addr;
wire [7:0]  blit_burstcnt;
wire [7:0]  blit_be;
wire [63:0] blit_din;
wire        blit_we;
wire        blit_busy;

// Blit dispatch from fetcher.
wire        blit_start;
wire [15:0] blit_dst_x, blit_dst_y, blit_dst_w, blit_dst_h;
wire [31:0] blit_color;
wire        blit_done;

ring_fetcher u_ring_fetcher (
    .clk             (clk_sys),
    .rst_n           (fetcher_rst_n),

    .enable_i        (reg_enable),
    .ring_base_i     (reg_ring_base),
    .ring_size_i     (reg_ring_size),
    .ring_tail_i     (reg_ring_tail),
    .ring_head_o     (fetcher_ring_head),
    .fence_value_o   (fetcher_fence_value),
    .frame_count_o   (fetcher_frame_count),
    .error_info_o    (fetcher_error_info),
    .status_busy_o   (fetcher_status_busy),
    .status_error_o  (fetcher_status_error),

    .blit_start_o    (blit_start),
    .blit_dst_x_o    (blit_dst_x),
    .blit_dst_y_o    (blit_dst_y),
    .blit_dst_w_o    (blit_dst_w),
    .blit_dst_h_o    (blit_dst_h),
    .blit_color_o    (blit_color),
    .blit_done_i     (blit_done),

    .ddram_addr_o       (fetch_addr),
    .ddram_burstcnt_o   (fetch_burstcnt),
    .ddram_be_o         (fetch_be),
    .ddram_rd_o         (fetch_rd),
    .ddram_busy_i       (DDRAM_BUSY),
    .ddram_dout_i       (DDRAM_DOUT),
    .ddram_dout_valid_i (DDRAM_DOUT_READY)
);

blit_engine u_blit_engine (
    .clk        (clk_sys),
    .rst_n      (fetcher_rst_n),

    .start_i    (blit_start),
    .dst_x_i    (blit_dst_x),
    .dst_y_i    (blit_dst_y),
    .dst_w_i    (blit_dst_w),
    .dst_h_i    (blit_dst_h),
    .color_i    (blit_color),

    .fb_base_i  (32'h3000_0000),     // matches FB_BASE assignment below
    .fb_stride_i(14'd7680),

    .busy_o     (blit_busy),
    .done_o     (blit_done),

    .ddram_addr_o     (blit_addr),
    .ddram_burstcnt_o (blit_burstcnt),
    .ddram_be_o       (blit_be),
    .ddram_din_o      (blit_din),
    .ddram_we_o       (blit_we),
    .ddram_busy_i     (DDRAM_BUSY)
);

// DDRAM_* mux: blit engine owns the bus while it's busy (writes only),
// fetcher otherwise (reads only). Read-data flows back to the fetcher
// regardless — only the request side is muxed.
assign DDRAM_ADDR     = blit_busy ? blit_addr     : fetch_addr;
assign DDRAM_BURSTCNT = blit_busy ? blit_burstcnt : fetch_burstcnt;
assign DDRAM_BE       = blit_busy ? blit_be       : fetch_be;
assign DDRAM_DIN      = blit_busy ? blit_din      : 64'd0;
assign DDRAM_RD       = blit_busy ? 1'b0          : fetch_rd;
assign DDRAM_WE       = blit_busy ? blit_we       : 1'b0;

// reg_ring_kick is currently advisory — the fetcher polls RING_TAIL
// every cycle anyway. Wire-suppress to avoid unused warnings until
// M2c+ lets it gate a low-power idle.
wire _unused_kick = reg_ring_kick;

////////////////////////////////////////////////////////////////////////////
// MISTER_FB configuration — this is the whole of the menu core for M1.
//
// FB_FORMAT:
//   [2:0] = 3'b110 → 32 bpp
//   [3]   = 1'b0   → irrelevant at 32bpp (selects 565 vs 1555 for 16bpp)
//   [4]   = 1'b1   → BGR channel order (for 32bpp this matches Linux
//                     BGRA8888 byte order B,G,R,A in ascending addresses,
//                     which is PROTOCOL.md §7.1)
//
// FB_BASE points at framebuffer slot 0 (PROTOCOL.md §2.1). Future
// revisions will toggle this between 0x30000000 / 0x30800000 / 0x31000000
// at vsync under command-ring control.
//
// FB_STRIDE is 7680 = 1920 * 4 bytes.
////////////////////////////////////////////////////////////////////////////

`ifdef MISTER_FB
assign FB_EN          = 1'b1;
assign FB_FORMAT      = 5'b10110;        // BGR, 32bpp
assign FB_WIDTH       = 12'd1920;
assign FB_HEIGHT      = 12'd1080;
assign FB_BASE        = 32'h3000_0000;   // PROTOCOL.md §2 framebuffer 0
assign FB_STRIDE      = 14'd7680;        // 1920 * 4
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

assign LED_USER = reg_enable ? fast_blink : breathe;

endmodule
