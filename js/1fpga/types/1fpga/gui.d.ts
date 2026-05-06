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
   * Mark `root` as the live UI root. After the JS `main()` returns,
   * the runtime starts driving a frame loop with this root.
   */
  export function run(root: NodeId): void;
}
