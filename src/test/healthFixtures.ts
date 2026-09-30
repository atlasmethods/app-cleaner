import type { FixReport, HealthCategory, HealthReport } from '../api/health';
import type { Settings } from '../api/settings';

const GB = 1024 * 1024 * 1024;

export const privacyCat: HealthCategory = {
  id: 'privacy',
  title: 'Privacy',
  status: 'warning',
  summary: '12 tracking cookies in 3 browsers.',
  fixable: true,
  metrics: { trackers: 12, historyRows: 0 },
  findings: [{ kind: 'trackers', count: 12, browsers: ['Google Chrome', 'Mozilla Firefox', 'Brave'] }],
};

export const spaceCat: HealthCategory = {
  id: 'space',
  title: 'Space',
  status: 'problem',
  summary: '1.2 GB of junk in 214 files.',
  fixable: true,
  metrics: { bytes: Math.round(1.2 * GB), files: 214, rows: 0 },
  findings: [
    { kind: 'junk', group: 'Google Chrome', bytes: Math.round(0.8 * GB), files: 100, rows: 0 },
    { kind: 'junk', group: 'System', bytes: Math.round(0.4 * GB), files: 114, rows: 0 },
  ],
};

export const speedCat: HealthCategory = {
  id: 'speed',
  title: 'Speed',
  status: 'problem',
  summary: '4 startup items slow down startup; 2 apps run in the background (900 MB).',
  fixable: true,
  metrics: { highImpact: 1, mediumImpact: 3, startupItems: 4, backgroundApps: 2 },
  findings: [
    {
      kind: 'startup',
      items: [
        { id: 'xdg:user:slack.desktop', name: 'Slack', impact: 'high' },
        { id: 'xdg:user:spotify.desktop', name: 'Spotify', impact: 'medium' },
        { id: 'xdg:user:dropbox.desktop', name: 'Dropbox', impact: 'medium' },
        { id: 'xdg:user:steam.desktop', name: 'Steam', impact: 'medium' },
      ],
    },
    {
      kind: 'background_apps',
      apps: [
        { appId: 'slack', name: 'Slack', memoryBytes: 600 * 1024 * 1024 },
        { appId: 'spotify', name: 'Spotify', memoryBytes: 300 * 1024 * 1024 },
      ],
    },
  ],
};

export const securityCat: HealthCategory = {
  id: 'security',
  title: 'Security',
  status: 'problem',
  summary: '3 updates available, 1 security.',
  fixable: true,
  metrics: { updates: 3, securityUpdates: 1 },
  findings: [
    {
      kind: 'updates',
      count: 3,
      security: 1,
      items: [
        { id: 'apt:firefox', name: 'firefox', currentVersion: '125.0', newVersion: '126.0', security: true },
        { id: 'apt:vim', name: 'vim', currentVersion: '9.0', newVersion: '9.1', security: false },
        { id: 'apt:htop', name: 'htop', currentVersion: '3.2', newVersion: '3.3', security: false },
      ],
    },
  ],
};

export const report72: HealthReport = {
  score: 72,
  scannedAt: '2026-05-01T10:00:00Z',
  categories: [privacyCat, spaceCat, speedCat, securityCat],
};

const goodCat = (c: HealthCategory, summary: string): HealthCategory => ({
  ...c,
  status: 'good',
  summary,
  fixable: false,
  findings: [],
  metrics: {},
});

export const report100: HealthReport = {
  score: 100,
  scannedAt: '2026-05-01T10:05:00Z',
  categories: [
    goodCat(privacyCat, 'No tracking cookies or browsing traces found.'),
    goodCat(spaceCat, 'No junk files found.'),
    goodCat(speedCat, 'Nothing is slowing down startup or running in the background.'),
    goodCat(securityCat, 'All software is up to date.'),
  ],
};

export const reportClean: HealthReport = { ...report100, score: 100 };

export function fixReport(over: Partial<FixReport> = {}): FixReport {
  return {
    parts: [
      {
        part: 'privacy',
        status: 'done',
        message: 'Removed 12 database entries.',
        removedBytes: 0,
        removedFiles: 0,
        removedRows: 12,
        blockedApps: [],
        items: [],
      },
      {
        part: 'space',
        status: 'done',
        message: 'Removed 1.2 GB in 214 files.',
        removedBytes: Math.round(1.2 * GB),
        removedFiles: 214,
        removedRows: 0,
        blockedApps: [],
        items: [],
      },
    ],
    cancelled: false,
    report: report100,
    ...over,
  };
}

export const baseSettings: Settings = {
  theme: 'system',
  secureDelete: { enabled: false, passes: 1 },
  closeBrowsers: 'ask',
  tempMinAgeHours: 24,
  include: [],
  exclude: [],
  cookieKeep: [],
  selectedRules: null,
  smart: { enabled: false, thresholdMb: 500, cleanOnBrowserClose: [], autoClean: false, notify: true },
  runAtStartup: false,
  language: 'en',
};
