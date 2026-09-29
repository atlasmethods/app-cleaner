import { vi } from 'vitest';

export interface RecordedCall {
  method: string;
  params: unknown;
}

export interface CallOpts {
  signal?: AbortSignal;
  onProgress?: (e: { stage: string; fraction?: number; message?: string }) => void;
}

type Handler = (params: unknown, opts: CallOpts) => unknown | Promise<unknown>;

/** Shared state for `vi.mock('../lib/transport')`; create with `vi.hoisted`. */
export interface ApiMock {
  calls: RecordedCall[];
  handlers: Record<string, Handler>;
  call: (method: string, params?: unknown, opts?: CallOpts) => Promise<unknown>;
  reset: () => void;
  /** Recorded params of every call to `method`. */
  paramsOf: (method: string) => unknown[];
}

export function createApiMock(): ApiMock {
  const mock: ApiMock = {
    calls: [],
    handlers: {},
    call: async (method, params, opts = {}) => {
      mock.calls.push({ method, params });
      const h = mock.handlers[method];
      if (!h) throw new Error(`unexpected API call in test: ${method}`);
      return h(params, opts);
    },
    reset: () => {
      mock.calls.length = 0;
      mock.handlers = {};
    },
    paramsOf: (method) => mock.calls.filter((c) => c.method === method).map((c) => c.params),
  };
  return mock;
}

/** A promise that never settles until aborted (for cancel tests). */
export function pendingUntilAborted(opts: CallOpts): Promise<never> {
  return new Promise((_, reject) => {
    opts.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
  });
}

export const noop = vi.fn();
