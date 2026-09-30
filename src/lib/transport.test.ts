import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiCallError, _resetTokenForTests, call, consumeCallStream, getToken, initToken, readNdjson } from './transport';
import type { ProgressEvent, WireMessage } from '../api/types';

const enc = new TextEncoder();

function streamOf(chunks: (string | Uint8Array)[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(c) {
      for (const ch of chunks) c.enqueue(typeof ch === 'string' ? enc.encode(ch) : ch);
      c.close();
    },
  });
}

const progress = '{"type":"progress","stage":"scan","fraction":0.5,"current":1,"total":2}\n';
const result = '{"type":"result","value":{"ok":true}}\n';

describe('readNdjson', () => {
  it('delivers each complete line', async () => {
    const got: WireMessage[] = [];
    await readNdjson(streamOf([progress + result]), (m) => got.push(m));
    expect(got.map((m) => m.type)).toEqual(['progress', 'result']);
  });

  it('handles lines split across chunk boundaries at every offset', async () => {
    const full = progress + result;
    for (let i = 1; i < full.length; i++) {
      const got: WireMessage[] = [];
      await readNdjson(streamOf([full.slice(0, i), full.slice(i)]), (m) => got.push(m));
      expect(got.map((m) => m.type)).toEqual(['progress', 'result']);
    }
  });

  it('handles multi-byte characters split between chunks', async () => {
    const line = enc.encode('{"type":"result","value":"héllo €"}\n');
    const got: WireMessage[] = [];
    for (let i = 1; i < line.length; i++) {
      got.length = 0;
      await readNdjson(streamOf([line.slice(0, i), line.slice(i)]), (m) => got.push(m));
      expect(got).toEqual([{ type: 'result', value: 'héllo €' }]);
    }
  });

  it('delivers a final line that has no trailing newline', async () => {
    const got: WireMessage[] = [];
    await readNdjson(streamOf(['{"type":"result","value":1}']), (m) => got.push(m));
    expect(got).toEqual([{ type: 'result', value: 1 }]);
  });

  it('rejects on malformed JSON', async () => {
    await expect(readNdjson(streamOf(['not json\n']), () => undefined)).rejects.toBeInstanceOf(ApiCallError);
  });
});

describe('consumeCallStream', () => {
  it('reports progress (without the type tag) then resolves with the result value', async () => {
    const events: ProgressEvent[] = [];
    const v = await consumeCallStream<{ ok: boolean }>(streamOf([progress, result]), (e) => events.push(e));
    expect(v).toEqual({ ok: true });
    expect(events).toEqual([{ stage: 'scan', fraction: 0.5, current: 1, total: 2 }]);
  });

  it('throws ApiCallError with the server code on an error line', async () => {
    const err = await consumeCallStream(
      streamOf(['{"type":"error","error":{"code":"NotImplemented","message":"soon"}}\n']),
    ).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiCallError);
    expect((err as ApiCallError).code).toBe('NotImplemented');
    expect((err as ApiCallError).message).toBe('soon');
  });

  it('fails when the stream ends without a terminal line', async () => {
    await expect(consumeCallStream(streamOf([progress]))).rejects.toMatchObject({ code: 'Unreachable' });
  });
});

describe('call over HTTP', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    sessionStorage.clear();
    _resetTokenForTests();
  });

  it('posts callId/method/params with the token and parses the stream', async () => {
    sessionStorage.setItem('clearsweep.token', 'tok123');
    const fetchMock = vi.fn(async () => new Response(streamOf([progress, result])));
    vi.stubGlobal('fetch', fetchMock);
    const events: ProgressEvent[] = [];
    const v = await call('sysinfo.get', { a: 1 }, { onProgress: (e) => events.push(e) });
    expect(v).toEqual({ ok: true });
    expect(events).toHaveLength(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/call');
    expect((init.headers as Record<string, string>)['X-Sweep-Token']).toBe('tok123');
    const body = JSON.parse(init.body as string) as Record<string, unknown>;
    expect(body.method).toBe('sysinfo.get');
    expect(body.params).toEqual({ a: 1 });
    expect(typeof body.callId).toBe('string');
  });

  it('maps 401 to PermissionDenied', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response('unauthorized', { status: 401 })));
    await expect(call('x.y')).rejects.toMatchObject({ code: 'PermissionDenied' });
  });

  it('abort sends /api/cancel and rejects with AbortError', async () => {
    const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
      if (url === '/api/call') {
        return new Promise<Response>((_, reject) => {
          init?.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
        });
      }
      return new Response('{}');
    });
    vi.stubGlobal('fetch', fetchMock);
    const ac = new AbortController();
    const p = call('api.sleep', { ms: 5000 }, { signal: ac.signal });
    await Promise.resolve();
    ac.abort();
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
    const cancelCall = fetchMock.mock.calls.find((c) => c[0] === '/api/cancel');
    expect(cancelCall).toBeTruthy();
  });
});

describe('token handling', () => {
  afterEach(() => {
    sessionStorage.clear();
    _resetTokenForTests();
    window.history.replaceState(null, '', '/');
  });

  it('stores ?t= in sessionStorage and strips it from the URL', () => {
    window.history.replaceState(null, '', '/?t=abc&x=1#/tools');
    initToken();
    expect(getToken()).toBe('abc');
    expect(sessionStorage.getItem('clearsweep.token')).toBe('abc');
    expect(window.location.search).toBe('?x=1');
    expect(window.location.hash).toBe('#/tools');
  });
});
