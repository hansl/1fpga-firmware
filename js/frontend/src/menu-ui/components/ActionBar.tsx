// Bottom action hints. Maps a list of (intent, label) pairs to the
// active input device's glyphs so the same UI reads as "Ⓐ Select"
// on a gamepad and "↵ Select" on a keyboard, switching live when
// the user touches a different device.

import type { CSSProperties } from 'react';

import { useInputSource } from '../hooks';

/** A single hint shown in the bar. `intent` is the semantic action
 *  (`confirm`, `back`, `face_north`, …); the component picks the
 *  glyph based on the current input source. */
export interface ActionBinding {
  intent: string;
  label: string;
}

const rootStyle: CSSProperties = {
  position: 'absolute',
  left: 0,
  right: 0,
  bottom: 0,
  height: 56,
  paddingLeft: 48,
  paddingRight: 48,
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: 32,
  backgroundColor: '#0008181c', // 'subtle dark strip — close to transparent.
};

const itemStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: 10,
};

const glyphStyle: CSSProperties = {
  fontSize: 22,
  color: '#ffd060',
};

const labelStyle: CSSProperties = {
  fontSize: 20,
  color: '#d0d8e0',
};

/** Resolve `intent` to the printable glyph for `source`. ASCII-only
 *  for now because the bundled font is a heavily subsetted
 *  NotoSans-Regular (Latin only — no arrow / box / symbol blocks).
 *  Will switch to PNG button artwork once we ship a sprite set,
 *  which sidesteps the font issue entirely and gives proper
 *  Xbox-style "Ⓐ" / "Ⓑ" / shoulder-button glyphs. */
export function glyphFor(intent: string, source: 'keyboard' | 'gamepad' | 'mouse'): string {
  if (source === 'keyboard') {
    switch (intent) {
      case 'confirm': return 'Enter';
      case 'back': return 'Esc';
      case 'menu': return 'F1';
      case 'navigate_up': return 'Up';
      case 'navigate_down': return 'Dn';
      case 'navigate_left': return 'Lt';
      case 'navigate_right': return 'Rt';
      case 'navigate_updown': return 'Up/Dn';
      case 'navigate_leftright': return 'Lt/Rt';
      case 'face_south': return 'Z';
      case 'face_east': return 'X';
      case 'face_west': return 'A';
      case 'face_north': return 'S';
      case 'shoulder_l1': return 'Q';
      case 'shoulder_r1': return 'W';
      case 'shoulder_l2': return '1';
      case 'shoulder_r2': return '2';
      case 'start': return 'F11';
      case 'select': return 'F12';
      case 'tab': return 'Tab';
      default: return '';
    }
  } else if (source === 'gamepad') {
    switch (intent) {
      // Compass labels — neutral across Xbox / Nintendo layouts.
      // Real button artwork comes later; the parenthesised single
      // letters read cleanly in small UI labels.
      case 'confirm': return '(A)';
      case 'back': return '(B)';
      case 'menu': return '(Mode)';
      case 'navigate_up': return 'D-Up';
      case 'navigate_down': return 'D-Dn';
      case 'navigate_left': return 'D-Lt';
      case 'navigate_right': return 'D-Rt';
      case 'navigate_updown': return 'D-Pad';
      case 'navigate_leftright': return 'D-Pad';
      case 'face_south': return '(A)';
      case 'face_east': return '(B)';
      case 'face_west': return '(X)';
      case 'face_north': return '(Y)';
      case 'shoulder_l1': return 'L1';
      case 'shoulder_r1': return 'R1';
      case 'shoulder_l2': return 'L2';
      case 'shoulder_r2': return 'R2';
      case 'start': return 'Start';
      case 'select': return 'Select';
      default: return '';
    }
  } else {
    return intent === 'confirm' ? 'Click' : '';
  }
}

export function ActionBar({ actions }: { actions: ActionBinding[] }) {
  const source = useInputSource();
  return (
    <div style={rootStyle}>
      {actions.map((a) => (
        <div key={a.intent} style={itemStyle}>
          <div style={glyphStyle}>{glyphFor(a.intent, source)}</div>
          <div style={labelStyle}>{a.label}</div>
        </div>
      ))}
    </div>
  );
}
