// Render a Material Icons glyph as a text node with the bundled
// 'icons' font. Material Icons codepoints live in Unicode's Private
// Use Area; we look them up by name so consumer code stays readable.
//
// Codepoints sourced from the Material Icons v145 codepoints file:
//   https://raw.githubusercontent.com/google/material-design-icons/
//     master/font/MaterialIcons-Regular.codepoints

import { memo } from 'react';
import type { CSSProperties } from 'react';

export type IconName =
  // Navigation / arrows
  | 'keyboard_arrow_up'
  | 'keyboard_arrow_down'
  | 'keyboard_arrow_left'
  | 'keyboard_arrow_right'
  | 'unfold_more'
  | 'compare_arrows'
  | 'arrow_back'
  | 'arrow_forward'
  // System / UI
  | 'wifi'
  | 'wifi_off'
  | 'signal_wifi_4_bar'
  | 'person'
  | 'account_circle'
  | 'notifications'
  | 'notifications_active'
  | 'settings'
  | 'menu'
  | 'home'
  | 'search'
  | 'star'
  | 'star_border'
  | 'favorite'
  | 'info'
  | 'check'
  | 'close'
  | 'keyboard'
  | 'keyboard_return'
  | 'check_circle'
  | 'highlight_off'
  // Controllers / gaming
  | 'sports_esports'
  | 'videogame_asset'
  | 'gamepad'
  | 'memory'
  // Files / storage
  | 'sd_card'
  | 'usb'
  | 'folder'
  // Power
  | 'power_settings_new'
  | 'restart_alt';

// Hex codepoints — converted to single-char strings via
// String.fromCodePoint at module init. Keeping them as hex numbers
// in source keeps the file legible (the actual PUA glyphs render as
// boxes / invisible in most editors).
const HEX: Record<IconName, number> = {
  keyboard_arrow_up:    0xe316,
  keyboard_arrow_down:  0xe313,
  keyboard_arrow_left:  0xe314,
  keyboard_arrow_right: 0xe315,
  unfold_more:          0xe5d7,
  compare_arrows:       0xe915,
  arrow_back:           0xe5c4,
  arrow_forward:        0xe5c8,
  wifi:                 0xe63e,
  wifi_off:             0xe648,
  signal_wifi_4_bar:    0xe1d8,
  person:               0xe7fd,
  account_circle:       0xe853,
  notifications:        0xe7f4,
  notifications_active: 0xe7f7,
  settings:             0xe8b8,
  menu:                 0xe5d2,
  home:                 0xe88a,
  search:               0xe8b6,
  star:                 0xe838,
  star_border:          0xe83a,
  favorite:             0xe87d,
  info:                 0xe88e,
  check:                0xe5ca,
  close:                0xe5cd,
  keyboard:             0xe312,
  keyboard_return:      0xe31b,
  check_circle:         0xe86c,
  highlight_off:        0xe888,
  sports_esports:       0xea28,
  videogame_asset:      0xe338,
  gamepad:              0xe30f,
  memory:               0xe322,
  sd_card:              0xe623,
  usb:                  0xe1e0,
  folder:               0xe2c7,
  power_settings_new:   0xe8ac,
  restart_alt:          0xf053,
};

const CODEPOINTS: Record<IconName, string> = Object.fromEntries(
  Object.entries(HEX).map(([k, v]) => [k, String.fromCodePoint(v)]),
) as Record<IconName, string>;

/** Lookup helper — returns the single-character string for `name`. */
export function iconCodepoint(name: IconName): string {
  return CODEPOINTS[name];
}

interface IconProps {
  name: IconName;
  size?: number;
  color?: string;
  /** Inline style extras (margin, alignment). `fontFamily`,
   *  `fontSize`, `color`, and `lineHeight` are managed by the
   *  component. */
  style?: CSSProperties;
}

/**
 * Memoised so an Icon re-renders only when its actual props change.
 * Without this, every parent re-render (e.g. on navigation) would
 * emit a fresh `style` object identity and trigger commitUpdate +
 * damage even though the rendered pixels are identical.
 */
export const Icon = memo(function Icon({
  name,
  size = 24,
  color = '#ffffff',
  style,
}: IconProps) {
  return (
    <div
      style={{
        ...style,
        fontFamily: 'icons',
        fontSize: size,
        color,
        height: size,
        lineHeight: size,
      }}
    >
      {CODEPOINTS[name]}
    </div>
  );
});
