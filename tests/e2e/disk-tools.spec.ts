import { spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statfsSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from 'node:fs';
import path from 'node:path';
import type { Page } from '@playwright/test';
import { expect, noHorizontalScroll, test, type ServerInfo } from './fixtures';

// Disk Analyzer, Duplicate Finder and Drive Wiper. Every test builds its own files in a
// throwaway directory next to the sandbox (never under the sandbox's home/root/data,
// which `sandbox` resets), and only deletes what it created.
test.skip(process.platform !== 'linux', 'the fixtures use Linux paths');

function workdir(server: ServerInfo, name: string): string {
  return mkdtempSync(path.join(server.dir, `${name}-`));
}

function put(file: string, content: string | Buffer, mtimeSecs?: number): void {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, content);
  if (mtimeSecs !== undefined) utimesSync(file, mtimeSecs, mtimeSecs);
}

async function fits(page: Page): Promise<void> {
  const m = await noHorizontalScroll(page);
  expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
}

async function openTool(page: Page, id: string): Promise<void> {
  await page.getByTestId('tab-tools').click();
  await page.getByTestId(`tile-${id}`).click();
  await expect(page.getByTestId(`page-${id}`)).toBeVisible();
}

// ---------------------------------------------------------------- Disk Analyzer

test.describe('disk analyzer', () => {
  test('analyze a folder, browse a category, delete a file from disk', async ({ app, server }) => {
    const dir = workdir(server, 'disk');
    try {
      put(path.join(dir, 'pics/photo1.jpg'), Buffer.alloc(3000, 'a'));
      put(path.join(dir, 'pics/photo2.png'), Buffer.alloc(1000, 'b'));
      put(path.join(dir, 'music/song.mp3'), Buffer.alloc(8000, 'c'));
      put(path.join(dir, 'videos/clip.mkv'), Buffer.alloc(20000, 'd'));
      put(path.join(dir, 'docs/report.pdf'), Buffer.alloc(500, 'e'));
      put(path.join(dir, 'docs/notes.txt'), Buffer.alloc(100, 'f'));
      put(path.join(dir, 'data.bin'), Buffer.alloc(40, 'g'));
      // A symlink loop must not hang or double count.
      symlinkSync(dir, path.join(dir, 'pics/loop'));

      await openTool(app, 'disk');
      await expect(app.getByTestId('disk-drives')).toBeVisible();
      await expect(app.getByTestId('btn-disk-analyze')).toBeDisabled();
      await app.getByTestId('disk-folder-input').fill(dir);
      await app.getByTestId('disk-folder-add').click();
      await app.getByTestId('btn-disk-analyze').click();

      await expect(app.getByTestId('disk-categories')).toBeVisible();
      // 3000+1000 pictures, 8000 music, 20000 video, 600 documents, 40 other = 32640
      await expect(app.getByTestId('disk-summary')).toContainText('7 files');
      await expect(app.getByTestId('disk-cat-video-bytes')).toHaveText('19.5 KB');
      await expect(app.getByTestId('disk-cat-video-pct')).toHaveText('61.3%');
      await expect(app.getByTestId('disk-cat-pictures-files')).toHaveText('2 files');
      await expect(app.getByTestId('disk-cat-email')).toHaveCount(0);
      const order = await app
        .locator('[data-testid^="disk-cat-"]')
        .evaluateAll((els) =>
          els.map((e) => e.getAttribute('data-testid') ?? '').filter((id) => /^disk-cat-[a-z]+$/.test(id)),
        );
      expect(order).toEqual(['disk-cat-video', 'disk-cat-music', 'disk-cat-pictures', 'disk-cat-documents', 'disk-cat-other']);
      await fits(app);

      // category -> file list, sorted by size
      await app.getByTestId('disk-cat-pictures').click();
      await expect(app.getByTestId('disk-file-photo1.jpg')).toBeVisible();
      await expect(app.getByTestId('disk-size-photo1.jpg')).toHaveText('2.9 KB');
      const names = await app.locator('[data-testid^="disk-file-"][data-path]').evaluateAll((els) => els.map((e) => e.getAttribute('data-testid')));
      expect(names).toEqual(['disk-file-photo1.jpg', 'disk-file-photo2.png']);
      await app.getByTestId('disk-sort').selectOption('name');
      await expect(app.getByTestId('disk-files-count')).toHaveText('2 files');
      await fits(app);

      // delete photo2.png only
      await app.getByTestId('disk-check-photo2.png').check();
      await app.getByTestId('btn-disk-delete').click();
      await expect(app.getByTestId('confirm-sheet')).toContainText('1000 B will be permanently deleted');
      expect(existsSync(path.join(dir, 'pics/photo2.png'))).toBe(true);
      await app.getByTestId('confirm-sheet-confirm').click();
      await expect(app.getByTestId('disk-note')).toContainText('Deleted 1 file, freed 1000 B');
      await expect(app.getByTestId('disk-file-photo2.png')).toHaveCount(0);
      expect(existsSync(path.join(dir, 'pics/photo2.png'))).toBe(false);
      expect(existsSync(path.join(dir, 'pics/photo1.jpg'))).toBe(true);
      expect(existsSync(path.join(dir, 'music/song.mp3'))).toBe(true);

      // the category bars reflect it without a rescan
      await app.getByTestId('disk-files-back').click();
      await expect(app.getByTestId('disk-cat-pictures-files')).toHaveText('1 file');
      await expect(app.getByTestId('disk-summary')).toContainText('6 files');
      await fits(app);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('folders view drills down with bars and a breadcrumb', async ({ app, server }) => {
    const dir = workdir(server, 'disk-tree');
    try {
      put(path.join(dir, 'big/inner/a.bin'), Buffer.alloc(9000));
      put(path.join(dir, 'small/b.bin'), Buffer.alloc(1000));
      put(path.join(dir, 'top.txt'), Buffer.alloc(50));
      await openTool(app, 'disk');
      await app.getByTestId('disk-folder-input').fill(dir);
      await app.getByTestId('disk-folder-input').press('Enter');
      await app.getByTestId('btn-disk-analyze').click();
      await expect(app.getByTestId('disk-categories')).toBeVisible();

      await app.getByTestId('disk-tab-folders').click();
      await expect(app.getByTestId('disk-folder-big')).toContainText('8.8 KB');
      await expect(app.getByTestId('disk-folder-big')).toContainText('89.6%');
      const bigWidth = await app.getByTestId('disk-folder-bar-big').evaluate((e) => (e as HTMLElement).style.width);
      const smallWidth = await app.getByTestId('disk-folder-bar-small').evaluate((e) => (e as HTMLElement).style.width);
      expect(bigWidth).toBe('100%');
      expect(smallWidth).toBe('11%');
      await expect(app.getByTestId('disk-folder-files')).toContainText('1 here');
      await fits(app);

      await app.getByTestId('disk-folder-big').click();
      await expect(app.getByTestId('disk-crumbs')).toContainText('big');
      await app.getByTestId('disk-folder-inner').click();
      await expect(app.getByTestId('disk-crumbs')).toContainText('inner');
      await fits(app);
      // a crumb jumps back up
      await app.getByTestId('disk-crumb-1').click();
      await expect(app.getByTestId('disk-folder-inner')).toBeVisible();
      // list the files directly from a folder
      await app.getByTestId('disk-folder-inner').click();
      await app.getByTestId('disk-folder-files').click();
      await expect(app.getByTestId('disk-file-a.bin')).toBeVisible();
      await app.getByTestId('disk-files-back').click();
      await expect(app.getByTestId('disk-folders')).toBeVisible();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('opening a folder without a file manager explains itself instead of failing silently', async ({ app, server }) => {
    const dir = workdir(server, 'disk-open');
    try {
      put(path.join(dir, 'a.txt'), 'hello');
      await openTool(app, 'disk');
      await app.getByTestId('disk-folder-input').fill(dir);
      await app.getByTestId('disk-folder-input').press('Enter');
      await app.getByTestId('btn-disk-analyze').click();
      await app.getByTestId('disk-cat-documents').click();
      await app.getByTestId('disk-open-a.txt').click();
      // The sandbox has no programs on PATH. A missing file manager is explained, not flagged
      // as a red error.
      await expect(app.getByTestId('info-banner')).toContainText('xdg-open');
      await expect(app.getByTestId('error-banner')).toHaveCount(0);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('a bad folder is reported', async ({ app }) => {
    await openTool(app, 'disk');
    await app.getByTestId('disk-folder-input').fill('/definitely/not/here');
    await app.getByTestId('disk-folder-input').press('Enter');
    await app.getByTestId('btn-disk-analyze').click();
    await expect(app.getByTestId('error-banner')).toBeVisible();
    await fits(app);
  });
});

// ---------------------------------------------------------------- Duplicate Finder

test.describe('duplicate finder', () => {
  test('find, auto select, delete: one copy of every group survives', async ({ app, server }) => {
    const dir = workdir(server, 'dups');
    try {
      const photo = Buffer.alloc(4000, 'P');
      // three copies of a photo, oldest first
      put(path.join(dir, 'a/photo.jpg'), photo, 1_500_000_000);
      put(path.join(dir, 'b/photo-copy.jpg'), photo, 1_600_000_000);
      put(path.join(dir, 'c/deep/photo (2).jpg'), photo, 1_700_000_000);
      // a pair of notes
      put(path.join(dir, 'a/note.txt'), 'the same note', 1_500_000_000);
      put(path.join(dir, 'b/note.txt'), 'the same note', 1_600_000_000);
      // same size as the notes but different content, and a unique file
      put(path.join(dir, 'b/other.txt'), 'THE SAME NOTE', 1_600_000_000);
      put(path.join(dir, 'unique.bin'), Buffer.alloc(123, 'u'));

      await openTool(app, 'duplicates');
      await expect(app.getByTestId('btn-dup-delete')).toHaveCount(0);
      await app.getByTestId('btn-dup-options').click();
      await app.getByTestId('dup-path-input').fill(dir);
      await app.getByTestId('dup-path-add').click();
      await expect(app.getByTestId('dup-path-list')).toContainText(dir);
      await fits(app);
      await app.getByTestId('dup-options-done').click();
      await app.getByTestId('btn-dup-scan').click();

      await expect(app.getByTestId('dup-groups')).toBeVisible();
      await expect(app.getByTestId('dup-summary')).toContainText('2 groups');
      await expect(app.locator('[data-testid="dup-file"]')).toHaveCount(5);
      // the photo group wastes the most and comes first
      await expect(app.locator('[data-testid^="dup-group-g"]').first()).toHaveAttribute('data-testid', 'dup-group-g0');
      await expect(app.getByTestId('dup-group-g0-summary')).toContainText('3 files of 3.9 KB');
      await fits(app);

      // ticking is limited: two of three copies, then the third is locked
      const g0 = app.getByTestId('dup-group-g0').getByTestId('dup-check');
      await g0.nth(0).check();
      await g0.nth(1).check();
      await expect(g0.nth(2)).toBeDisabled();
      await expect(app.getByTestId('btn-dup-delete')).toBeEnabled();
      await app.getByTestId('btn-dup-clear').click();
      await expect(app.getByTestId('btn-dup-delete')).toBeDisabled();

      // auto select: keep the newest
      await app.getByTestId('btn-dup-auto').click();
      await app.getByTestId('dup-rule-keep_newest').click();
      await expect(app.getByTestId('dup-note')).toContainText('Selected 3 files');
      await expect(app.getByTestId('btn-dup-delete')).toContainText('Delete selected (3)');
      await fits(app);

      await app.getByTestId('btn-dup-delete').click();
      await expect(app.getByTestId('confirm-sheet')).toContainText('At least one copy of each stays');
      // nothing is deleted before the confirmation
      expect(existsSync(path.join(dir, 'a/photo.jpg'))).toBe(true);
      await app.getByTestId('confirm-sheet-confirm').click();
      await expect(app.getByTestId('dup-note')).toContainText('Deleted 3 files');

      // on disk: exactly the newest copy of each group survives, everything else is untouched
      expect(existsSync(path.join(dir, 'a/photo.jpg'))).toBe(false);
      expect(existsSync(path.join(dir, 'b/photo-copy.jpg'))).toBe(false);
      expect(readFileSync(path.join(dir, 'c/deep/photo (2).jpg')).equals(photo)).toBe(true);
      expect(existsSync(path.join(dir, 'a/note.txt'))).toBe(false);
      expect(readFileSync(path.join(dir, 'b/note.txt'), 'utf8')).toBe('the same note');
      expect(readFileSync(path.join(dir, 'b/other.txt'), 'utf8')).toBe('THE SAME NOTE');
      expect(existsSync(path.join(dir, 'unique.bin'))).toBe(true);
      // resolved groups leave the list
      await expect(app.locator('[data-testid="dup-file"]')).toHaveCount(0);
      await expect(app.getByText('No duplicates left')).toBeVisible();
      await fits(app);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('the UI refuses to select every copy and "keep only this" leaves one', async ({ app, server }) => {
    const dir = workdir(server, 'dups-ui');
    try {
      for (const n of ['one', 'two', 'three']) put(path.join(dir, `${n}.dat`), 'identical bytes');
      await openTool(app, 'duplicates');
      await app.getByTestId('btn-dup-options').click();
      await app.getByTestId('dup-path-input').fill(dir);
      await app.getByTestId('dup-path-input').press('Enter');
      await app.getByTestId('dup-options-done').click();
      await app.getByTestId('btn-dup-scan').click();
      await expect(app.locator('[data-testid="dup-file"]')).toHaveCount(3);

      const rows = app.locator('[data-testid="dup-file"]');
      await rows.nth(1).getByTestId('dup-keep-only').click();
      const checked = await rows.getByTestId('dup-check').evaluateAll((els) => els.map((e) => (e as HTMLInputElement).checked));
      expect(checked).toEqual([true, false, true]);
      // the kept one is locked while the others are ticked
      await expect(rows.nth(1).getByTestId('dup-check')).toBeDisabled();
      await expect(app.getByTestId('dup-blocked')).toHaveCount(0);

      // delete, then the group is gone and the kept file is the one we chose
      await app.getByTestId('btn-dup-delete').click();
      await app.getByTestId('confirm-sheet-confirm').click();
      await expect(app.getByTestId('dup-note')).toContainText('Deleted 2 files');
      expect(readdirSync(dir).length).toBe(1);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('a file changed after the search is not deleted', async ({ app, server }) => {
    const dir = workdir(server, 'dups-changed');
    try {
      put(path.join(dir, 'a.txt'), 'same content', 1_500_000_000);
      put(path.join(dir, 'b.txt'), 'same content', 1_600_000_000);
      await openTool(app, 'duplicates');
      await app.getByTestId('btn-dup-options').click();
      await app.getByTestId('dup-path-input').fill(dir);
      await app.getByTestId('dup-path-input').press('Enter');
      await app.getByTestId('dup-options-done').click();
      await app.getByTestId('btn-dup-scan').click();
      await expect(app.locator('[data-testid="dup-file"]')).toHaveCount(2);
      await app.getByTestId('btn-dup-auto').click();
      await app.getByTestId('dup-rule-keep_newest').click();
      // someone edits the old copy (same length, same mtime) before we delete
      writeFileSync(path.join(dir, 'a.txt'), 'DIFFERENT!!!');
      utimesSync(path.join(dir, 'a.txt'), 1_500_000_000, 1_500_000_000);
      await app.getByTestId('btn-dup-delete').click();
      await app.getByTestId('confirm-sheet-confirm').click();
      await expect(app.getByTestId('dup-failures')).toContainText('content differs');
      expect(readFileSync(path.join(dir, 'a.txt'), 'utf8')).toBe('DIFFERENT!!!');
      expect(existsSync(path.join(dir, 'b.txt'))).toBe(true);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('exports the report as a downloadable CSV', async ({ app, server }) => {
    const dir = workdir(server, 'dups-export');
    try {
      put(path.join(dir, 'x/a.txt'), 'twin');
      put(path.join(dir, 'y/a.txt'), 'twin');
      await openTool(app, 'duplicates');
      await app.getByTestId('btn-dup-options').click();
      await app.getByTestId('dup-path-input').fill(dir);
      await app.getByTestId('dup-path-input').press('Enter');
      await app.getByTestId('dup-options-done').click();
      await app.getByTestId('btn-dup-scan').click();
      await expect(app.locator('[data-testid="dup-file"]')).toHaveCount(2);
      await app.getByTestId('btn-dup-export').click();
      const download = app.waitForEvent('download');
      await app.getByTestId('dup-export-csv').click();
      const file = await download;
      expect(file.suggestedFilename()).toBe('clearsweep-duplicates.csv');
      const csv = readFileSync((await file.path()) as string, 'utf8');
      expect(csv.split('\r\n')[0]).toBe('group,wasted_bytes,path,bytes,modified');
      expect(csv).toContain(`${dir}/x/a.txt`);
      expect(csv).toContain(`${dir}/y/a.txt`);
      await expect(app.getByTestId('dup-note')).toContainText('Saved clearsweep-duplicates.csv');
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('options sheet validates and stays within the screen width', async ({ app }) => {
    await openTool(app, 'duplicates');
    await app.getByTestId('btn-dup-scan').click();
    await expect(app.getByTestId('dup-form-error')).toContainText('at least one folder');
    await expect(app.getByTestId('dup-options')).toBeVisible();
    await fits(app);
    await app.getByTestId('dup-match-content').uncheck();
    await expect(app.getByTestId('dup-weak-warning')).toBeVisible();
    await app.getByTestId('dup-path-input').fill('/some/long/folder/name/that/keeps/going/and/going/and/going/on/and/on');
    await app.getByTestId('dup-path-input').press('Enter');
    await fits(app);
    await app.getByTestId('dup-options-done').click();
    await app.getByTestId('btn-dup-scan').click();
    await expect(app.getByTestId('dup-form-error')).toContainText('at least one way');
  });
});

// ---------------------------------------------------------------- Drive Wiper

/** A fake machine in the sandbox root: sysfs + /proc/mounts as the server reads them. */
function fakeMachine(server: ServerInfo): { dev: (n: string) => string } {
  const root = path.join(server.dir, 'root');
  const dev = (n: string) => path.join(root, 'dev', n);
  const disk = (name: string, sectors: number, parts: string[], removable = '0') => {
    const d = path.join(root, 'sys/block', name);
    put(path.join(d, 'size'), `${sectors}\n`);
    put(path.join(d, 'removable'), `${removable}\n`);
    put(path.join(d, 'device/model'), `Model ${name}\n`);
    for (const p of parts) {
      put(path.join(d, p, 'partition'), '1\n');
      put(path.join(d, p, 'size'), '1000\n');
    }
  };
  disk('sda', 2_000_000, ['sda1', 'sda2']);
  disk('sdb', 4_000_000, ['sdb1'], '1');
  disk('sdc', 4_000_000, [], '1');
  put(
    path.join(root, 'proc/mounts'),
    `${dev('sda2')} / ext4 rw 0 0\n${dev('sda1')} /boot/efi vfat rw 0 0\n${dev('sdb1')} /media/user/USB\\040STICK vfat rw 0 0\n`,
  );
  return { dev };
}

test.describe('drive wiper', () => {
  test('entire drive: system and mounted drives are blocked, the typed path gates the button', async ({ app, server }) => {
    const { dev } = fakeMachine(server);
    await openTool(app, 'wiper');
    await expect(app.getByTestId('wiper-warning')).toContainText('fills the drive');
    await expect(app.getByTestId('btn-wiper-start')).toBeDisabled();
    await fits(app);

    await app.getByTestId('wiper-mode-drive').click();
    await expect(app.getByTestId('wiper-warning')).toContainText('destroys EVERYTHING');
    const sda = app.getByTestId(`wiper-device-${dev('sda')}`);
    const sdb = app.getByTestId(`wiper-device-${dev('sdb')}`);
    const sdc = app.getByTestId(`wiper-device-${dev('sdc')}`);
    await expect(sda).toBeDisabled();
    await expect(sda).toContainText('System');
    await expect(app.getByTestId(`wiper-reason-${dev('sda')}`)).toContainText('System drive');
    await expect(sdb).toBeDisabled();
    await expect(app.getByTestId(`wiper-reason-${dev('sdb')}`)).toContainText('/media/user/USB STICK');
    await expect(sdc).toBeEnabled();
    await fits(app);

    await sdc.click();
    const input = app.getByTestId('wiper-confirm-input');
    const start = app.getByTestId('btn-wiper-start');
    await expect(start).toBeDisabled();
    await input.fill('sdc');
    await expect(start).toBeDisabled();
    await input.fill(`${dev('sdc')} `);
    await expect(start).toBeDisabled();
    await input.fill(dev('sdc'));
    await expect(start).toBeEnabled();
    await fits(app);

    // The fake device node does not exist, so the server stops at open() (or at the
    // privilege check) and nothing is written anywhere.
    await start.click();
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('error-banner')).toBeVisible();
    await expect(app.getByTestId('wiper-result')).toHaveCount(0);
    expect(existsSync(dev('sdc'))).toBe(false);
  });

  test('the server itself refuses a system disk even with the right confirmation', async ({ app, server }) => {
    const { dev } = fakeMachine(server);
    const res = await fetch(`${server.origin}/api/call`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': server.token },
      body: JSON.stringify({
        callId: 'wipe-sys',
        method: 'wiper.wipe_drive',
        params: { device: dev('sda'), passes: 1, confirm: dev('sda') },
      }),
    });
    const last = JSON.parse((await res.text()).trim().split('\n').pop() ?? '{}') as { type: string; error?: { code: string; message: string } };
    expect(last.type).toBe('error');
    expect(last.error?.code).toBe('PermissionDenied');
    expect(last.error?.message).toContain('operating system');
    void app;
  });

  test('free space: pass options and the system badge', async ({ app }) => {
    await openTool(app, 'wiper');
    await expect(app.getByTestId('wiper-drives')).toBeVisible();
    await expect(app.locator('[data-testid^="wiper-drive-"]').first()).toBeVisible();
    await expect(app.getByTestId('wiper-system-badge').first()).toBeVisible();
    const values = await app.getByTestId('wiper-passes').locator('option').evaluateAll((els) => els.map((e) => (e as HTMLOptionElement).value));
    expect(values).toEqual(['1', '3', '7', '35']);
    await app.getByTestId('wiper-passes').selectOption('3');
    await fits(app);
  });

  test('free space wipe on a real 16 MiB ext4 image fills it, then restores everything', async ({ app, server }) => {
    test.skip(process.getuid?.() !== 0, 'mounting a loop device needs root');
    const img = path.join(server.dir, 'wipe.img');
    const mnt = path.join(server.dir, 'wipe-mnt');
    mkdirSync(mnt, { recursive: true });
    writeFileSync(img, Buffer.alloc(16 * 1024 * 1024));
    const made = spawnSync('mkfs.ext4', ['-q', '-F', img]);
    const mounted = made.status === 0 ? spawnSync('mount', ['-o', 'loop', img, mnt]) : made;
    if (mounted.status !== 0) {
      rmSync(mnt, { recursive: true, force: true });
      rmSync(img, { force: true });
    }
    test.skip(mounted.status !== 0, 'cannot create and mount an ext4 loop image in this environment');
    try {
      put(path.join(mnt, 'keep.txt'), 'do not touch');
      const before = statfsSync(mnt);
      const freeBefore = before.bavail * before.bsize;
      expect(freeBefore).toBeGreaterThan(8 * 1024 * 1024);

      await openTool(app, 'wiper');
      const drive = app.getByTestId(`wiper-drive-${mnt}`);
      await expect(drive).toBeVisible();
      await expect(drive).not.toContainText('System');
      await drive.click();
      await fits(app);
      await app.getByTestId('btn-wiper-start').click();
      await expect(app.getByTestId('confirm-sheet')).toContainText(mnt);
      await app.getByTestId('confirm-sheet-confirm').click();
      await expect(app.getByTestId('wiper-result')).toContainText('Wiped', { timeout: 25_000 });
      await expect(app.getByTestId('wiper-result')).toContainText('Temporary files were removed');

      // Everything the wipe created is gone and the space is back.
      expect(readdirSync(mnt).filter((n) => n !== 'lost+found')).toEqual(['keep.txt']);
      expect(readFileSync(path.join(mnt, 'keep.txt'), 'utf8')).toBe('do not touch');
      const after = statfsSync(mnt);
      expect(after.bavail * after.bsize).toBeGreaterThan(freeBefore - 64 * 1024);
      await fits(app);
    } finally {
      spawnSync('umount', [mnt]);
      // Never delete through a mount that is still up.
      if (spawnSync('mountpoint', ['-q', mnt]).status !== 0) {
        rmSync(mnt, { recursive: true, force: true });
        rmSync(img, { force: true });
      }
    }
  });
});
