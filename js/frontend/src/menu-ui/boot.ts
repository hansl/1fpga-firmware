// Boot-time configuration, loaded from the settings data table.
//
// The router's initial route comes from GlobalStorage (the same
// key/value JSON table the schemas define) so the device can boot
// straight into any screen — set `ui.startupRoute` to e.g.
// '"/collections/favorites"' and the menu opens there. Absent key,
// absent table (fresh SD card, migrations not yet run) or malformed
// value all fall back to '/home'.

import { globalGet } from './services/db';

export const STARTUP_ROUTE_KEY = 'ui.startupRoute';

export async function resolveStartupRoute(): Promise<string | null> {
  try {
    // Through getDb() so the schema is applied before the first
    // query — a fresh SD card boots clean instead of warning.
    const value = await globalGet<unknown>(STARTUP_ROUTE_KEY);
    if (value == null) return null;
    if (typeof value === 'string' && value.startsWith('/')) {
      return value;
    }
    console.warn(`ignoring malformed ${STARTUP_ROUTE_KEY}: ${JSON.stringify(value)}`);
    return null;
  } catch (e) {
    // Unreadable database — boot the default route rather than hang.
    console.warn(`startup route lookup skipped: ${e}`);
    return null;
  }
}
