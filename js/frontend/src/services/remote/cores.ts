import type * as schemas from '@1fpga/schemas';

import * as net from '1fpga:net';

import { NormalizedCatalog, NormalizedCore } from '@/services/remote/catalog';
import { downloadAndCheck } from '@/services/remote/files';
import { compareVersions } from '@/utils';

export function latestReleaseOf(core: NormalizedCore): schemas.catalog.Release | undefined {
  // If a release has the tag `latest`, use that.
  const latest = core.releases.find(r => r.tags?.includes('latest'));
  if (latest) {
    return latest;
  }

  // Sort by version number, descending, skipping `alpha` or `beta` tags.
  return core.releases
    .filter(x => !(x.tags?.includes('alpha') || x.tags?.includes('beta')))
    .sort((a, b) => -compareVersions(a.version, b.version))[0];
}

export async function download(
  catalog: NormalizedCatalog,
  core: NormalizedCore,
  release: schemas.catalog.Release | undefined = latestReleaseOf(core),
) {
  if (!release) {
    throw new Error(
      `Core ${JSON.stringify(core.uniqueName)} exists but no release could be selected.`,
    );
  }
  if (!release.files.some(x => x.type === 'mister.core.rbf')) {
    throw new Error(`Release contains multiple RBF files.`);
  }

  const root = `/media/fat/1fpga/cores/${catalog.uniqueName}/${core.uniqueName}`;
  let result = null;
  for (const f of release.files) {
    const url = new URL(f.url, core._url ?? catalog.cores?._url ?? catalog._url);
    const p = await downloadAndCheck(url, f, root);
    if (f.type === 'mister.core.rbf') {
      result = p;
    }
  }

  return result;
}
