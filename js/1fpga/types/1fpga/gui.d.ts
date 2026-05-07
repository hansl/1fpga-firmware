declare module '1fpga:gui' {
  /**
   * Stable identifier for a node in the host tree. Returned by
   * `createInstance`, accepted by every other host call. The numeric
   * value is opaque — do not synthesize ids manually.
   */
  export type NodeId = number;

  /** A length value, in pixels. (No `%` / `em` / `rem` yet.) */
  export type Length = number;

  /** Style fields recognised by the runtime. Every field is optional;
   *  unset fields fall back to the layout engine's CSS defaults.
   *  Lengths are pixels. Colors are CSS color strings: `#rgb`,
   *  `#rrggbb`, `#rrggbbaa`. */
  export interface Style {
    // ---- Layout ------------------------------------------------------
    display?: 'block' | 'flex';
    position?: 'relative' | 'absolute';
    flexDirection?: 'row' | 'column' | 'row-reverse' | 'column-reverse';
    flexWrap?: 'nowrap' | 'wrap' | 'wrap-reverse';
    justifyContent?:
      | 'flex-start' | 'flex-end' | 'center'
      | 'space-between' | 'space-around' | 'space-evenly';
    alignItems?: 'stretch' | 'flex-start' | 'flex-end' | 'center' | 'baseline';
    alignSelf?: 'stretch' | 'flex-start' | 'flex-end' | 'center' | 'baseline';
    flexGrow?: number;
    flexShrink?: number;
    flexBasis?: Length;
    gap?: Length;

    // ---- Position offsets (mostly meaningful for position:absolute) -
    top?: Length;
    right?: Length;
    bottom?: Length;
    left?: Length;

    // ---- Box ---------------------------------------------------------
    width?: Length;
    height?: Length;
    minWidth?: Length;
    maxWidth?: Length;
    minHeight?: Length;
    maxHeight?: Length;
    padding?: Length;
    paddingTop?: Length;
    paddingRight?: Length;
    paddingBottom?: Length;
    paddingLeft?: Length;
    margin?: Length;
    marginTop?: Length;
    marginRight?: Length;
    marginBottom?: Length;
    marginLeft?: Length;

    // ---- Visual ------------------------------------------------------
    backgroundColor?: string;
    opacity?: number;
    overflow?: 'visible' | 'hidden';

    // ---- Text --------------------------------------------------------
    color?: string;
    fontFamily?: string;
    fontSize?: number;
  }

  /** Props bag accepted by `createInstance` / `commitUpdate`. Mirrors
   *  React's instance props shape. */
  export interface Props {
    style?: Style;
    className?: string;
    children?: unknown;
    /** Filesystem path for `<img>` instances (PNG, decoded once). */
    src?: string;
  }

  /**
   * Allocate a new host node. `type` selects the renderer; the
   * runtime supports `'div'` (containers) and `'img'` (image leaves;
   * pass the file path in `props.src`). Text content is created via
   * `createTextInstance` (handled by react-reconciler when it
   * encounters string children).
   */
  export function createInstance(type: 'div' | 'img', props?: Props): NodeId;

  /**
   * Allocate a new text leaf node. The text content uses the parent
   * `<div>`'s resolved `color` / `fontFamily` / `fontSize` style.
   */
  export function createTextInstance(text: string): NodeId;

  /** Replace the text content of an existing text node. */
  export function commitTextUpdate(node: NodeId, text: string): void;

  /** Append `child` to the end of `parent`'s children list. */
  export function appendChild(parent: NodeId, child: NodeId): void;

  /**
   * Insert `child` into `parent`'s children list immediately before
   * `before`. If `before` is no longer a child of `parent`, falls back
   * to appending.
   */
  export function insertBefore(parent: NodeId, child: NodeId, before: NodeId): void;

  /**
   * Detach `child` from `parent` and recursively free the subtree.
   * Existing NodeIds for the removed subtree become invalid.
   */
  export function removeChild(parent: NodeId, child: NodeId): void;

  /**
   * Replace the props (currently style only) on an existing node.
   * Used by react-reconciler on every commit.
   */
  export function commitUpdate(node: NodeId, props: Props): void;

  /** Replace `node`'s style. Equivalent to `commitUpdate(node, { style })`. */
  export function setStyle(node: NodeId, style: Style): void;

  /**
   * Merge a partial style onto the node's existing style. Unlike
   * `setStyle` (which replaces the whole style), `updateStyle` only
   * overwrites the keys present in `patch`. Designed for animation
   * fast-paths — tweens deliver one key at a time without clobbering
   * the rest of the node's layout.
   */
  export function updateStyle(node: NodeId, patch: Partial<Style>): void;

  /**
   * Mark `root` as the live UI root. After the JS `main()` returns,
   * the runtime starts driving a frame loop with this root.
   */
  export function run(root: NodeId): void;

  // ===== Input =====================================================

  /** A high-level intent dispatched by the runtime's intent router. */
  export interface IntentEvent {
    name: string;
    kind: 'pressed' | 'released' | 'repeat';
  }

  /** Raw keyboard / gamepad / mouse event passed to `addRawInputListener`. */
  export interface RawInputEvent {
    source: 'keyboard' | 'gamepad' | 'mouse';
    /** Linux evdev keycode for keyboard / gamepad button. */
    code?: number;
    pressed?: boolean;
    /** True iff this is a kernel auto-repeat fire. (Keyboard only.) */
    repeat?: boolean;
    /** Gamepad axis events. */
    kind?: 'button' | 'axis' | 'move' | 'wheel';
    axis?: number;
    value?: number;
    /** Mouse motion / wheel deltas. */
    dx?: number;
    dy?: number;
    delta?: number;
  }

  /** Optional per-listener configuration. */
  export interface ListenerOpts {
    /** When `true` (default), the listener fires regardless of focus.
     *  When `false`, it only fires when the focused subtree includes
     *  `nodeId`. */
    global?: boolean;
    nodeId?: NodeId;
  }

  /** Subscribe to a high-level intent. Returns a numeric `id` —
   *  pass it to `removeListener` to unsubscribe. */
  export function addIntentListener(
    name: string,
    handler: (e: IntentEvent) => void,
    opts?: ListenerOpts,
  ): number;

  /** Subscribe to raw events from a specific input source. */
  export function addRawInputListener(
    source: 'keyboard' | 'gamepad' | 'mouse',
    handler: (e: RawInputEvent) => void,
    opts?: ListenerOpts,
  ): number;

  /** Unsubscribe a listener previously returned by `add*Listener`. */
  export function removeListener(id: number): boolean;

  /** Push `node` onto the focus stack — it becomes the active focus. */
  export function pushFocus(node: NodeId): void;
  /** Pop the top of the focus stack and return the previous value. */
  export function popFocus(): NodeId | null;
  /** Replace the entire focus stack with just `node`. */
  export function setFocus(node: NodeId): void;
  /** Read the current focus (top of stack), or null if empty. */
  export function getFocus(): NodeId | null;

  // ===== Diagnostics ===============================================

  /**
   * Rolling-average frames-per-second over the last 1-second window.
   * Returns 0 until the first window completes (first frame).
   */
  export function fps(): number;

  /**
   * Schedule `cb` for invocation on the next frame. The callback fires
   * once and receives the current high-resolution timestamp (ms since
   * runtime start), matching the browser `DOMHighResTimeStamp` shape.
   *
   * To keep animating, the callback re-schedules itself by calling
   * `requestAnimationFrame` again from inside its body. Cancellations
   * land in the same queue with [`cancelAnimationFrame`].
   *
   * Returns a non-zero handle suitable for `cancelAnimationFrame`.
   */
  export function requestAnimationFrame(cb: (now: number) => void): number;

  /**
   * Cancel a previously-scheduled callback. No-op if the callback has
   * already fired or the handle was never registered.
   */
  export function cancelAnimationFrame(id: number): void;

  /**
   * Pre-build font atlases at one or more sizes covering every
   * character in `chars`. Queue from app init; the runtime drains
   * the requests once between bundle eval and the first frame, so
   * subsequent text renders never trigger a synchronous atlas
   * rebuild. `sizes` may be a single number or an array.
   */
  export function warmupGlyphs(
    family: string,
    sizes: number | number[],
    chars: string,
  ): void;
}
