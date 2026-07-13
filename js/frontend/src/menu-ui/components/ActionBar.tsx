// Bottom action hints. Maps a list of (intent, label) pairs to the
// active input device's glyph + label so the same UI reads as
// "Ⓐ Select" on a gamepad and "↵ Select" on a keyboard, switching
// live when the user touches a different device.

import { memo } from 'react';
import type { CSSProperties } from 'react';

import { useInputSource } from '../hooks';
import { s } from '../scale';
import { Icon, type IconName } from './Icon';

/** A single hint shown in the bar. `intent` is the semantic action
 *  (`confirm`, `back`, `face_north`, …); the bar picks the glyph
 *  based on the current input source. */
export interface ActionBinding {
  intent: string;
  label: string;
}

/** A glyph is either a Material Icons name (rendered through the
 *  icon font) or a literal text label (rendered with the default
 *  font). Different intents pick different forms — keyboard arrow
 *  glyphs are icons, but gamepad button labels like "(A)" stay as
 *  text until we ship gamepad-button artwork. */
type GlyphSpec =
  | { kind: 'icon'; name: IconName }
  | { kind: 'text'; text: string };

const rootStyle: CSSProperties = {
  position: 'absolute',
  left: 0,
  right: 0,
  bottom: 0,
  height: s(56),
  paddingLeft: s(48),
  paddingRight: s(48),
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: s(32),
  backgroundColor: '#0008181c',
};

const itemStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: s(10),
};

const textGlyphStyle: CSSProperties = {
  fontSize: s(22),
  color: '#ffd060',
};

const labelStyle: CSSProperties = {
  fontSize: s(20),
  color: '#d0d8e0',
};

/** Resolve `intent` × source to the glyph that should appear before
 *  the action's label. Keyboard arrows / Enter / Esc are real icons;
 *  letter-key fallbacks (Z, X, Q…) and gamepad button labels stay
 *  as text. Future work: ship a gamepad-button sprite set and
 *  switch the gamepad cases to icons too. */
export function glyphFor(
  intent: string,
  source: 'keyboard' | 'gamepad' | 'mouse',
): GlyphSpec | null {
  if (source === 'keyboard') {
    switch (intent) {
      case 'confirm': return { kind: 'icon', name: 'keyboard_return' };
      case 'back': return { kind: 'text', text: 'Esc' };
      case 'menu': return { kind: 'text', text: 'F1' };
      case 'navigate_up': return { kind: 'icon', name: 'keyboard_arrow_up' };
      case 'navigate_down': return { kind: 'icon', name: 'keyboard_arrow_down' };
      case 'navigate_left': return { kind: 'icon', name: 'keyboard_arrow_left' };
      case 'navigate_right': return { kind: 'icon', name: 'keyboard_arrow_right' };
      case 'navigate_updown': return { kind: 'icon', name: 'unfold_more' };
      case 'navigate_leftright': return { kind: 'icon', name: 'compare_arrows' };
      case 'face_south': return { kind: 'text', text: 'Z' };
      case 'face_east': return { kind: 'text', text: 'X' };
      case 'face_west': return { kind: 'text', text: 'A' };
      case 'face_north': return { kind: 'text', text: 'S' };
      case 'shoulder_l1': return { kind: 'text', text: 'Q' };
      case 'shoulder_r1': return { kind: 'text', text: 'W' };
      case 'shoulder_l2': return { kind: 'text', text: '1' };
      case 'shoulder_r2': return { kind: 'text', text: '2' };
      case 'start': return { kind: 'text', text: 'F11' };
      case 'select': return { kind: 'text', text: 'F12' };
      case 'tab': return { kind: 'text', text: 'Tab' };
      default: return null;
    }
  } else if (source === 'gamepad') {
    switch (intent) {
      case 'confirm': return { kind: 'text', text: '(A)' };
      case 'back': return { kind: 'text', text: '(B)' };
      case 'menu': return { kind: 'icon', name: 'menu' };
      case 'navigate_up': return { kind: 'icon', name: 'keyboard_arrow_up' };
      case 'navigate_down': return { kind: 'icon', name: 'keyboard_arrow_down' };
      case 'navigate_left': return { kind: 'icon', name: 'keyboard_arrow_left' };
      case 'navigate_right': return { kind: 'icon', name: 'keyboard_arrow_right' };
      case 'navigate_updown': return { kind: 'icon', name: 'unfold_more' };
      case 'navigate_leftright': return { kind: 'icon', name: 'compare_arrows' };
      case 'face_south': return { kind: 'text', text: '(A)' };
      case 'face_east': return { kind: 'text', text: '(B)' };
      case 'face_west': return { kind: 'text', text: '(X)' };
      case 'face_north': return { kind: 'text', text: '(Y)' };
      case 'shoulder_l1': return { kind: 'text', text: 'L1' };
      case 'shoulder_r1': return { kind: 'text', text: 'R1' };
      case 'shoulder_l2': return { kind: 'text', text: 'L2' };
      case 'shoulder_r2': return { kind: 'text', text: 'R2' };
      case 'start': return { kind: 'text', text: 'Start' };
      case 'select': return { kind: 'text', text: 'Select' };
      default: return null;
    }
  } else {
    return intent === 'confirm' ? { kind: 'text', text: 'Click' } : null;
  }
}

function Glyph({ spec }: { spec: GlyphSpec }) {
  if (spec.kind === 'icon') {
    return <Icon name={spec.name} size={s(22)} color="#ffd060" />;
  }
  return <div style={textGlyphStyle}>{spec.text}</div>;
}

export const ActionBar = memo(function ActionBar({
  actions,
}: {
  actions: ActionBinding[];
}) {
  const source = useInputSource();
  return (
    <div style={rootStyle}>
      {actions.map((a) => {
        const spec = glyphFor(a.intent, source);
        return (
          <div key={a.intent} style={itemStyle}>
            {spec ? <Glyph spec={spec} /> : null}
            <div style={labelStyle}>{a.label}</div>
          </div>
        );
      })}
    </div>
  );
});
