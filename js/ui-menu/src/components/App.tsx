import * as React from 'react';
import { Background } from './Background';
import { Menu } from './Menu';

const MENU_ITEMS = [
  { label: 'Start', action: 'start' },
  { label: 'Blue', action: 'blue' },
  { label: 'Quit', action: 'quit' },
];

interface AppProps {
  onSelect: (action: string) => void;
  inputRef: React.MutableRefObject<((event: string) => void) | null>;
}

export function App({ onSelect, inputRef }: AppProps) {
  const [focused, setFocused] = React.useState(0);

  // Expose the input handler so the render loop can call it
  inputRef.current = (event: string) => {
    if (event === 'up') {
      setFocused(i => (i > 0 ? i - 1 : MENU_ITEMS.length - 1));
    } else if (event === 'down') {
      setFocused(i => (i < MENU_ITEMS.length - 1 ? i + 1 : 0));
    } else if (event === 'select') {
      onSelect(MENU_ITEMS[focused].action);
    }
  };

  return (
    <view
      flexDirection="column"
      justifyContent="center"
      alignItems="center"
      flexGrow={1}
    >
      <Background src="embedded:background" />
      <Menu items={MENU_ITEMS} focused={focused} />
    </view>
  );
}
