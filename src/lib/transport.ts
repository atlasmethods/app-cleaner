/**
 * One `call()` API for both runtimes:
 *  - Tauri desktop: IPC commands `api_call` / `api_cancel` with a progress Channel.
 *  - Browser: `POST /api/call` returning an NDJSON stream (progress lines, then exactly
 *    one result or error line), authenticated with the per-launch token.
 */
import type { ApiErrorBody, ClientErrorCode, ErrorCode, ProgressEvent, WireMessage } from '../api/types';

export interface CallOptions {
  onProgress?: (e: ProgressEvent) => void;
  signal?: AbortSignal;
}

export const UNREACHABLE_MESSAGE =
  'The ClearSweep background service is not reachable. It may have been closed or stopped. Start it again (run `clearsweep ui` or reopen the app), then retry.';

/** Error thrown by `call()`; `code` mirrors the Rust `ErrorCode`. */
export class ApiCallError extends Error {
  readonly code: ErrorCode | ClientErrorCode;
  constructor(code: ErrorCode | ClientErrorCode, message: string) {
    super(message);
    this.name = 'ApiCallError';
    this.code = code;
  }
  static from(e: unknown): ApiCallError {
    if (e instanceof ApiCallError) return e;
    if (isErrorBody(e)) return new ApiCallError(e.code, e.message);
    if (e instanceof Error) return new ApiCallError('Internal', e.message);
    return new ApiCallError('Internal', typeof e === 'string' ? e : 'Unknown error');
  }
}

function isErrorBody(e: unknown): e is ApiErrorBody {
  return (
    typeof e === 'object' &&
    e !== null &&
    typeof (e as ApiErrorBody).code === 'string' &&
    typeof (e as ApiErrorBody).message === 'string'
  );
}

export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

// ---------------------------------------------------------------- token

const TOKEN_KEY = 'clearsweep.token';
let memoryToken: string | null = null;

/** Read `?t=` from the URL, remember it for this tab, and strip it from the address bar. */
export function initToken(): void {
  if (typeof window === 'undefined') return;
  const url = new URL(window.location.href);
  const t = url.searchParams.get('t');
  if (!t) return;
  memoryToken = t;
  try {
    window.sessionStorage.setItem(TOKEN_KEY, t);
  } catch {
    /* storage unavailable: keep the in-memory copy */
  }
  url.searchParams.delete('t');
  window.history.replaceState(window.history.state, '', url.pathname + url.search + url.hash);
}

export function getToken(): string | null {
  if (memoryToken) return memoryToken;
  try {
    return window.sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

/** Test helper. */
export function _resetTokenForTests(): void {
  memoryToken = null;
}

// ---------------------------------------------------------------- auth state

let authRequired = false;
const authListeners = new Set<() => void>();

/** True once the server rejected our token (missing, stale or from another launch). */
export function isAuthRequired(): boolean {
  return authRequired;
}

export function subscribeAuth(cb: () => void): () => void {
  authListeners.add(cb);
  return () => authListeners.delete(cb);
}

function setAuthRequired(v: boolean): void {
  if (authRequired === v) return;
  authRequired = v;
  authListeners.forEach((l) => l());
}

/** Test helper. */
export function _resetAuthForTests(): void {
  authRequired = false;
  authListeners.clear();
}

// ---------------------------------------------------------------- NDJSON

/**
 * Read an NDJSON byte stream and invoke `onMessage` for every complete line, however the
 * bytes are chunked. A trailing line without `\n` is still delivered at end of stream.
 */
export async function readNdjson(
  body: ReadableStream<Uint8Array>,
  onMessage: (m: WireMessage) => void,
): Promise<void> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  const flushLine = (raw: string) => {
    const line = raw.trim();
    if (!line) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      throw new ApiCallError('Internal', 'Malformed response line from server');
    }
    onMessage(parsed as WireMessage);
  };
  for (;;) {
    let chunk: ReadableStreamReadResult<Uint8Array>;
    try {
      chunk = await reader.read();
    } catch (e) {
      // A dropped connection surfaces as a TypeError ("network error"); an abort stays an abort.
      if (e instanceof TypeError) throw new ApiCallError('Unreachable', UNREACHABLE_MESSAGE);
      throw e;
    }
    const { done, value } = chunk;
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let nl: number;
    while ((nl = buf.indexOf('\n')) >= 0) {
      flushLine(buf.slice(0, nl));
      buf = buf.slice(nl + 1);
    }
  }
  buf += decoder.decode();
  flushLine(buf);
}

