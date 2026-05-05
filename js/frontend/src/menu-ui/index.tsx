// menu-ui entry — N2: React reconciler integration.
//
// JSX renders through `react-reconciler` into the Rust-side host tree
// via `1fpga:gui`. Same visual outcome as N1 (full-screen dark blue +
// red box), but this time React owns the tree.

import { App } from './App';
import { render } from './reconciler';

export async function main(): Promise<void> {
  console.log('menu-ui: main() entered');
  render(<App />);
  console.log('menu-ui: render() returned');
}
