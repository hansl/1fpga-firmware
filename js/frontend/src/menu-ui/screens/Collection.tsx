// Collection: the selected system's card docks top-left, its game
// directory fills the screen as a browsable, virtualized list.
// Routed as '/collections/:sys' (system uniqueName) — deep-linkable
// via ui.startupRoute.
//
// Subdirectory navigation is local state (a path stack), so `back`
// unwinds the directory first and only then pops the route. Systems
// without a games directory (utility cores etc.) show the core's
// details instead of a listing. Launching a selected entry is the
// next feature — confirm on a file currently just logs.

import { useCallback, useEffect, useState } from 'react';
import type { CSSProperties } from 'react';

import { VW, VH, s } from '../scale';
import { ActionBar, type ActionBinding } from '../components/ActionBar';
import { Icon } from '../components/Icon';
import { FocusZone, useZoneIntent } from '../focus';
import { useRouter } from '../router';
import {
  getSystem,
  listEntries,
  type LibEntry,
  type SystemCard,
} from '../services/library';
import { useAsync } from '../services/useAsync';
import { GameList } from '../theme/GameList';

const root: CSSProperties = {
  position: 'relative',
  width: VW,
  height: VH,
};

const DOCK_X = s(32);
const DOCK_Y = s(90);
const DOCK_W = s(190);
const DOCK_H = s(230);
const LIST_TOP = s(380);

const dockCardStyle: CSSProperties = {
  position: 'absolute',
  left: DOCK_X,
  top: DOCK_Y,
  width: DOCK_W,
  height: DOCK_H,
  backgroundColor: '#131d29e6',
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'center',
  justifyContent: 'center',
  gap: s(18),
};

const titleStyle: CSSProperties = {
  position: 'absolute',
  left: DOCK_X + DOCK_W + s(40),
  top: DOCK_Y + s(28),
  fontSize: s(48),
  color: '#ffffff',
};

const subtitleStyle: CSSProperties = {
  position: 'absolute',
  left: DOCK_X + DOCK_W + s(40),
  top: DOCK_Y + s(104),
  fontSize: s(22),
  color: '#7e8ea0',
};

const msgStyle: CSSProperties = {
  position: 'absolute',
  left: s(96),
  top: LIST_TOP,
  fontSize: s(26),
  color: '#9fb4c8',
};

const LIST_ACTIONS: ActionBinding[] = [
  { intent: 'navigate_updown', label: 'Move' },
  { intent: 'confirm', label: 'Open' },
  { intent: 'back', label: 'Back' },
];

export function CollectionScreen({ sys }: { sys: string }) {
  const system = useAsync(() => getSystem(sys), [sys]);

  return (
    <div style={root}>
      <FocusZone id="list">
        {system.loading ? (
          <div style={msgStyle}>Loading…</div>
        ) : system.value ? (
          <Loaded system={system.value} />
        ) : (
          <NotFound sys={sys} />
        )}
      </FocusZone>
    </div>
  );
}

function DockedCard({ system }: { system: SystemCard }) {
  return (
    <>
      <div style={dockCardStyle}>
        <Icon name="sports_esports" size={s(72)} color="#5f7a90" />
        <div style={{ fontSize: s(22), color: '#e8f0f8' }}>{system.name}</div>
      </div>
      <div style={titleStyle}>{system.name}</div>
    </>
  );
}

function Loaded({ system }: { system: SystemCard }) {
  const router = useRouter();
  // Path segments below the system's games root.
  const [stack, setStack] = useState<string[]>([]);
  const [cursor, setCursor] = useState(0);

  const dirPath =
    system.gamesPath === null ? null : [system.gamesPath, ...stack].join('/');
  const entries = useAsync<LibEntry[]>(
    () => (dirPath === null ? Promise.resolve([]) : listEntries(dirPath)),
    [dirPath],
  );
  // New directory → cursor back to the top.
  useEffect(() => setCursor(0), [dirPath]);

  const list = entries.value ?? [];

  useZoneIntent(
    'navigate_up',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setCursor((c) => Math.max(0, c - 1));
      }
    }, []),
  );
  useZoneIntent(
    'navigate_down',
    useCallback(
      (e) => {
        if (e.kind === 'pressed' || e.kind === 'repeat') {
          setCursor((c) => Math.min(Math.max(0, list.length - 1), c + 1));
        }
      },
      [list.length],
    ),
  );
  useZoneIntent(
    'confirm',
    useCallback(
      (e) => {
        if (e.kind !== 'pressed') return;
        const entry = list[cursor];
        if (!entry) return;
        if (entry.dir) {
          setStack((st) => [...st, entry.name]);
        } else {
          // TODO: launch pipeline (core management on the engine
          // thread). For now, make the selection observable.
          console.log(`select: ${dirPath}/${entry.name}`);
        }
      },
      [list, cursor, dirPath],
    ),
  );
  useZoneIntent(
    'back',
    useCallback(
      (e) => {
        if (e.kind !== 'pressed') return;
        setStack((st) => {
          if (st.length > 0) return st.slice(0, -1);
          router.back();
          return st;
        });
      },
      [router],
    ),
  );

  const crumb = stack.length > 0 ? ` / ${stack.join(' / ')}` : '';
  return (
    <>
      <DockedCard system={system} />
      {system.gamesPath === null ? (
        <>
          <div style={subtitleStyle}>{system.rbfPath ?? 'No core file'}</div>
          <div style={msgStyle}>
            No games directory for this system — core-only entry.
          </div>
        </>
      ) : (
        <>
          <div style={subtitleStyle}>
            {entries.loading ? 'Loading…' : `${list.length} entries${crumb}`}
          </div>
          <GameList entries={list} cursor={cursor} top={LIST_TOP} />
        </>
      )}
      <ActionBar actions={LIST_ACTIONS} />
    </>
  );
}

function NotFound({ sys }: { sys: string }) {
  const router = useRouter();
  useZoneIntent(
    'back',
    useCallback(
      (e) => {
        if (e.kind === 'pressed') router.back();
      },
      [router],
    ),
  );
  return <div style={msgStyle}>{`Unknown system '${sys}'`}</div>;
}
