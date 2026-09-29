import { useCallback, useEffect, useRef, useState } from 'react';
import type { ProgressEvent } from '../api/types';
import { ApiCallError, call } from '../lib/transport';

export interface CallState<T> {
  data: T | null;
  error: ApiCallError | null;
  loading: boolean;
  progress: ProgressEvent | null;
}

export interface UseCall<T, P> extends CallState<T> {
  /** Start (or restart) the call. Resolves with the result, or undefined on error/cancel. */
  run: (params?: P) => Promise<T | undefined>;
  /** Abort the in-flight call, if any. */
  cancel: () => void;
  reset: () => void;
}

/** Loading / error / progress / cancel state around a single API method. */
export function useCall<T = unknown, P = unknown>(method: string): UseCall<T, P> {
  const [state, setState] = useState<CallState<T>>({ data: null, error: null, loading: false, progress: null });
  const controller = useRef<AbortController | null>(null);

  const cancel = useCallback(() => controller.current?.abort(), []);

  const run = useCallback(
    async (params?: P): Promise<T | undefined> => {
      controller.current?.abort();
      const ac = new AbortController();
      controller.current = ac;
      setState((s) => ({ ...s, loading: true, error: null, progress: null }));
      try {
        const data = await call<T>(method, params ?? {}, {
          signal: ac.signal,
          onProgress: (progress) => {
            if (controller.current === ac) setState((s) => ({ ...s, progress }));
          },
        });
        if (controller.current === ac) setState({ data, error: null, loading: false, progress: null });
        return data;
      } catch (e) {
        if (controller.current === ac) {
          const aborted = e instanceof DOMException && e.name === 'AbortError';
          setState((s) => ({
            ...s,
            loading: false,
            progress: null,
            error: aborted ? null : ApiCallError.from(e),
          }));
        }
        return undefined;
      }
    },
    [method],
  );

  const reset = useCallback(() => {
    controller.current?.abort();
    controller.current = null;
    setState({ data: null, error: null, loading: false, progress: null });
  }, []);

  // Abort on unmount.
  useEffect(
    () => () => {
      controller.current?.abort();
    },
    [],
  );

  return { ...state, run, cancel, reset };
}
