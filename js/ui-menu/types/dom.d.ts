declare module '1fpga:dom' {
  export function createElement(type: string): number;
  export function createText(text: string): number;
  export function appendChild(parent: number, child: number): void;
  export function insertBefore(parent: number, child: number, before: number): void;
  export function removeChild(parent: number, child: number): void;
  export function removeNode(nodeId: number): void;
  export function setProp(nodeId: number, key: string, value: any): void;
  export function setTextContent(nodeId: number, text: string): void;
  export function getRootNode(): number;
  export function requestRender(): void;
  export function flushJobs(): void;
  export function exitRenderLoop(value?: string): void;
  export function animate(
    nodeId: number,
    options: {
      property: 'opacity' | 'translateX' | 'translateY' | 'scale';
      from: number;
      to: number;
      duration: number;
      easing?: 'linear' | 'easeIn' | 'easeOut' | 'easeInOut';
    },
  ): void;
  export function startRenderLoop(onInput: (event: string) => void): Promise<string>;
}
