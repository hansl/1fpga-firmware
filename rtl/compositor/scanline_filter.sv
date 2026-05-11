//============================================================================
//
//  Scanline active-list builder (Phase 2a step 3).
//
//  Once per scanline (kicked by `start_i` from the compositor's HBlank
//  entry edge), walks all `layer_count_i` slots of the layer cache,
//  picks the ones that cover `y_next_i`, and emits them as a compact
//  array of "active" entries — the painter then consumes this array
//  in parallel and outputs the right pixel for each x in the next
//  scanline.
//
//  Walk pipeline (1-cycle BRAM read latency):
//    cycle N:    cache_slot_o = K (issue read for slot K).
//    cycle N+1:  cache_data_i = layer[K], hit-test, conditionally
//                write to active list.
//
//  Total cycles per pass: layer_count + ~2 (one for IDLE→ISSUE, one
//  for DRAIN). 256 layers → ~258 cycles. HBlank window is 280 cycles
//  in the compositor's min-blanking 1280x720@43 timing — i.e. ~8%
//  margin above the worst-case count=256 walk. If you shrink HBlank
//  below 258, full layer-count frames will clip.
//
//  Phase 2b step 1: textured layers (tex_id != 0xFFFF) are kept in
//  the active list. Their tex_id field rides along so the painter
//  (and, in step 2, the texture_unit) can route them through the
//  texel-sampler path instead of using the solid `color` field.
//
//  Output capacity: parametrised by MAX_ACTIVE. Hits beyond the cap
//  are silently dropped. Default 16 is enough for a typical UI scene
//  (background + a few panels + a focus indicator) without straining
//  the per-pixel comparator tree.
//
//============================================================================

