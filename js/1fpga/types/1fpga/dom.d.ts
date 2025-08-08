// Type definitions for `1fpga:dom` module.
import type React from 'react';

/** Document Object Model for the 1FPGA UI. */
declare module '1fpga:dom' {
  export class Text extends React.ComponentWithChildren<{ position: { x: number; y: number } }> {}

  export class Node {
    append(child: Node | TextFragment): void;

    text: string | undefined;
    readonly tagName: string | undefined;
  }

  export class Root extends Node {
    render();
  }

  export function createNode(type: string, props: object): Node;

  export function createFragment(str: string): TextFragment;

  export function root(): Root;
}
