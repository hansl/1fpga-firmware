// Boot-time configuration, loaded from the settings data table.
//
// The router's initial route comes from GlobalStorage (the same
// key/value JSON table the schemas define) so the device can boot
// straight into any screen — set `ui.startupRoute` to e.g.
// '"/collections/favorites"' and the menu opens there. Absent key,
// absent table (fresh SD card, migrations not yet run) or malformed
// value all fall back to '/home'.

import * as db from '1fpga:db';

export const STARTUP_ROUTE_KEY = 'ui.startupRoute';

export async function resolveStartupRoute(): Promise<string | null> {
  try {
    const d = await db.load('1fpga');
    const row = await d.queryOne<{ value: string }>(
      'SELECT value FROM GlobalStorage WHERE key = ?',
      [STARTUP_ROUTE_KEY],
    );
    if (!row) return null;
    // `value` is a JSON column; a route is stored as a JSON string.
    const parsed: unknown = JSON.parse(row.value);
    if (typeof parsed === 'string' && parsed.startsWith('/')) {
      return parsed;
    }
    console.warn(`ignoring malformed ${STARTUP_ROUTE_KEY}: ${row.value}`);
    return null;
  } catch (e) {
    // Fresh install or missing migrations — not an error.
    console.warn(`startup route lookup skipped: ${e}`);
    return null;
  }
}
