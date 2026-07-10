// Top-right status cluster: settings / wifi / bluetooth / account /
// notifications icons, then the clock and battery — the theme's
// UP-from-carousel target. Purely presentational: the owning zone
// (Home's StatusZone) holds the selection index and passes it in
// with the zone-focused flag; per-item focus renders as a brightened
// cell behind the selected icon.
//
// Wifi/bluetooth/battery states are placeholders until the
// `1fpga:system` host module lands — the layout, focus interaction
// and routing are what this component establishes.

import { memo, useEffect, useState } from 'react';
import type { CSSProperties } from 'react';

import { s } from '../scale';
import { Icon, type IconName } from '../components/Icon';

export interface StatusItem {
  icon: IconName;
  /** Route pushed when the item is confirmed. */
  route: string;
  label: string;
}

/** The focusable items, left to right. The owning zone's selection
 *  index and confirm handler are defined against this array. */
export const STATUS_ITEMS: StatusItem[] = [
  { icon: 'settings', route: '/settings', label: 'Settings' },
  { icon: 'wifi', route: '/settings/network', label: 'Network' },
  { icon: 'bluetooth', route: '/settings/bluetooth', label: 'Bluetooth' },
  { icon: 'account_circle', route: '/settings/account', label: 'Account' },
  { icon: 'notifications', route: '/notifications', label: 'Notifications' },
];

const CELL = s(46);

const rootStyle: CSSProperties = {
  position: 'absolute',
  top: s(18),
  right: s(32),
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: s(10),
  paddingLeft: s(14),
  paddingRight: s(16),
  height: s(56),
  backgroundColor: '#0a1218aa',
};

const cellStyle: CSSProperties = {
  width: CELL,
  height: CELL,
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
};

const dividerStyle: CSSProperties = {
  width: s(2),
  height: s(26),
  marginLeft: s(6),
  marginRight: s(6),
  backgroundColor: '#ffffff2a',
};

const clockStyle: CSSProperties = {
  fontSize: s(24),
  color: '#e8f0f8',
};

function pad2(n: number): string {
  return n < 10 ? `0${n}` : `${n}`;
}

/** Minute-resolution clock, aligned to wall-clock minute flips. */
function useClock(): string {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const msToNextMinute = 60_000 - (Date.now() % 60_000);
    let id: ReturnType<typeof setInterval> | null = null;
    const timeout = setTimeout(() => {
      setNow(new Date());
      id = setInterval(() => setNow(new Date()), 60_000);
    }, msToNextMinute);
    return () => {
      clearTimeout(timeout);
      if (id) clearInterval(id);
    };
  }, []);
  return `${pad2(now.getHours())}:${pad2(now.getMinutes())}`;
}

export const StatusCluster = memo(function StatusCluster({
  focused,
  selected,
}: {
  /** The status focus zone is active. */
  focused: boolean;
  /** Index into STATUS_ITEMS of the highlighted item. */
  selected: number;
}) {
  const time = useClock();
  return (
    <div style={{ ...rootStyle, opacity: focused ? 1 : 0.82 }}>
      {STATUS_ITEMS.map((item, i) => {
        const active = focused && i === selected;
        return (
          <div
            key={item.icon}
            style={{
              ...cellStyle,
              backgroundColor: active ? '#ffffff2e' : '#00000000',
            }}
          >
            <Icon name={item.icon} size={s(26)} color={active ? '#ffffff' : '#9fb4c8'} />
          </div>
        );
      })}
      <div style={dividerStyle} />
      <div style={clockStyle}>{time}</div>
      <Icon name="battery_full" size={s(26)} color="#9fd8a8" />
    </div>
  );
});
