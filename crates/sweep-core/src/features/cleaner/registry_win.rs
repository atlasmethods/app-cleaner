//! Windows registry MRU cleaning (HKCU only; keys are allow-listed by rule validation).

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, KEY_WRITE};
use winreg::RegKey;

fn open(key: &str, write: bool) -> Result<Option<RegKey>, String> {
    let sub = key
        .strip_prefix("HKCU\\")
        .ok_or_else(|| format!("unsupported registry root in `{key}`"))?;
    let flags = if write {
        KEY_READ | KEY_WRITE | KEY_SET_VALUE
    } else {
        KEY_READ
    };
    match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(sub, flags) {
        Ok(k) => Ok(Some(k)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Number of values and/or subkeys `clear` would remove.
pub fn count(key: &str, values: bool, subkeys: bool) -> Result<u64, String> {
    let Some(k) = open(key, false)? else {
        return Ok(0);
    };
    let mut n = 0u64;
    if values {
        n += k.enum_values().filter(|r| r.is_ok()).count() as u64;
    }
    if subkeys {
        n += k.enum_keys().filter(|r| r.is_ok()).count() as u64;
    }
    Ok(n)
}

/// Delete the values and/or subkeys; returns how many were removed.
pub fn clear(key: &str, values: bool, subkeys: bool) -> Result<u64, String> {
    let Some(k) = open(key, true)? else {
        return Ok(0);
    };
    let mut n = 0u64;
    if values {
        let names: Vec<String> = k
            .enum_values()
            .filter_map(|r| r.ok().map(|(n, _)| n))
            .collect();
        for name in names {
            if k.delete_value(&name).is_ok() {
                n += 1;
            }
        }
    }
    if subkeys {
        let names: Vec<String> = k.enum_keys().filter_map(|r| r.ok()).collect();
        for name in names {
            if k.delete_subkey_all(&name).is_ok() {
                n += 1;
            }
        }
    }
    Ok(n)
}
