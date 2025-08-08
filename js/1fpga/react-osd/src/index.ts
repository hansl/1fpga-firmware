import * as React from 'react';
import ReactFiberReconciler, { HostConfig, OpaqueHandle, OpaqueRoot } from 'react-reconciler';
import { DefaultEventPriority } from 'react-reconciler/constants';

import * as dom from '1fpga:dom';

interface HostContext {}

type Type = string;

type Props = object;

type Container = dom.Root;

type Instance = dom.Node;
type TextInstance = dom.Node;
type SuspenseInstance = unknown;
type HydratableInstance = unknown;
type FormInstance = unknown;
type PublicInstance = unknown;
type ChildSet = unknown;
type TimeoutHandle = NodeJS.Timeout;
type NoTimeout = unknown;
type TransitionStatus = unknown;

export function createRenderer() {
  const hostConfig: HostConfig<
    Type,
    Props,
    Container,
    Instance,
    TextInstance,
    SuspenseInstance,
    HydratableInstance,
    FormInstance,
    PublicInstance,
    HostContext,
    ChildSet,
    TimeoutHandle,
    NoTimeout,
    TransitionStatus
  > = {
    supportsMutation: true,
    supportsMicrotasks: true,
    supportsHydration: false,
    supportsPersistence: false,

    /**
     * This method should return a newly created node. For example, the DOM renderer would call
     * `document.createElement(type)` here and then set the properties from `props`.
     *
     * You can use `rootContainer` to access the root container associated with that tree. For
     * example, in the DOM renderer, this is useful to get the correct `document` reference that the
     * root belongs to.
     *
     * The `hostContext` parameter lets you keep track of some information about your current place
     * in the tree. To learn more about it, see `getChildHostContext` below.
     *
     * The `internalHandle` data structure is meant to be opaque. If you bend the rules and rely on
     * its internal fields, be aware that it may change significantly between versions. You're
     * taking on additional maintenance risk by reading from it, and giving up all guarantees if you
     * write something to it.
     *
     * This method happens **in the render phase**. It can (and usually should) mutate the node it
     * has just created before returning it, but it must not modify any other nodes. It must not
     * register any event handlers on the parent tree. This is because an instance being created
     * doesn't guarantee it would be placed in the tree — it could be left unused and later
     * collected by GC. If you need to do something when an instance is definitely in the tree, look
     * at `commitMount` instead.
     */
    createInstance(
      type: Type,
      props: Props,
      rootContainer: Container,
      hostContext: HostContext,
      internalHandle: OpaqueHandle,
    ): Instance {
      console.log('createNode', JSON.stringify(rootContainer));
      return dom.createNode(type, props);
    },

    /**
     * Same as `createInstance`, but for text nodes. If your renderer doesn't support text nodes,
     * you can throw here.
     */
    createTextInstance(
      text: string,
      rootContainer: Container,
      hostContext: HostContext,
      internalHandle: OpaqueHandle,
    ): TextInstance {
      console.log('createFragment');
      return dom.createFragment(text);
    },

    /**
     * This method should mutate the `parentInstance` and add the child to its list of children. For
     * example, in the DOM this would translate to a `parentInstance.appendChild(child)` call.
     *
     * This method happens **in the render phase**. It can mutate `parentInstance` and `child`, but
     * it must not modify any other nodes. It's called while the tree is still being built up and
     * not connected to the actual tree on the screen.
     */
    appendInitialChild(parentInstance: Instance, child: Instance | TextInstance): void {
      console.log('appendInitialChild', JSON.stringify(child));
      parentInstance.append(child);
    },

    /**
     * In this method, you can perform some final mutations on the `instance`. Unlike with
     * `createInstance`, by the time `finalizeInitialChildren` is called, all the initial children
     * have already been added to the `instance`, but the instance itself has not yet been connected
     * to the tree on the screen.
     *
     * This method happens **in the render phase**. It can mutate `instance`, but it must not modify
     * any other nodes. It's called while the tree is still being built up and not connected to the
     * actual tree on the screen.
     *
     * There is a second purpose to this method. It lets you specify whether there is some work that
     * needs to happen when the node is connected to the tree on the screen. If you return `true`,
     * the instance will receive a `commitMount` call later. See its documentation below.
     *
     * If you don't want to do anything here, you should return `false`.
     */
    finalizeInitialChildren(
      instance: Instance,
      type: Type,
      props: Props,
      rootContainer: Container,
      hostContext: HostContext,
    ): boolean {
      console.log('finalizeInitialChildren');
      return false;
    },

    /**
     * Some target platforms support setting an instance's text content without manually creating a
     * text node. For example, in the DOM, you can set `node.textContent` instead of creating a text
     * node and appending it.
     *
     * If you return `true` from this method, React will assume that this node's children are text,
     * and will not create nodes for them. It will instead rely on you to have filled that text
     * during `createInstance`. This is a performance optimization. For example, the DOM renderer
     * returns `true` only if `type` is a known text-only parent (like `'textarea'`) or if
     * `props.children` has a `'string'` type. If you return `true`, you will need to implement
     * `resetTextContent` too.
     *
     * If you don't want to do anything here, you should return `false`.
     *
     * This method happens **in the render phase**. Do not mutate the tree from it.
     */
    shouldSetTextContent(type: Type, props: Props): boolean {
      console.log('shouldSetTextContent', type, JSON.stringify(props));
      return false;
    },

    /**
     * This method lets you return the initial host context from the root of the tree. See
     * `getChildHostContext` for the explanation of host context.
     *
     * If you don't intend to use host context, you can return `null`.
     *
     * This method happens **in the render phase**. Do not mutate the tree from it.
     */
    getRootHostContext(rootContainer: Container): HostContext | null {
      console.log('getRootHostContext');
      return null;
    },

    /**
     * Host context lets you track some information about where you are in the tree so that it's
     * available inside `createInstance` as the `hostContext` parameter. For example, the DOM
     * renderer uses it to track whether it's inside an HTML or an SVG tree, because
     * `createInstance` implementation needs to be different for them.
     *
     * If the node of this `type` does not influence the context you want to pass down, you can
     * return `parentHostContext`. Alternatively, you can return any custom object representing the
     * information you want to pass down.
     *
     * If you don't want to do anything here, return `parentHostContext`.
     *
     * This method happens **in the render phase**. Do not mutate the tree from it.
     */
    getChildHostContext(
      parentHostContext: HostContext,
      type: Type,
      rootContainer: Container,
    ): HostContext {
      console.log('getChildHostContext');
      return {};
    },

    getPublicInstance(instance: unknown): unknown {
      console.log('getPublicInstance');
      return {};
    },

    /**
     * This method lets you store some information before React starts making changes to the tree on
     * the screen. For example, the DOM renderer stores the current text selection so that it can
     * later restore it. This method is mirrored by `resetAfterCommit`.
     *
     * Even if you don't want to do anything here, you need to return `null` from it.
     */
    prepareForCommit(containerInfo: Container): Record<string, any> | null {
      return null;
    },

    /**
     * This method is called right after React has performed the tree mutations. You can use it to
     * restore something you've stored in `prepareForCommit` — for example, text selection.
     *
     * You can leave it empty.
     */
    resetAfterCommit(containerInfo: Container): void {},

    preparePortalMount(containerInfo: Container): void {
      console.log('preparePortalMount');
    },

    /** You can proxy this to `setTimeout` or its equivalent in your environment. */
    scheduleTimeout(fn: (...args: unknown[]) => unknown, delay?: number): TimeoutHandle {
      return setTimeout(fn, delay);
    },
    cancelTimeout(id: unknown): void {
      console.log('cancelTimeout');
    },
    noTimeout: -1,
    isPrimaryRenderer: true,
    warnsIfNotActing: true,
    getInstanceFromNode(node: any): ReactFiberReconciler.Fiber | null | undefined {
      console.log(`getInstanceFromNode(${node})`);
      return null;
    },
    beforeActiveInstanceBlur(): void {
      console.log('beforeActiveInstanceBlur');
    },
    afterActiveInstanceBlur(): void {
      console.log('afterActiveInstanceBlur');
    },
    prepareScopeUpdate(scopeInstance: any, instance: any): void {
      console.log('prepareScopeUpdate');
    },
    getInstanceFromScope(scopeInstance: any): null | Instance {
      console.log('getInstanceFromScope');
      return null;
    },
    detachDeletedInstance(node: unknown): void {
      console.log('detachDeletedInstance');
    },

    // -------------------
    //  Mutation Methods
    //    (optional)
    //  If you're using React in mutation mode (you probably do), you'll need to implement a few more methods.
    // -------------------

    /**
     * This method should mutate the `parentInstance` and add the child to its list of children. For
     * example, in the DOM this would translate to a `parentInstance.appendChild(child)` call.
     *
     * Although this method currently runs in the commit phase, you still should not mutate any
     * other nodes in it. If you need to do some additional work when a node is definitely connected
     * to the visible tree, look at `commitMount`.
     */
    appendChild(parentInstance: Instance, child: Instance | TextInstance): void {
      console.log(`appendChild(${parentInstance}, ${child});`);
      parentInstance.append(child);
    },

    /**
     * Same as `appendChild`, but for when a node is attached to the root container. This is useful
     * if attaching to the root has a slightly different implementation, or if the root container
     * nodes are of a different type than the rest of the tree.
     */
    appendChildToContainer(container: Container, child: Instance | TextInstance): void {
      console.log('appendChildToContainer');
      container.append(child);
      console.log('appendChildToContainer.2');
    },

    /**
     * This method should mutate the `parentInstance` and place the `child` before `beforeChild` in
     * the list of its children. For example, in the DOM this would translate to a
     * `parentInstance.insertBefore(child, beforeChild)` call.
     *
     * Note that React uses this method both for insertions and for reordering nodes. Similar to
     * DOM, it is expected that you can call `insertBefore` to reposition an existing child. Do not
     * mutate any other parts of the tree from it.
     */
    insertBefore(
      parentInstance: Instance,
      child: Instance | TextInstance,
      beforeChild: Instance | TextInstance | SuspenseInstance,
    ): void {
      console.log('insertBefore');
    },

    /**
     * Same as `insertBefore`, but for when a node is attached to the root container. This is useful
     * if attaching to the root has a slightly different implementation, or if the root container
     * nodes are of a different type than the rest of the tree.
     */
    insertInContainerBefore(
      container: Container,
      child: Instance | TextInstance,
      beforeChild: Instance | TextInstance | SuspenseInstance,
    ): void {
      console.log('insertInContainerBefore');
    },

    /**
     * This method should mutate the `parentInstance` to remove the `child` from the list of its
     * children.
     *
     * React will only call it for the top-level node that is being removed. It is expected that
     * garbage collection would take care of the whole subtree. You are not expected to traverse the
     * child tree in it.
     */
    removeChild(parentInstance: Instance, child: Instance | TextInstance | SuspenseInstance): void {
      console.log('removeChild');
    },

    /**
     * Same as `removeChild`, but for when a node is detached from the root container. This is
     * useful if attaching to the root has a slightly different implementation, or if the root
     * container nodes are of a different type than the rest of the tree.
     */
    removeChildFromContainer(
      container: Container,
      child: Instance | TextInstance | SuspenseInstance,
    ): void {
      console.log('removeChildFromContainer');
    },

    /**
     * If you returned `true` from `shouldSetTextContent` for the previous props, but returned
     * `false` from `shouldSetTextContent` for the next props, React will call this method so that
     * you can clear the text content you were managing manually. For example, in the DOM you could
     * set `node.textContent = ''`.
     *
     * If you never return `true` from `shouldSetTextContent`, you can leave it empty.
     */
    resetTextContent(instance: Instance): void {
      console.log('resetTextContent');
    },

    /**
     * This method should mutate the `textInstance` and update its text content to `nextText`.
     *
     * Here, `textInstance` is a node created by `createTextInstance`.
     */
    commitTextUpdate(textInstance: TextInstance, oldText: string, newText: string): void {
      console.log(
        `commitTextUpdate(old: ${JSON.stringify(oldText)}, new: ${JSON.stringify(newText)})`,
      );
      textInstance.text = newText;
    },

    /**
     * This method is only called if you returned `true` from `finalizeInitialChildren` for this
     * instance.
     *
     * It lets you do some additional work after the node is actually attached to the tree on the
     * screen for the first time. For example, the DOM renderer uses it to trigger focus on nodes
     * with the `autoFocus` attribute.
     *
     * Note that `commitMount` does not mirror `removeChild` one to one because `removeChild` is
     * only called for the top-level removed node. This is why ideally `commitMount` should not
     * mutate any nodes other than the `instance` itself. For example, if it registers some events
     * on some node above, it will be your responsibility to traverse the tree in `removeChild` and
     * clean them up, which is not ideal.
     *
     * The `internalHandle` data structure is meant to be opaque. If you bend the rules and rely on
     * its internal fields, be aware that it may change significantly between versions. You're
     * taking on additional maintenance risk by reading from it, and giving up all guarantees if you
     * write something to it.
     *
     * If you never return `true` from `finalizeInitialChildren`, you can leave it empty.
     */
    commitMount(
      instance: Instance,
      type: Type,
      props: Props,
      internalInstanceHandle: OpaqueHandle,
    ): void {
      console.log('commitMount');
    },

    /**
     * This method should mutate the instance to match nextProps.
     *
     * The internalHandle data structure is meant to be opaque. If you bend the rules and rely on
     * its internal fields, be aware that it may change significantly between versions. You're
     * taking on additional maintenance risk by reading from it, and giving up all guarantees if you
     * write something to it.
     */
    commitUpdate(
      instance: Instance,
      type: Type,
      prevProps: Props,
      nextProps: Props,
      internalHandle: OpaqueHandle,
    ): void {
      console.log(
        `commitUpdate(${type}, ${JSON.stringify(prevProps)}, ${JSON.stringify(nextProps)})`,
      );

      doRender();
    },

    /**
     * This method should make the `instance` invisible without removing it from the tree. For
     * example, it can apply visual styling to hide it. It is used by Suspense to hide the tree
     * while the fallback is visible.
     */
    hideInstance(instance: Instance): void {
      console.log('hideInstance');
    },

    /** Same as `hideInstance`, but for nodes created by `createTextInstance`. */
    hideTextInstance(textInstance: TextInstance): void {
      console.log('hideTextInstance');
    },

    /** This method should make the `instance` visible, undoing what `hideInstance` did. */
    unhideInstance(instance: Instance, props: Props): void {
      console.log('unhideInstance');
    },

    /** Same as `unhideInstance`, but for nodes created by `createTextInstance`. */
    unhideTextInstance(textInstance: TextInstance, text: string): void {
      console.log('unhideTextInstance');
    },

    /** This method should mutate the `container` root node and remove all children from it. */
    clearContainer(container: Container): void {
      console.log('clearContainer');
    },

    NotPendingTransition: null,
    HostTransitionContext: null as any,
    setCurrentUpdatePriority(newPriority: ReactFiberReconciler.EventPriority): void {
      console.log('setCurrentUpdatePriority');
    },
    getCurrentUpdatePriority(): ReactFiberReconciler.EventPriority {
      console.log('getCurrentUpdatePriority');
      return DefaultEventPriority;
    },
    resolveUpdatePriority(): ReactFiberReconciler.EventPriority {
      console.log('resolveUpdatePriority');
      return DefaultEventPriority;
    },
    resetFormInstance(form: unknown): void {
      console.log('resetFormInstance');
    },
    requestPostPaintCallback(callback: (time: number) => void): void {
      console.log('requestPostPaintCallback');
    },
    shouldAttemptEagerTransition(): boolean {
      console.log('shouldAttemptEagerTransition');
      return false;
    },
    trackSchedulerEvent(): void {
      console.log('trackSchedulerEvent');
    },
    resolveEventType(): null | string {
      console.log('resolveEventType');
      return null;
    },
    resolveEventTimeStamp(): number {
      console.log('resolveEventTimeStamp');
      return 0;
    },

    /**
     * This method is called during render to determine if the Host Component type and props require
     * some kind of loading process to complete before committing an update.
     */
    maySuspendCommit(type: unknown, props: unknown): boolean {
      console.log('maySuspendCommit');
      return false;
    },

    /**
     * This method may be called during render if the Host Component type and props might suspend a
     * commit. It can be used to initiate any work that might shorten the duration of a suspended
     * commit.
     */
    preloadInstance(type: unknown, props: unknown): boolean {
      console.log('preloadInstance');
      return false;
    },

    startSuspendingCommit(): void {
      console.log('startSuspendingCommit');
    },
    suspendInstance(type: unknown, props: unknown): void {
      console.log('suspendInstance');
    },
    waitForCommitToBeReady():
      | ((initiateCommit: (...args: unknown[]) => unknown) => (...args: unknown[]) => unknown)
      | null {
      console.log('waitForCommitToBeReady');
      return null;
    },
    scheduleMicrotask(fn: () => unknown): void {
      console.log('scheduleMicrotask');
      Promise.resolve().then(() => {
        try {
          return fn();
        } catch (e) {
          console.error('Error happened in microtask: ', e);
        }
      });
    },
  };

  const Reconciler = ReactFiberReconciler(hostConfig);

  /** There's only one root. */
  let reactRoot: OpaqueRoot;
  let root = dom.root();

  const debounce = <T extends (...args: any[]) => any>(callback: T, waitFor: number) => {
    let timeout: ReturnType<typeof setTimeout>;
    return (...args: Parameters<T>) => {
      timeout && clearTimeout(timeout);
      timeout = setTimeout(() => callback(...args), waitFor);
    };
  };

  let n = 0;
  /** Render the screen. */
  let doRender = debounce(() => {
    console.log(`render(${n++})`);
    root.render();
  }, 16);

  return function render(element: React.ReactNode, callback: (() => void) | null) {
    if (!reactRoot) {
      reactRoot = Reconciler.createContainer(
        root,
        0,
        null,
        false,
        false,
        '',
        error => {
          console.error('Recoverable error: ', error.message);
        },
        null,
      );
    }

    Reconciler.updateContainer(element, reactRoot, null, callback);
    doRender();
    return Reconciler.getPublicRootInstance(reactRoot);
  };
}

export function render(element: React.ReactNode, callback?: () => void) {
  const renderer = createRenderer();
  return renderer(element, callback ?? null);
}
