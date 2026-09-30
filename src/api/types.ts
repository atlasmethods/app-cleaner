/** Shared wire types. Rust structs serialize with serde rename_all = "camelCase". */

export type ErrorCode =
  | 'NotImplemented'
  | 'InvalidParams'
  | 'NotFound'
  | 'Unsupported'
  | 'PermissionDenied'
  | 'Cancelled'
  | 'Io'
  | 'Internal';

/** Raised by the frontend itself, never by the backend. */
export type ClientErrorCode = 'Unreachable';

export interface ApiErrorBody {
  code: ErrorCode;
  message: string;
}

/** Mirrors `sweep_core::ProgressEvent`. */
export interface ProgressEvent {
  stage: string;
  /** 0..1 when known */
  fraction?: number;
  message?: string;
  current?: number;
  total?: number;
}

/** NDJSON lines sent by the HTTP transport. */
export type WireMessage =
  | ({ type: 'progress' } & ProgressEvent)
  | { type: 'result'; value: unknown }
  | { type: 'error'; error: ApiErrorBody };
