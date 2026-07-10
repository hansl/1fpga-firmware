// Application shell — deliberately thin. No cursor state, no intent
// handlers, no screen knowledge: providers plus the route table.
// Screens own their state; focus zones own their input (see
// focus/index.tsx); the initial route comes from the settings data
// table (see boot.ts), which is also how "boot straight into a
// screen" works.

import { FocusProvider } from './focus';
import { RouterProvider, Routes } from './router';
import { resolveStartupRoute } from './boot';
import { HomeScreen } from './screens/Home';
import { CollectionScreen } from './screens/Collection';

export function App() {
  return (
    <RouterProvider resolveInitial={resolveStartupRoute}>
      <FocusProvider initial="carousel">
        <Routes
          routes={[
            { pattern: '/home', render: () => <HomeScreen /> },
            {
              pattern: '/collections/:id',
              render: (p) => <CollectionScreen id={p.id} />,
            },
          ]}
        />
      </FocusProvider>
    </RouterProvider>
  );
}