/** Drive a call response stream to completion: progress → callback, result → value, error → throw. */
export async function consumeCallStream<T>(
  body: ReadableStream<Uint8Array>,
  onProgress?: (e: ProgressEvent) => void,
): Promise<T> {
  let outcome: { ok: true; value: T } | { ok: false; error: ApiCallError } | null = null;
  await readNdjson(body, (m) => {
    if (outcome) return; // exactly one terminal line is expected; ignore anything after
    switch (m.type) {
      case 'progress': {
        const { type: _type, ...ev } = m;
        onProgress?.(ev);
        break;
      }
      case 'result':
        outcome = { ok: true, value: m.value as T };
        break;
      case 'error':
        outcome = { ok: false, error: new ApiCallError(m.error.code, m.error.message) };
        break;
    }
  });
  // A stream that ends without a result line means the server went away mid-call.
  if (!outcome) throw new ApiCallError('Unreachable', `The connection was lost before the call finished. ${UNREACHABLE_MESSAGE}`);
  const o = outcome as { ok: true; value: T } | { ok: false; error: ApiCallError };
  if (o.ok) return o.value;
  throw o.error;
}

// ---------------------------------------------------------------- call

function newCallId(): string {
  return globalThis.crypto.randomUUID();
}

function abortError(): DOMException {
  return new DOMException('The call was aborted', 'AbortError');
}

export async function call<T = unknown>(
  method: string,
  params: unknown = {},
  opts: CallOptions = {},
): Promise<T> {
  if (opts.signal?.aborted) throw abortError();
  const callId = newCallId();
  return isTauri() ? callTauri<T>(callId, method, params, opts) : callHttp<T>(callId, method, params, opts);
}

async function callTauri<T>(callId: string, method: string, params: unknown, opts: CallOptions): Promise<T> {
  const { invoke, Channel } = await import('@tauri-apps/api/core');
  const channel = new Channel<ProgressEvent>();
  channel.onmessage = (e) => opts.onProgress?.(e);

  let aborted = false;
  const onAbort = () => {
    aborted = true;
    void invoke('api_cancel', { callId }).catch(() => undefined);
  };
  opts.signal?.addEventListener('abort', onAbort, { once: true });
  try {
    return await invoke<T>('api_call', { callId, method, params, onProgress: channel });
  } catch (e) {
    if (aborted) throw abortError();
    throw ApiCallError.from(e);
  } finally {
    opts.signal?.removeEventListener('abort', onAbort);
  }
}

function authHeaders(): Record<string, string> {
  const token = getToken();
  return token ? { 'X-Sweep-Token': token } : {};
}

async function callHttp<T>(callId: string, method: string, params: unknown, opts: CallOptions): Promise<T> {
  const controller = new AbortController();
  const onAbort = () => {
    // Ask the server to stop the job, then drop the connection.
    void fetch('/api/cancel', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', ...authHeaders() },
      body: JSON.stringify({ callId }),
    }).catch(() => undefined);
    controller.abort();
  };
  opts.signal?.addEventListener('abort', onAbort, { once: true });
  try {
    let res: Response;
    try {
      res = await fetch('/api/call', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...authHeaders() },
        body: JSON.stringify({ callId, method, params: params ?? null }),
        signal: controller.signal,
      });
    } catch (e) {
      // fetch() rejects with a TypeError when nothing answers (server stopped, connection refused).
      if (e instanceof TypeError) throw new ApiCallError('Unreachable', UNREACHABLE_MESSAGE);
      throw e;
    }
    if (!res.ok || !res.body) {
      if (res.status === 401) setAuthRequired(true);
      if (res.status === 401 || res.status === 403) {
        throw new ApiCallError(
          'PermissionDenied',
          `The local server rejected the request (${res.status}). Re-open ClearSweep from the link printed by \`clearsweep ui\`.`,
        );
      }
      throw new ApiCallError('Internal', `Server error (HTTP ${res.status})`);
    }
    return await consumeCallStream<T>(res.body, opts.onProgress);
  } catch (e) {
    if (opts.signal?.aborted || (e instanceof DOMException && e.name === 'AbortError')) throw abortError();
    throw ApiCallError.from(e);
  } finally {
    opts.signal?.removeEventListener('abort', onAbort);
  }
}

// ---------------------------------------------------------------- heartbeat

/** Browser mode only: lets the server exit once the tab is closed. Returns a stop function. */
export function startHeartbeat(intervalMs = 5000): () => void {
  if (isTauri()) return () => undefined;
  const beat = () => {
    void fetch('/api/heartbeat', { method: 'POST', headers: authHeaders() })
      .then((r) => {
        if (r.status === 401) setAuthRequired(true);
      })
      .catch(() => undefined);
  };
  beat();
  const id = setInterval(beat, intervalMs);
  // A hidden tab's timers may be throttled to one per minute: beat as soon as it is shown again.
  const onVisible = () => {
    if (document.visibilityState === 'visible') beat();
  };
  document.addEventListener('visibilitychange', onVisible);
  return () => {
    clearInterval(id);
    document.removeEventListener('visibilitychange', onVisible);
  };
}
