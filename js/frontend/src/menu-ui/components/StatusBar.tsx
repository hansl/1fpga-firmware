// Top-right status strip: network status, signed-in user, any
// pending notifications, and the current time. Everything in here
// is mocked for now; once the firmware exposes the matching
// services through `1fpga:` modules, swap the inline literals for
// service hooks.

import { useEffect, useState } from 'react';
import type { CSSProperties } from 'react';

const rootStyle: CSSProperties = {
  position: 'absolute',
  top: 16,
  right: 24,
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: 24,
};

const cellStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: 8,
};

const glyphStyle: CSSProperties = {
  fontSize: 22,
  color: '#80c0ff',
};

const labelStyle: CSSProperties = {
  fontSize: 20,
  color: '#d0d8e0',
};

const labelMutedStyle: CSSProperties = {
  ...labelStyle,
  color: '#80909a',
};

function pad2(n: number): string {
  return n < 10 ? `0${n}` : `${n}`;
}

/** Tick the clock once per minute. Avoids a per-second invalidation
 *  that would damage-paint the status bar 60 times/minute. */
function useClock(): string {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    // Align the first interval to the next minute boundary so
    // the displayed minute flips when the wall clock does.
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

export function StatusBar({
  user = 'guest',
  notifications = 0,
  wifi = 'connected',
}: {
  user?: string;
  notifications?: number;
  wifi?: 'connected' | 'disconnected' | 'unknown';
} = {}) {
  const time = useClock();
  const wifiLabel =
    wifi === 'connected' ? 'WiFi' : wifi === 'disconnected' ? 'No net' : 'WiFi?';
  return (
    <div style={rootStyle}>
      <div style={cellStyle}>
        <div style={glyphStyle}>{wifiLabel}</div>
      </div>
      <div style={cellStyle}>
        <div style={labelStyle}>{user}</div>
      </div>
      {notifications > 0 ? (
        <div style={cellStyle}>
          <div style={{ ...glyphStyle, color: '#ffd060' }}>!</div>
          <div style={labelStyle}>{notifications}</div>
        </div>
      ) : null}
      <div style={cellStyle}>
        <div style={labelMutedStyle}>{time}</div>
      </div>
    </div>
  );
}
