//! In-memory Windows for tests: a registry, a file system, environment variables and drives.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use super::regaccess::{RegData, RegValue, RegistryAccess, Root, Stat, WinProbe};

fn norm(p: &str) -> String {
    p.replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

#[derive(Default, Clone)]
struct FakeKey {
    display: String,
    values: Vec<RegValue>,
}

/// A fake machine. Names are case-insensitive like on Windows.
#[derive(Default)]
pub struct FakeWin {
    keys: RefCell<BTreeMap<(Root, String), FakeKey>>,
    files: RefCell<HashSet<String>>,
    dirs: RefCell<HashSet<String>>,
    unreadable: RefCell<HashSet<String>>,
    env: RefCell<HashMap<String, String>>,
    fixed: RefCell<HashSet<char>>,
}

impl FakeWin {
    /// A machine with `C:` (fixed), a Windows directory and the usual environment.
    pub fn new() -> FakeWin {
        let w = FakeWin::default();
        w.fixed.borrow_mut().insert('c');
        for (k, v) in [
            ("SystemRoot", r"C:\Windows"),
            ("windir", r"C:\Windows"),
            ("ProgramFiles", r"C:\Program Files"),
            ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
            ("APPDATA", r"C:\Users\Bob\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\Bob\AppData\Local"),
            ("ProgramData", r"C:\ProgramData"),
            ("PATH", r"C:\Windows\system32;C:\Windows;C:\Tools"),
        ] {
            w.env(k, v);
        }
        for d in [
            r"C:\",
            r"C:\Windows",
            r"C:\Windows\System32",
            r"C:\Windows\SysWOW64",
            r"C:\Windows\Fonts",
            r"C:\Program Files",
            r"C:\Program Files (x86)",
        ] {
            w.dir(d);
        }
        w
    }

    pub fn env(&self, k: &str, v: &str) -> &Self {
        self.env
            .borrow_mut()
            .insert(k.to_ascii_lowercase(), v.into());
        self
    }
    pub fn file(&self, p: &str) -> &Self {
        self.files.borrow_mut().insert(norm(p));
        // make sure the parents exist as directories
        let mut cur = String::new();
        let parts: Vec<&str> = p.split(['\\', '/']).collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !cur.is_empty() {
                cur.push('\\');
            }
            cur.push_str(part);
            self.dirs.borrow_mut().insert(norm(&cur));
        }
        self
    }
    pub fn dir(&self, p: &str) -> &Self {
        self.dirs.borrow_mut().insert(norm(p));
        self
    }
    /// Probing `p` reports `Unknown` (access denied).
    pub fn unreadable(&self, p: &str) -> &Self {
        self.unreadable.borrow_mut().insert(norm(p));
        self
    }
    pub fn fixed_drive(&self, letter: char) -> &Self {
        self.fixed.borrow_mut().insert(letter.to_ascii_lowercase());
        // the drive root exists
        self.dir(&format!("{}:\\", letter.to_ascii_uppercase()));
        self
    }
    /// A drive that exists but is not fixed (removable / network).
    pub fn removable_drive(&self, letter: char) -> &Self {
        self.dir(&format!("{}:\\", letter.to_ascii_uppercase()));
        self
    }

    /// Create a key (and its ancestors).
    pub fn key(&self, root: Root, key: &str) -> &Self {
        let mut cur = String::new();
        let mut keys = self.keys.borrow_mut();
        for part in key.split('\\') {
            if !cur.is_empty() {
                cur.push('\\');
            }
            cur.push_str(part);
            keys.entry((root, cur.to_ascii_lowercase()))
                .or_insert_with(|| FakeKey {
                    display: part.to_string(),
                    values: Vec::new(),
                });
        }
        self
    }
    pub fn set(&self, root: Root, key: &str, name: &str, data: RegData) -> &Self {
        self.key(root, key);
        let mut keys = self.keys.borrow_mut();
        let k = keys.get_mut(&(root, key.to_ascii_lowercase())).unwrap();
        k.values.retain(|v| !v.name.eq_ignore_ascii_case(name));
        k.values.push(RegValue {
            name: name.to_string(),
            data,
        });
        self
    }
    pub fn sz(&self, root: Root, key: &str, name: &str, data: &str) -> &Self {
        self.set(root, key, name, RegData::Str(data.into()))
    }
    pub fn expand(&self, root: Root, key: &str, name: &str, data: &str) -> &Self {
        self.set(root, key, name, RegData::ExpandStr(data.into()))
    }
    pub fn dword(&self, root: Root, key: &str, name: &str, data: u32) -> &Self {
        self.set(root, key, name, RegData::Dword(data))
    }
    /// Remove a key and everything below it (what a successful `reg delete` does).
    pub fn delete_key(&self, root: Root, key: &str) {
        let prefix = format!("{}\\", key.to_ascii_lowercase());
        self.keys.borrow_mut().retain(|(r, k), _| {
            !(*r == root && (k == &key.to_ascii_lowercase() || k.starts_with(&prefix)))
        });
    }
}

