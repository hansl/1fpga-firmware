// Must be imported BEFORE React to ensure the scheduler sees our polyfills.

// Polyfill performance.now() — advance by 10ms per call so React's
// scheduler yields quickly in Boa's synchronous execution model.
let _perfCounter = Date.now();
(globalThis as any).performance = {
  now: () => {
    _perfCounter += 10;
    return _perfCounter;
  },
};
