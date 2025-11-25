import * as React from 'react';

type TextChild = number | string;

type NodeStyle<P = {}> = P & {
  font?: string;
}

type BoxStyle = NodeStyle<{ location?: { x: number; y: number } }>;
type TStyle = NodeStyle<>;

declare module "react/jsx-runtime" {
  namespace JSX {
      interface IntrinsicElements {
        box: React.PropsWithChildren<{ style?: BoxStyle }>;

        /// A text box, containing only text children.
        t: {
          style?: TStyle;
          children?: TextChild[] | TextChild;
        };
      }
    }
}
