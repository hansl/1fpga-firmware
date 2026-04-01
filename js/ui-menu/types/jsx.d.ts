// Custom JSX element types for the 1FPGA React renderer.
// These map to Rust DOM node types via the react-reconciler bridge.
// We use module augmentation to override React's built-in SVG element types.

import 'react';

declare module 'react' {
  namespace JSX {
    interface IntrinsicElements {
      view: {
        key?: string | number;
        children?: any;
        width?: number;
        height?: number;
        flexDirection?: 'row' | 'column';
        justifyContent?:
          | 'flex-start'
          | 'flex-end'
          | 'center'
          | 'space-between'
          | 'space-around'
          | 'space-evenly';
        alignItems?: 'flex-start' | 'flex-end' | 'center' | 'stretch' | 'baseline';
        padding?: number;
        margin?: number;
        gap?: number;
        flexGrow?: number;
        flexShrink?: number;
        position?: 'relative' | 'absolute';
        top?: number;
        left?: number;
        right?: number;
        bottom?: number;
        backgroundColor?: number[] | string;
        opacity?: number;
        borderRadius?: number;
      };
      text: {
        key?: string | number;
        children?: any;
        color?: number[] | string;
        fontSize?: number;
      };
      image: {
        key?: string | number;
        src?: string;
        width?: number;
        height?: number;
        position?: 'relative' | 'absolute';
        top?: number;
        left?: number;
        right?: number;
        bottom?: number;
      };
    }
  }
}

declare module 'react-reconciler' {
  const Reconciler: any;
  export default Reconciler;
}
