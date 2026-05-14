import { memo, useCallback, useState } from 'react';
import type { CSSProperties } from 'react';

import { useIntent } from './hooks';

// N9 demo — a horizontal "core picker": a row of cards with a focus
// highlight that animates between cards via react-spring as the user
// presses ← / →. Pressing Enter pulses the focused card. Bottom line
// echoes the focused card's name. Top-left corner shows the live
// frame rate (diagnostic).

interface Item {
  name: string;
  accent: string;
}

const ITEMS: Item[] = [
  { name: 'NES', accent: '#d04040' },
  { name: 'SNES', accent: '#6070ff' },
  { name: 'Genesis', accent: '#3a3a3a' },
  { name: 'Game Boy', accent: '#80a040' },
  { name: 'Atari', accent: '#d09040' },
];

// Menu-ui renders into a 768×576 canvas RT centred on screen with a
// dark-navy letterbox around it. Sized this way to stay below the
// compositor's "large textured layer flickers even when host is
// idle" threshold (bug #1 from the late-night debugging session;
// 1024×720 triggers it, 64×64 doesn't, menu-text-demo's 760×620
// was stable). Until the underlying RTL contention bug is fixed,
// the menu lives inside this letterboxed canvas.
const root: CSSProperties = {
  position: 'relative',
  display: 'flex',
  flexDirection: 'column',
  width: 768,
  height: 576,
  justifyContent: 'space-between',
  alignItems: 'center',
  paddingTop: 48,
  paddingBottom: 48,
  backgroundColor: '#0a0a14',
};

// Canvas-sized wallpaper PNG. Absolute-positioned so it sits behind
// every other root child in DOM order. The fallback solid
// backgroundColor on the root shows through if the file is missing.
const bgStyle: CSSProperties = {
  position: 'absolute',
  top: 0,
  left: 0,
  width: 768,
  height: 576,
};

const headerWrap: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'center',
  gap: 16,
};

const titleStyle: CSSProperties = {
  fontSize: 72,
  color: '#ffffff',
};

const subtitleStyle: CSSProperties = {
  fontSize: 28,
  color: '#80909a',
};

const listStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: 32,
};

const cardBase: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  width: 130,
  height: 190,
  alignItems: 'center',
  justifyContent: 'flex-end',
  paddingBottom: 20,
};

const cardLabelStyle: CSSProperties = {
  fontSize: 32,
  color: '#ffffff',
};

const footerWrap: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'center',
  gap: 12,
};

const selectedNameStyle: CSSProperties = {
  fontSize: 48,
  color: '#ffd060',
};

const hintStyle: CSSProperties = {
  fontSize: 24,
  color: '#506070',
};

const fpsStyle: CSSProperties = {
  position: 'absolute',
  top: 12,
  left: 12,
  fontSize: 20,
  color: '#60ff60',
};

// memo() — focus change only re-renders the two cards whose
// `focused` flipped. Earlier revision used react-spring here; that
// added ~30-45ms of React commit cost per nav (hooks + observer
// setup) which dominated the frame budget when the user mashed
// arrows. Static style toggle now: visual transition is barely
// perceptible at our ~17fps paint rate anyway, so removing the
// tween costs nothing visible and recovers the frame.
const Card = memo(function Card({
  item,
  focused,
  flashing,
}: {
  item: Item;
  focused: boolean;
  flashing: boolean;
}) {
  const bg = flashing ? '#ffffff' : focused ? item.accent : '#1a1a2a';
  return (
    <div style={{ ...cardBase, backgroundColor: bg }}>
      <div style={cardLabelStyle}>{item.name}</div>
    </div>
  );
});

export function App() {
  const [focus, setFocus] = useState(0);
  // -1 = nothing flashing. Set to focused index on confirm,
  // back to -1 after one paint via setTimeout(0).
  const [flashIdx, setFlashIdx] = useState(-1);
  // fps display intentionally disabled. The setInterval(250ms) that
  // used to update it was triggering per-frame repaints, and host
  // blit contention with the compositor's per-scanline tex_unit
  // reads produces column flicker on the textured layer. Without
  // the periodic state change, the React tree is stable, scene_hash
  // matches each loop iteration, and the runtime skips paint
  // entirely → no blit → no contention. Re-enable once the RTL
  // contention bug is fixed.

  useIntent(
    'navigate_left',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setFocus((f) => Math.max(0, f - 1));
      }
    }, []),
  );
  useIntent(
    'navigate_right',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setFocus((f) => Math.min(ITEMS.length - 1, f + 1));
      }
    }, []),
  );
  useIntent(
    'confirm',
    useCallback((e) => {
      if (e.kind !== 'pressed') return;
      // Flash the currently-focused card white for ~120ms, then
      // restore. setFocus uses functional update so we read the
      // latest value without depending on it (avoids useCallback
      // dep churn that resubscribes the listener every focus change).
      setFocus((f) => {
        setFlashIdx(f);
        setTimeout(() => setFlashIdx(-1), 120);
        return f;
      });
    }, []),
  );

  return (
    <div style={root}>
      <img src="/media/fat/menu_ui_bg.png" style={bgStyle} />
      <div style={headerWrap}>
        <div style={titleStyle}>menu-ui · N9 demo</div>
        <div style={subtitleStyle}>core picker</div>
      </div>
      <div style={listStyle}>
        {ITEMS.map((item, i) => (
          <Card
            key={item.name}
            item={item}
            focused={i === focus}
            flashing={i === flashIdx}
          />
        ))}
      </div>
      <div style={footerWrap}>
        <div style={selectedNameStyle}>{ITEMS[focus].name}</div>
        <div style={hintStyle}>{'← / → navigate   ·   Enter confirm'}</div>
      </div>
    </div>
  );
}
