export function shutdown(): never {
  // This runs in the worker.
  postMessage({ kind: "shutdown" });
  while (true) {
  }
}
