import * as React from 'react';
import Reconciler from 'react-reconciler';
import * as dom from '1fpga:dom';

type Instance = number; // NodeId
type TextInstance = number;
type Container = number;
type Props = Record<string, any>;

function applyProps(nodeId: number, oldProps: Props, newProps: Props) {
  // Remove old props not in newProps
  for (const key of Object.keys(oldProps)) {
    if (key === 'children' || key === 'key' || key === 'ref') continue;
    if (!(key in newProps)) {
      dom.setProp(nodeId, key, undefined);
    }
  }
  // Set new/changed props
  for (const [key, value] of Object.entries(newProps)) {
    if (key === 'children' || key === 'key' || key === 'ref') continue;
    if (oldProps[key] !== value) {
      dom.setProp(nodeId, key, value);
    }
  }
}

const reconciler = Reconciler({
  supportsMutation: true,
  supportsPersistence: false,
  supportsHydration: false,

  createInstance(type: string, props: Props): Instance {
    console.log('createInstance', type);
    const nodeId = dom.createElement(type);
    applyProps(nodeId, {}, props);
    return nodeId;
  },

  createTextInstance(text: string): TextInstance {
    return dom.createText(text);
  },

  appendInitialChild(parent: Instance, child: Instance) {
    dom.appendChild(parent, child);
  },

  appendChild(parent: Instance, child: Instance) {
    dom.appendChild(parent, child);
  },

  appendChildToContainer(container: Container, child: Instance) {
    dom.appendChild(container, child);
  },

  insertBefore(parent: Instance, child: Instance, before: Instance) {
    dom.insertBefore(parent, child, before);
  },

  insertInContainerBefore(container: Container, child: Instance, before: Instance) {
    dom.insertBefore(container, child, before);
  },

  removeChild(parent: Instance, child: Instance) {
    dom.removeChild(parent, child);
  },

  removeChildFromContainer(container: Container, child: Instance) {
    dom.removeChild(container, child);
  },

  commitTextUpdate(textInstance: TextInstance, _oldText: string, newText: string) {
    dom.setTextContent(textInstance, newText);
  },

  commitUpdate(
    instance: Instance,
    _type: string,
    oldProps: Props,
    newProps: Props,
  ) {
    applyProps(instance, oldProps, newProps);
  },

  prepareUpdate(): boolean {
    return true;
  },

  finalizeInitialChildren(): boolean {
    return false;
  },

  getPublicInstance(instance: Instance): Instance {
    return instance;
  },

  getRootHostContext() {
    return {};
  },

  getChildHostContext() {
    return {};
  },

  shouldSetTextContent(): boolean {
    return false;
  },

  prepareForCommit(): null {
    return null;
  },

  resetAfterCommit() {
    dom.requestRender();
  },

  clearContainer(_container: Container) {
    // no-op for now
  },

  detachDeletedInstance() {},
  preparePortalMount() {},

  scheduleTimeout: setTimeout as any,
  cancelTimeout: clearTimeout as any,
  noTimeout: -1,
  isPrimaryRenderer: true,
  getCurrentEventPriority: () => 99,
  getInstanceFromNode: () => null,
  prepareScopeUpdate: () => {},
  getInstanceFromScope: () => null,
  setCurrentUpdatePriority: () => {},
  getCurrentUpdatePriority: () => 99,
  resolveUpdatePriority: () => 99,
} as any);

let container: any = null;

export function render(element: React.ReactElement) {
  const root = dom.getRootNode();
  if (!container) {
    // LegacyRoot = 0 for synchronous rendering
    container = reconciler.createContainer(root, 0, null, false, null, '', console.error, null);
  }
  reconciler.updateContainer(element, container, null, null);
}
