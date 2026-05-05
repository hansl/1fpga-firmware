declare module '1fpga:gui' {
  /**
   * Stable identifier for a node in the host tree. Returned by
   * `createInstance`, accepted by every other host call. The numeric
   * value is opaque — do not synthesize ids manually.
   */
  export type NodeId = number;

  /**
   * Style fields recognised by N1 (more land in later milestones).
   * `width` / `height` / `top` / `left` are integer pixels.
   * `backgroundColor` is a CSS color string (`#rgb`, `#rrggbb`,
   * `#rrggbbaa`).
   */
  export interface Style {
    backgroundColor?: string;
    width?: number;
    height?: number;
    top?: number;
    left?: number;
  }

  /**
   * Props bag accepted by `createInstance`. Mirrors React's instance
   * props shape so the upcoming reconciler integration drops in
   * unchanged.
   */
  export interface Props {
    style?: Style;
    className?: string;
    children?: unknown;
  }

  /**
   * Allocate a new host node. `type` selects the renderer; N1 ships
   * `'div'` only.
   */
  export function createInstance(type: 'div', props?: Props): NodeId;

  /** Append `child` to `parent`'s children list. */
  export function appendChild(parent: NodeId, child: NodeId): void;

  /** Replace `node`'s style with `style`. */
  export function setStyle(node: NodeId, style: Style): void;

  /**
   * Mark `root` as the live UI root. After the JS `main()` returns,
   * the runtime starts driving a frame loop with this root.
   */
  export function run(root: NodeId): void;
}
