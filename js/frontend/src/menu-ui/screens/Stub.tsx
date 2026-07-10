// Placeholder screen for routes whose feature hasn't been ported
// yet (settings pages, notifications). Proves the route exists and
// is deep-linkable; back pops home.

import { useCallback } from 'react';
import type { CSSProperties } from 'react';

import { VW, VH, s } from '../scale';
import { ActionBar } from '../components/ActionBar';
import { FocusZone, useZoneIntent } from '../focus';
import { useRouter } from '../router';

const root: CSSProperties = {
  position: 'relative',
  width: VW,
  height: VH,
};

const titleStyle: CSSProperties = {
  position: 'absolute',
  left: 0,
  right: 0,
  top: Math.round(VH * 0.4),
  textAlign: 'center',
  fontSize: s(44),
  color: '#ffffff',
};

const subStyle: CSSProperties = {
  position: 'absolute',
  left: 0,
  right: 0,
  top: Math.round(VH * 0.4) + s(72),
  textAlign: 'center',
  fontSize: s(24),
  color: '#7e8ea0',
};

export function StubScreen({ title }: { title: string }) {
  return (
    <div style={root}>
      <FocusZone id="stub">
        <StubBody title={title} />
      </FocusZone>
    </div>
  );
}

function StubBody({ title }: { title: string }) {
  const router = useRouter();
  const goBack = useCallback(
    (e: { kind: string }) => {
      if (e.kind === 'pressed') router.back();
    },
    [router],
  );
  useZoneIntent('back', goBack);
  useZoneIntent('confirm', goBack);
  return (
    <>
      <div style={titleStyle}>{title}</div>
      <div style={subStyle}>Coming soon</div>
      <ActionBar actions={[{ intent: 'back', label: 'Back' }]} />
    </>
  );
}
