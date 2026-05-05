// react-reconciler HostConfig wired to `1fpga:gui`.
//
// Mutation mode: every change React makes turns into a `gui.appendChild`
// / `gui.insertBefore` / `gui.removeChild` / `gui.commitUpdate` call.
// Most of the surface (host-context tracking, transition / scheduler /
// suspend hooks) is irrelevant to a single-render-target embedded UI
// — those slots are filled with no-op stubs that match the interface.

import * as gui from '1fpga:gui';
import type { ReactNode } from 'react';
import ReactReconciler from 'react-reconciler';
import { DefaultEventPriority } from 'react-reconciler/constants';

type Type = 'div';
type Props = gui.Props;
type NodeId = gui.NodeId;
type Container = NodeId;

// `null` rather than a sentinel because we don't track host context.
type HostContext = null;
type TimeoutHandle = number;
type NoTimeout = -1;

const NO_TIMEOUT: NoTimeout = -1;

const reconciler = ReactReconciler<
  Type,
  Props,
  Container,
  /* Instance        */ NodeId,
  /* TextInstance    */ never,
  /* SuspenseInst    */ never,
  /* HydratableInst  */ never,
  /* FormInstance    */ never,
  /* PublicInstance  */ NodeId,
  HostContext,
  /* ChildSet        */ never,
  TimeoutHandle,
  NoTimeout,
  /* TransitionStatus */ null
>({
  // ---- Mode -------------------------------------------------------
  supportsMutation: true,
  supportsPersistence: false,
  supportsHydration: false,
  isPrimaryRenderer: true,

  // ---- Tree construction (render phase) ---------------------------
  createInstance(type, props) {
    return gui.createInstance(type, props);
  },
  createTextInstance() {
    // N4 adds real text rendering. Until then, ignore string children
    // by returning a sentinel that's never used in the host tree.
    throw new Error('text nodes are not supported yet (lands in N4)');
  },
  appendInitialChild(parent, child) {
    gui.appendChild(parent, child);
  },
  finalizeInitialChildren() {
    return false;
  },
  shouldSetTextContent() {
    return false;
  },

  // ---- Mutation primitives (commit phase) -------------------------
  appendChild(parent, child) {
    gui.appendChild(parent, child);
  },
  appendChildToContainer(container, child) {
    gui.appendChild(container, child);
  },
  insertBefore(parent, child, before) {
    gui.insertBefore(parent, child, before);
  },
  insertInContainerBefore(container, child, before) {
    gui.insertBefore(container, child, before);
  },
  removeChild(parent, child) {
    gui.removeChild(parent, child);
  },
  removeChildFromContainer(container, child) {
    gui.removeChild(container, child);
  },
  commitUpdate(instance, _type, _prevProps, nextProps) {
    gui.commitUpdate(instance, nextProps);
  },
  clearContainer() {
    // N1 doesn't need container clearing — root nodes persist across
    // renders. (React only calls this when unmounting the whole tree.)
  },

  // ---- Host context (we don't use any) ----------------------------
  getRootHostContext: () => null,
  getChildHostContext: () => null,
  getPublicInstance: (instance) => instance,

  // ---- Commit lifecycle (no-ops) ----------------------------------
  prepareForCommit: () => null,
  resetAfterCommit: () => {},
  preparePortalMount: () => {},

  // ---- Scheduling ------------------------------------------------
  // We don't have real timers in Boa; sync (legacy) root mode never
  // needs them. The stub just runs immediately for delay=0 callers
  // (avoids React deadlocking on its self-debounce paths) and ignores
  // longer delays for now.
  scheduleTimeout(fn, delay) {
    if (!delay) fn();
    return NO_TIMEOUT;
  },
  cancelTimeout() {},
  noTimeout: NO_TIMEOUT,

  // ---- Update priority (concurrent mode internals) ----------------
  setCurrentUpdatePriority: () => {},
  getCurrentUpdatePriority: () => DefaultEventPriority,
  resolveUpdatePriority: () => DefaultEventPriority,

  // ---- Suspense / transitions / forms (all unused) ----------------
  maySuspendCommit: () => false,
  preloadInstance: () => true,
  startSuspendingCommit: () => {},
  suspendInstance: () => {},
  waitForCommitToBeReady: () => null,
  NotPendingTransition: null,
  HostTransitionContext: null as never,
  resetFormInstance: () => {},
  requestPostPaintCallback: () => {},
  shouldAttemptEagerTransition: () => false,
  trackSchedulerEvent: () => {},
  resolveEventType: () => null,
  resolveEventTimeStamp: () => -1.1,

  // ---- Scope / focus management (unused) --------------------------
  beforeActiveInstanceBlur: () => {},
  afterActiveInstanceBlur: () => {},
  prepareScopeUpdate: () => {},
  getInstanceFromScope: () => null,
  getInstanceFromNode: () => null,
  detachDeletedInstance: () => {},
});

/**
 * Mount `element` as the menu-ui root and hand control to the Rust
 * frame loop. Returns when the runtime exits.
 */
export function render(element: ReactNode): void {
  // Pre-create the root host node — React mutates under it.
  const rootNode = gui.createInstance('div', {
    style: { width: 1920, height: 1080, top: 0, left: 0 },
  });

  // tag=0 → legacy / sync root. Concurrent mode (1) needs real
  // scheduler timers we don't have yet.
  const container = reconciler.createContainer(
    rootNode,
    /* tag */ 0,
    /* hydrationCallbacks */ null,
    /* isStrictMode */ false,
    /* concurrentUpdatesByDefault */ null,
    /* identifierPrefix */ '',
    /* onUncaughtError */ (error) => console.error('uncaught:', String(error)),
    /* onCaughtError */ (error) => console.error('caught:', String(error)),
    /* onRecoverableError */ (error) => console.warn('recoverable:', String(error)),
    /* onDefaultTransitionIndicator */ () => {},
  );

  reconciler.updateContainer(element, container, null, null);

  gui.run(rootNode);
}