module scanline_filter #(
    // Per-scanline active-list capacity. Hard-coded internal counter
    // widths (5 bits) cap this at 31. If you bump this past 31, also
    // widen `active_idx_q`, `active_count_q`, and `active_count_o`.
    parameter int MAX_ACTIVE = 16
) (
    input  logic        clk,
    input  logic        rst_n,

    // 1-cycle pulse: begin walking. Ignored if already busy.
    input  logic        start_i,
    input  logic [11:0] y_next_i,
    input  logic [8:0]  layer_count_i,

    // Layer-cache read port (shared with renderer? no — only this
    // module drives it).
    output logic [7:0]   cache_slot_o,
    input  logic [255:0] cache_data_i,

    // Active list. `active_count_o` is the number of populated
    // entries (0..MAX_ACTIVE). The painter walks i in 0..count-1
    // and treats the latest match as "in front" (PROTOCOL.md §11.1:
    // slot index = z-order).
    output logic [4:0]              active_count_o,
    output logic signed [16:0]      active_dst_x_lo_o [MAX_ACTIVE-1:0],
    output logic signed [17:0]      active_dst_x_hi_o [MAX_ACTIVE-1:0],
    output logic [31:0]             active_color_o    [MAX_ACTIVE-1:0],
    // tex_id rides along with each active entry. 0xFFFF means
    // solid-colour (use `color` field); anything else means the
    // painter / texture_unit will sample texels for this rect.
    output logic [15:0]             active_tex_id_o   [MAX_ACTIVE-1:0]
);

    typedef enum logic [1:0] {
        S_IDLE,
        S_ISSUE,
        S_DRAIN
    } state_t;

    state_t      state_q;
    logic [8:0]  issue_q;          // next slot to issue
    logic [8:0]  count_latched;
    logic [11:0] y_next_latched;
    logic        data_valid_q;     // cache_data_i carries a slot result this cycle
    logic [4:0]  active_idx_q;
    logic [4:0]  active_count_q;

    // Decode of cache_data_i (mirrors PROTOCOL.md §7.1 / §11 layout).
    wire [15:0]        flags  = cache_data_i[15:0];
    wire [15:0]        tex_id = cache_data_i[31:16];
    wire signed [15:0] dst_x  = cache_data_i[47:32];
    wire signed [15:0] dst_y  = cache_data_i[63:48];
    wire [15:0]        dst_w  = cache_data_i[79:64];
    wire [15:0]        dst_h  = cache_data_i[95:80];
    wire [31:0]        color  = cache_data_i[191:160];

    wire enabled = flags[0];

    // Y-in-range test. dst_y is i16 (layer can extend off-screen top);
    // dst_h is u16. y_next_i is u12. Compare in 17-bit signed.
    wire signed [16:0] dst_y_lo_s = {dst_y[15], dst_y};
    wire signed [16:0] dst_y_hi_s = dst_y_lo_s + $signed({1'b0, dst_h});
    wire signed [16:0] y_next_s   = $signed({5'b0, y_next_latched});
    wire y_in_range = (y_next_s >= dst_y_lo_s) && (y_next_s < dst_y_hi_s);

    // Phase 2b: accept both solid (tex_id == 0xFFFF) and textured
    // (tex_id < 0xFFFF) layers. The painter / texture_unit downstream
    // routes them differently based on the recorded tex_id.
    wire hit = data_valid_q && enabled && y_in_range;

    // dst_x extends to 17-bit signed; dst_x + dst_w to 18-bit (worst
    // case dst_x = -32768, dst_w = 65535 → 32767 which fits 17-bit
    // signed, but be safe with 18).
    wire signed [16:0] hit_dst_x_lo = {dst_x[15], dst_x};
    wire signed [17:0] hit_dst_x_hi = $signed({dst_x[15], dst_x[15], dst_x})
                                    + $signed({2'b00, dst_w});

    assign cache_slot_o   = issue_q[7:0];
    assign active_count_o = active_count_q;

    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state_q          <= S_IDLE;
            issue_q          <= 9'd0;
            count_latched    <= 9'd0;
            y_next_latched   <= 12'd0;
            data_valid_q     <= 1'b0;
            active_idx_q     <= 5'd0;
            active_count_q   <= 5'd0;
            for (int i = 0; i < MAX_ACTIVE; i++) begin
                active_dst_x_lo_o[i] <= 17'd0;
                active_dst_x_hi_o[i] <= 18'd0;
                active_color_o[i]    <= 32'd0;
                active_tex_id_o[i]   <= 16'd0;
            end
        end else begin
            // 1-cycle data pipeline: data_valid_q true iff the *previous*
            // cycle issued a read for a slot within range.
            data_valid_q <= (state_q == S_ISSUE) && (issue_q < count_latched);

            unique case (state_q)
                S_IDLE: begin
                    if (start_i) begin
                        issue_q          <= 9'd0;
                        count_latched    <= layer_count_i;
                        y_next_latched   <= y_next_i;
                        active_idx_q     <= 5'd0;
                        active_count_q   <= 5'd0;
                        state_q          <= S_ISSUE;
                    end
                end

                S_ISSUE: begin
                    if (issue_q < count_latched) begin
                        issue_q <= issue_q + 9'd1;
                    end else begin
                        state_q <= S_DRAIN;
                    end
                end

                S_DRAIN: begin
                    // The last useful data landed (was processed) on
                    // the cycle the state transitioned from S_ISSUE
                    // to here. Just finalise the count and idle.
                    active_count_q <= active_idx_q;
                    state_q        <= S_IDLE;
                end

                default: state_q <= S_IDLE;
            endcase

            // Active-list write — independent of state, gated by hit.
            if (hit && (active_idx_q < MAX_ACTIVE[4:0])) begin
                active_dst_x_lo_o[active_idx_q] <= hit_dst_x_lo;
                active_dst_x_hi_o[active_idx_q] <= hit_dst_x_hi;
                active_color_o[active_idx_q]    <= color;
                active_tex_id_o[active_idx_q]   <= tex_id;
                active_idx_q                    <= active_idx_q + 5'd1;
            end
        end
    end

endmodule
