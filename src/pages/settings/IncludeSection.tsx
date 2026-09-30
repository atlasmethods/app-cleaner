import { useState } from 'react';
import { FolderPlus, X } from 'lucide-react';
import { Card } from '../../components/Card';
import { Checkbox } from '../../components/Checkbox';
import { t } from '../../i18n';
import type { SectionProps } from './types';

const inputClass = 'h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-2 text-sm';

export function IncludeSection({ settings, patch }: SectionProps) {
  const [path, setPath] = useState('');
  const [mask, setMask] = useState('*');
  const [recursive, setRecursive] = useState(true);
  const [emptyDirs, setEmptyDirs] = useState(false);

  const add = async () => {
    if (!path.trim()) return;
    const entry = {
      id: globalThis.crypto.randomUUID(),
      path: path.trim(),
      recursive,
      mask: mask.trim() || '*',
      removeEmptyDirs: emptyDirs,
    };
    const next = await patch({ include: [...settings.include, entry] });
    if (next) {
      setPath('');
      setMask('*');
    }
  };

  const remove = (id: string) => void patch({ include: settings.include.filter((e) => e.id !== id) });

  return (
    <Card title={t('settings.include')} testId="settings-include">
      <p className="m-0 mb-2 text-xs text-muted">
        Extra folders to clean. Matching files are deleted when you clean the &ldquo;Custom folders&rdquo; item.
      </p>
      {settings.include.length > 0 && (
        <ul className="m-0 mb-3 list-none divide-y divide-line p-0" data-testid="include-list">
          {settings.include.map((e) => (
            <li key={e.id} data-testid={`include-${e.id}`} className="flex min-w-0 items-start gap-2 py-1.5">
              <span className="min-w-0 flex-1">
                <span className="block break-all text-sm">{e.path}</span>
                <span className="block text-xs text-muted">
                  {e.mask}
                  {e.recursive ? ' - subfolders' : ''}
                  {e.removeEmptyDirs ? ' - remove empty folders' : ''}
                </span>
              </span>
              <button
                type="button"
                onClick={() => remove(e.id)}
                aria-label={`Remove ${e.path}`}
                data-testid={`include-remove-${e.id}`}
                className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border-0 bg-transparent"
              >
                <X size={16} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-col gap-2">
        <input
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="Folder, e.g. ~/scratch"
          aria-label="Folder to include"
          data-testid="include-path"
          className={inputClass}
        />
        <input
          value={mask}
          onChange={(e) => setMask(e.target.value)}
          placeholder="File mask, e.g. *.tmp"
          aria-label="File mask"
          data-testid="include-mask"
          className={inputClass}
        />
        <label className="flex items-center gap-2 text-sm">
          <Checkbox checked={recursive} onChange={setRecursive} testId="include-recursive" />
          Include subfolders
        </label>
        <label className="flex items-center gap-2 text-sm">
          <Checkbox checked={emptyDirs} onChange={setEmptyDirs} testId="include-empty-dirs" />
          Remove empty folders afterwards
        </label>
        <button
          type="button"
          onClick={() => void add()}
          disabled={!path.trim()}
          data-testid="include-add"
          className="flex h-10 items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 text-sm font-medium disabled:opacity-50"
        >
          <FolderPlus size={16} aria-hidden /> Add folder
        </button>
      </div>
    </Card>
  );
}
