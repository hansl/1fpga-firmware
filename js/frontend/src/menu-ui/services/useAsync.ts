// Tiny async-data hook: run an async producer, deliver
// {loading | value | error}, ignore stale settlements after deps
// change or unmount.

import { useEffect, useState } from 'react';

export interface AsyncState<T> {
  loading: boolean;
  value?: T;
  error?: unknown;
}

export function useAsync<T>(fn: () => Promise<T>, deps: unknown[]): AsyncState<T> {
  const [state, setState] = useState<AsyncState<T>>({ loading: true });
  useEffect(() => {
    let live = true;
    setState({ loading: true });
    fn().then(
      (value) => {
        if (live) setState({ loading: false, value });
      },
      (error) => {
        console.warn(`useAsync: ${error}`);
        if (live) setState({ loading: false, error });
      },
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return state;
}