impl WinProbe for FakeWin {
    fn stat(&self, path: &str) -> Stat {
        let n = norm(path);
        if self.unreadable.borrow().contains(&n) {
            return Stat::Unknown;
        }
        if self.files.borrow().contains(&n) {
            Stat::File
        } else if self.dirs.borrow().contains(&n) {
            Stat::Dir
        } else {
            Stat::Missing
        }
    }
    fn env_var(&self, name: &str) -> Option<String> {
        self.env.borrow().get(&name.to_ascii_lowercase()).cloned()
    }
    fn is_fixed_drive(&self, letter: char) -> bool {
        self.fixed.borrow().contains(&letter.to_ascii_lowercase())
    }
}

impl RegistryAccess for FakeWin {
    fn subkeys(&self, root: Root, key: &str) -> Vec<String> {
        let prefix = format!("{}\\", key.to_ascii_lowercase());
        self.keys
            .borrow()
            .iter()
            .filter(|((r, k), _)| {
                *r == root && k.starts_with(&prefix) && !k[prefix.len()..].contains('\\')
            })
            .map(|(_, v)| v.display.clone())
            .collect()
    }
    fn values(&self, root: Root, key: &str) -> Vec<RegValue> {
        self.keys
            .borrow()
            .get(&(root, key.to_ascii_lowercase()))
            .map(|k| k.values.clone())
            .unwrap_or_default()
    }
    fn key_exists(&self, root: Root, key: &str) -> bool {
        self.keys
            .borrow()
            .contains_key(&(root, key.to_ascii_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_is_case_insensitive_and_enumerates() {
        let w = FakeWin::new();
        w.sz(Root::Hklm, r"SOFTWARE\A\B", "v", "1");
        w.sz(Root::Hklm, r"SOFTWARE\A\C", "", "d");
        assert!(w.key_exists(Root::Hklm, r"software\a"));
        let mut s = w.subkeys(Root::Hklm, r"SOFTWARE\A");
        s.sort();
        assert_eq!(s, ["B", "C"]);
        assert_eq!(
            w.string(Root::Hklm, r"software\a\b", "V").as_deref(),
            Some("1")
        );
        assert_eq!(
            w.string(Root::Hklm, r"SOFTWARE\A\C", "").as_deref(),
            Some("d")
        );
        w.delete_key(Root::Hklm, r"SOFTWARE\A");
        assert!(!w.key_exists(Root::Hklm, r"SOFTWARE\A\B"));
        w.file(r"D:\x\y.txt");
        assert_eq!(w.stat("d:/X/Y.TXT"), Stat::File);
        assert_eq!(w.stat(r"D:\x"), Stat::Dir);
        assert_eq!(w.stat(r"D:\nope"), Stat::Missing);
        w.unreadable(r"D:\x\y.txt");
        assert_eq!(w.stat(r"D:\x\y.txt"), Stat::Unknown);
    }
}
