// Route stack for the frontend.
//
// Paths are plain strings ('/collections/nes'); patterns support
// `:param` segments. The stack gives natural back-button semantics,
// and the INITIAL route is resolved asynchronously — from the
// GlobalStorage settings table (see boot.ts), a future `--route`
// boot flag, or the '/home' fallback — so the device can boot
// straight into any screen (e.g. a favorites list).

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from 'react';
import type { ReactNode } from 'react';

export interface RouterApi {
  /** Current route path (top of the stack). */
  path: string;
  /** Push a new route. */
  navigate(path: string): void;
  /** Replace the current route (no back entry). */
  replace(path: string): void;
  /** Pop one route; false if already at the stack bottom. */
  back(): boolean;
  /** Full stack, bottom → top (diagnostics / breadcrumbs). */
  stack: readonly string[];
}

const RouterContext = createContext<RouterApi | null>(null);

/**
 * Match `pattern` ('/collections/:id') against `path`
 * ('/collections/nes'). Returns extracted params or null.
 */
export function matchRoute(
  pattern: string,
  path: string,
): Record<string, string> | null {
  const ps = pattern.split('/').filter((s) => s.length > 0);
  const xs = path.split('/').filter((s) => s.length > 0);
  if (ps.length !== xs.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < ps.length; i++) {
    if (ps[i].startsWith(':')) {
      params[ps[i].slice(1)] = decodeURIComponent(xs[i]);
    } else if (ps[i] !== xs[i]) {
      return null;
    }
  }
  return params;
}

export function RouterProvider({
  resolveInitial,
  fallback = '/home',
  children,
}: {
  /**
   * Async initial-route source (settings table, boot flag). Errors
   * and null both fall back to `fallback`.
   */
  resolveInitial?: () => Promise<string | null>;
  fallback?: string;
  children: ReactNode;
}) {
  const [stack, setStack] = useState<string[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    (resolveInitial?.() ?? Promise.resolve(null))
      .catch((e) => {
        console.warn(`initial route resolution failed: ${e}`);
        return null;
      })
      .then((initial) => {
        if (!cancelled) setStack([initial ?? fallback]);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const navigate = useCallback((path: string) => {
    setStack((s) => (s ? [...s, path] : s));
  }, []);
  const replace = useCallback((path: string) => {
    setStack((s) => (s && s.length > 0 ? [...s.slice(0, -1), path] : s));
  }, []);
  const back = useCallback((): boolean => {
    let popped = false;
    setStack((s) => {
      if (s && s.length > 1) {
        popped = true;
        return s.slice(0, -1);
      }
      return s;
    });
    return popped;
  }, []);

  const api = useMemo<RouterApi | null>(() => {
    if (!stack) return null;
    return {
      path: stack[stack.length - 1],
      navigate,
      replace,
      back,
      stack,
    };
  }, [stack, navigate, replace, back]);

  // Nothing renders until the initial route resolves — the DB query
  // takes one tick; the wallpaper layer keeps the screen non-blank.
  if (!api) return null;
  return <RouterContext.Provider value={api}>{children}</RouterContext.Provider>;
}

export function useRouter(): RouterApi {
  const api = useContext(RouterContext);
  if (!api) throw new Error('useRouter outside <RouterProvider>');
  return api;
}

/** Params if the current route matches `pattern`, else null. */
export function useRoute(pattern: string): Record<string, string> | null {
  const { path } = useRouter();
  return useMemo(() => matchRoute(pattern, path), [pattern, path]);
}

export interface RouteDef {
  pattern: string;
  render: (params: Record<string, string>) => ReactNode;
}

/** First-match route table. */
export function Routes({ routes }: { routes: RouteDef[] }) {
  const { path } = useRouter();
  for (const r of routes) {
    const params = matchRoute(r.pattern, path);
    if (params) return <>{r.render(params)}</>;
  }
  console.warn(`no route matches '${path}'`);
  return null;
}
