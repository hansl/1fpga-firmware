// Scanout-backpressure gate for a blit engine's DDR command stream.
//
// Naive gating — ORing the pressure level into the ENGINE's busy
// input — is an Avalon protocol violation that wedges the whole
// pipeline: the port still sees the engine's asserted command while
// the engine believes it is stalled, so the port ACCEPTS the command;
// the engine, still seeing "busy", keeps it asserted and the port
// accepts a DUPLICATE. The response-beat accounting desyncs and the
// ring fetcher hangs in S_BLIT_WAIT forever. (Measured on hardware:
// fetcher STATUS stuck at BZ with no error, FENCE_VALUE never
// written, wallpaper-only screen, host fence timeout at boot — the
// compositor enables mid-frame, so a legitimate pressure pulse fired
// during the very first fill.)
//
// The correct gate keeps the two sides consistent:
//   - the PORT sees rd/we only when not gating; the remaining beats
//     of an already-accepted write burst always flow (a burst must
//     complete once started);
//   - the ENGINE sees busy while gated, so it legally holds its
//     command until the gate opens;
//   - response beats of already-issued reads pass untouched.
//
// New commands therefore stop at the gate while the scanout ring
// refills, and nothing is ever double-accepted.

module blit_pressure_gate (
    input  logic       clk,
    input  logic       rst_n,
    /// Scanout pressure level (already synchronised to `clk`).
    input  logic       pressure_i,

    // ---- Engine side --------------------------------------------
    input  logic       eng_rd_i,
    input  logic       eng_we_i,
    input  logic [7:0] eng_burstcnt_i,
    output logic       eng_busy_o,

    // ---- Port side ----------------------------------------------
    output logic       port_rd_o,
    output logic       port_we_o,
    input  logic       port_busy_i
);

    // Remaining beats of an in-flight write burst (0 = none). The
    // first accepted beat of an N-beat burst loads N-1.
    logic [7:0] wr_beats_left_q;
    wire in_wr_burst = (wr_beats_left_q != 8'd0);
    wire gate_new    = pressure_i & ~in_wr_burst;

    assign port_rd_o  = eng_rd_i & ~gate_new;
    assign port_we_o  = eng_we_i & (in_wr_burst | ~gate_new);
    assign eng_busy_o = port_busy_i | gate_new;

    wire wr_accept = port_we_o & ~port_busy_i;
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            wr_beats_left_q <= 8'd0;
        end else if (wr_accept) begin
            wr_beats_left_q <= in_wr_burst
                                   ? wr_beats_left_q - 8'd1
                                   : eng_burstcnt_i - 8'd1;
        end
    end

endmodule
