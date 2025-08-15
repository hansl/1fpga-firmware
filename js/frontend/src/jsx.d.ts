import * as React from 'react';

declare global {
  namespace React {
    namespace JSX {
      interface IntrinsicElements {
        box: React.PropsWithChildren<{ font?: string; location?: { x: number; y: number } }>;
      }
    }
  }
}
