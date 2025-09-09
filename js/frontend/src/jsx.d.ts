type TextChild = number | string;

type NodeStyle<P = {}> = P & {
  font?: string;
}

type BoxStyle = NodeStyle<{  location?: { x: number; y: number } }>;
type TStyle = NodeStyle<>;

declare global {
  namespace React {
    namespace JSX {
      interface IntrinsicElements {
        box: React.PropsWithChildren<{ style?: BoxStyle}>;

        /// A text box, containing only text children.
        t: {
          style?: TStyle;
          location?: { x: number; y: number };
          children?: TextChild[] | TextChild;
        };
      }
    }
  }
}
