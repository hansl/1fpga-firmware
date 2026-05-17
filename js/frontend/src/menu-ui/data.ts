// Mock menu structure for the XMB-style demo. Real menu data will
// come from the firmware's settings / persisted state once those
// services are re-wired; for now this is just a plausible shape so
// the layout, navigation, and animation logic have something to
// exercise.

import type { ActionBinding } from './components/ActionBar';

const ASSETS = '/media/fat/menu_ui_assets';

export interface MenuItem {
  /** Human-readable label, shown in the column and in the bottom
   *  selected-item line. */
  name: string;
  /** Optional path to a per-item icon. When absent, the column
   *  renders just the label. */
  icon?: string;
  /** Subtitle / hint shown smaller under the name. Optional. */
  subtitle?: string;
  /** Per-item overrides for the bottom action bar. When `undefined`
   *  the parent category's `defaultActions` are used. */
  actions?: ActionBinding[];
}

export interface MenuCategory {
  /** Human-readable label. */
  name: string;
  /** Path to the category's icon — shown in the top horizontal
   *  strip. */
  icon: string;
  /** Vertical list of items shown when this category is selected. */
  items: MenuItem[];
  /** Default action set for this category's items. Items can
   *  override per-item via `MenuItem.actions`. */
  defaultActions: ActionBinding[];
}

/** Action set every item in the demo inherits unless overridden. */
const NAV_ACTIONS: ActionBinding[] = [
  { intent: 'navigate_updown', label: 'Move' },
  { intent: 'confirm', label: 'Select' },
  { intent: 'back', label: 'Back' },
];

const GAME_ACTIONS: ActionBinding[] = [
  { intent: 'navigate_updown', label: 'Move' },
  { intent: 'confirm', label: 'Play' },
  { intent: 'face_north', label: 'Favourite' },
  { intent: 'face_west', label: 'Info' },
  { intent: 'back', label: 'Back' },
];

export const CATEGORIES: MenuCategory[] = [
  {
    name: 'Games',
    icon: `${ASSETS}/nes.png`,
    defaultActions: GAME_ACTIONS,
    items: [
      { name: 'Super Mario Bros.', subtitle: 'NES · 1985' },
      { name: 'The Legend of Zelda', subtitle: 'NES · 1986' },
      { name: 'Metroid', subtitle: 'NES · 1986' },
      { name: 'Castlevania', subtitle: 'NES · 1986' },
      { name: 'Mega Man 2', subtitle: 'NES · 1988' },
      { name: 'Tetris', subtitle: 'Game Boy · 1989' },
      { name: 'Sonic the Hedgehog', subtitle: 'Genesis · 1991' },
      { name: 'F-Zero', subtitle: 'SNES · 1990' },
    ],
  },
  {
    name: 'Cores',
    icon: `${ASSETS}/snes.png`,
    defaultActions: NAV_ACTIONS,
    items: [
      { name: 'NES', icon: `${ASSETS}/nes.png`, subtitle: 'Nintendo Entertainment System' },
      { name: 'SNES', icon: `${ASSETS}/snes.png`, subtitle: 'Super Nintendo' },
      { name: 'Genesis', icon: `${ASSETS}/genesis.png`, subtitle: 'Sega Mega Drive' },
      { name: 'Game Boy', icon: `${ASSETS}/gameboy.png`, subtitle: 'Nintendo Game Boy' },
      { name: 'Atari 2600', icon: `${ASSETS}/atari.png`, subtitle: 'Atari VCS' },
    ],
  },
  {
    name: 'Settings',
    icon: `${ASSETS}/genesis.png`,
    defaultActions: NAV_ACTIONS,
    items: [
      { name: 'Display', subtitle: 'HDMI mode, scaling, scanlines' },
      { name: 'Audio', subtitle: 'Volume, mixing, output' },
      { name: 'Input', subtitle: 'Keyboard, gamepad, hotkeys' },
      { name: 'Network', subtitle: 'Wi-Fi, hostname' },
      { name: 'Storage', subtitle: 'SD card, USB' },
      { name: 'About', subtitle: 'Firmware version, credits' },
    ],
  },
  {
    name: 'Tools',
    icon: `${ASSETS}/gameboy.png`,
    defaultActions: NAV_ACTIONS,
    items: [
      { name: 'File Manager' },
      { name: 'BIOS Manager' },
      { name: 'ROM Scanner' },
      { name: 'Save States' },
      { name: 'Cheats' },
    ],
  },
  {
    name: 'Profile',
    icon: `${ASSETS}/atari.png`,
    defaultActions: NAV_ACTIONS,
    items: [
      { name: 'Switch User' },
      { name: 'Achievements' },
      { name: 'Play History' },
      { name: 'Sign Out' },
    ],
  },
];
