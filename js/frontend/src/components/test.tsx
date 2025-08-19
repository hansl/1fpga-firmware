import { render } from '@1fpga/react-osd';
import { FunctionComponent, PropsWithChildren, useEffect, useState } from 'react';

// namespace React {
//   namespace JSX {
//     interface IntrinsicElements {
//       t: FunctionComponent<PropsWithChildren<{ font: string }>>;
//     }
//   }
// }

export function TestComponent({ state }: { state: { done: boolean } }) {
  setTimeout(() => {
    console.log(JSON.stringify(state));
    state.done = true;
    console.log(JSON.stringify(state));
  }, 100000);

  const [count, setCount] = useState(0);
  console.log('TestComponent', count);
  useEffect(() => {
    setInterval(() => {
      setCount(count => count + 1);
    }, 1000);
  }, []);

  return (
    <box>
      <box font="small">
        Node {count}. After. {'\n'}
      </box>
      <box font="medium" location={{ x: 10, y: 20 }}>
        Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut
        labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco
        laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in
        voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat
        cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.
      </box>
    </box>
  );
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
