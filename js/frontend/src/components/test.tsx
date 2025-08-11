import { render } from '@1fpga/react-osd';
import React, { useEffect } from 'react';

import * as dom from '1fpga:dom';

export function TestComponent({ state }: { state: { done: boolean } }) {
  setTimeout(() => {
    console.log(JSON.stringify(state));
    state.done = true;
    console.log(JSON.stringify(state));
  }, 100000);

  const [count, setCount] = React.useState(0);
  console.log('TestComponent', count);
  useEffect(() => {
    setInterval(() => {
      setCount(count => count + 1);
    }, 1000);
  }, []);

  return <div>Hello Node {'' + count}</div>;
}

export async function run() {
  let state = {
    done: false,
  };

  render(<TestComponent state={state} />, () => console.log('rendered ?'));

  while (!state.done) {
    await new Promise(r => setTimeout(r, 500));
  }

  console.log('done.1');
  await new Promise(r => setTimeout(r, 1000));
  console.log('done.2');
}
