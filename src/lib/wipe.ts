import type { Passes, WiperDevice } from '../api/wiper';

export const PASS_OPTIONS: { value: Passes; label: string; hint: string }[] = [
  { value: 1, label: '1 pass', hint: 'Simple overwrite with random data' },
  { value: 3, label: '3 passes', hint: 'DoD 5220.22-M' },
  { value: 7, label: '7 passes', hint: 'Extra thorough' },
  { value: 35, label: '35 passes', hint: 'Gutmann (very slow)' },
];

export function isPasses(n: number): n is Passes {
  return n === 1 || n === 3 || n === 7 || n === 35;
}

/** The typed confirmation must be the device path, character for character. */
export function confirmMatches(device: string, typed: string): boolean {
  return device.length > 0 && typed === device;
}

/** Why a whole-drive wipe of this device is not offered, or `null` when it is. */
export function deviceBlockReason(d: WiperDevice): string | null {
  if (d.isSystem) return 'System drive: it holds the operating system';
  if (d.mounts.length > 0) return `In use: mounted at ${d.mounts.join(', ')}. Unmount it first.`;
  if (d.swap) return 'In use as swap space';
  if (d.holders.length > 0) return `In use by ${d.holders.join(', ')}`;
  return null;
}

export function canWipeDrive(d: WiperDevice | null, typed: string): boolean {
  return d !== null && deviceBlockReason(d) === null && confirmMatches(d.device, typed);
}
