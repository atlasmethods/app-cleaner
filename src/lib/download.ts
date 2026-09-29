import { isTauri } from './transport';

/**
 * Hand `text` to the user as a file. Browsers get a normal download; the desktop webview
 * cannot reliably save blobs, so there the text goes to the clipboard instead.
 * Returns what happened, for a status message.
 */
export async function saveText(
  filename: string,
  text: string,
  mime = 'text/plain',
): Promise<'downloaded' | 'copied' | 'failed'> {
  if (isTauri()) {
    try {
      await navigator.clipboard.writeText(text);
      return 'copied';
    } catch {
      return 'failed';
    }
  }
  try {
    const url = URL.createObjectURL(new Blob([text], { type: `${mime};charset=utf-8` }));
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.style.display = 'none';
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 10_000);
    return 'downloaded';
  } catch {
    return 'failed';
  }
}
