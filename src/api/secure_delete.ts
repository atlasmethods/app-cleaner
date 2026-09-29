/** Mirrors `sweep_core::features::secure_delete` (serde camelCase). */
export const SECURE_DELETE_METHODS = ['secure_delete.delete'] as const;

export type SecureDeleteMethod = (typeof SECURE_DELETE_METHODS)[number];

export interface SecureDeleteParams {
  paths: string[];
  passes?: 1 | 3 | 7 | 35;
}

export interface PathResult {
  path: string;
  ok: boolean;
  error?: string;
  bytes: number;
}

export interface SecureDeleteResult {
  results: PathResult[];
  totalBytes: number;
}
