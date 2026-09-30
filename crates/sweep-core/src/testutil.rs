//! Fixture builder shared by unit tests, the CLI tests and the end-to-end sandbox.
//!
//! Everything is created under one base directory using [`Env::for_test`]'s layout
//! (`home/`, `root/`, `data/`), for the *current* operating system's paths. Browser
//! databases are real SQLite files with a realistic subset of the browsers' schemas.

use filetime::FileTime;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::ctx::{Env, Os};

/// Chromium-family browsers the fixture knows how to lay out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chromium {
    Chrome,
    Chromium,
    Edge,
    Brave,
    Vivaldi,
}

impl Chromium {
    /// Rule-id prefix (`chrome`, `edge`, ...).
    pub fn key(self) -> &'static str {
        match self {
            Chromium::Chrome => "chrome",
            Chromium::Chromium => "chromium",
            Chromium::Edge => "edge",
            Chromium::Brave => "brave",
            Chromium::Vivaldi => "vivaldi",
        }
    }

    /// Directory below the config / cache / local-appdata root.
    fn rel(self, os: Os) -> &'static str {
        match (os, self) {
            (Os::Linux, Chromium::Chrome) => "google-chrome",
            (Os::Linux, Chromium::Chromium) => "chromium",
            (Os::Linux, Chromium::Edge) => "microsoft-edge",
            (Os::Linux, Chromium::Brave) => "BraveSoftware/Brave-Browser",
            (Os::Linux, Chromium::Vivaldi) => "vivaldi",
            (Os::Windows, Chromium::Chrome) => "Google/Chrome/User Data",
            (Os::Windows, Chromium::Chromium) => "Chromium/User Data",
            (Os::Windows, Chromium::Edge) => "Microsoft/Edge/User Data",
            (Os::Windows, Chromium::Brave) => "BraveSoftware/Brave-Browser/User Data",
            (Os::Windows, Chromium::Vivaldi) => "Vivaldi/User Data",
            (Os::MacOs, Chromium::Chrome) => "Google/Chrome",
            (Os::MacOs, Chromium::Chromium) => "Chromium",
            (Os::MacOs, Chromium::Edge) => "Microsoft Edge",
            (Os::MacOs, Chromium::Brave) => "BraveSoftware/Brave-Browser",
            (Os::MacOs, Chromium::Vivaldi) => "Vivaldi",
        }
    }
}

/// Where one Chromium profile lives.
#[derive(Debug, Clone)]
pub struct ChromiumProfile {
    /// Profile data (History, Cookies, ...).
    pub data: PathBuf,
    /// Cache directories' parent (same as `data` on Windows).
    pub cache: PathBuf,
}

/// Where one Firefox profile lives.
#[derive(Debug, Clone)]
pub struct FirefoxProfile {
    pub data: PathBuf,
    /// `cache2` parent.
    pub cache: PathBuf,
}

/// Deterministic non-trivial content of `size` bytes.
pub fn blob(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8 ^ 0x5A).collect()
}

pub struct Fixture {
    pub env: Env,
}

impl Fixture {
    /// Create the base directories under `base`.
    pub fn new(base: &Path) -> Self {
        let env = Env::for_test(base);
        for d in [
            &env.root,
            &env.home,
            &env.config_dir,
            &env.cache_dir,
            &env.user_data_dir,
            &env.data_local_dir,
            &env.data_dir,
            &env.temp_dir,
        ] {
            fs::create_dir_all(d).expect("create fixture dir");
        }
        Fixture { env }
    }

    // ------------------------------------------------------------ primitives

    /// Write `size` bytes at `path` (parents created). Returns the path.
    pub fn file(&self, path: impl AsRef<Path>, size: usize) -> PathBuf {
        let p = path.as_ref().to_path_buf();
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(&p, blob(size)).expect("write");
        p
    }

    /// Like [`Fixture::file`] with the modification time set `age_hours` in the past.
    pub fn aged_file(&self, path: impl AsRef<Path>, size: usize, age_hours: u64) -> PathBuf {
        let p = self.file(path, size);
        set_age_hours(&p, age_hours);
        p
    }

    pub fn dir(&self, path: impl AsRef<Path>) -> PathBuf {
        let p = path.as_ref().to_path_buf();
        fs::create_dir_all(&p).expect("mkdir");
        p
    }

    /// Create an SQLite database with `schema_and_data` executed as a batch.
    pub fn sqlite(&self, path: impl AsRef<Path>, schema_and_data: &str) -> PathBuf {
        let p = path.as_ref().to_path_buf();
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        let _ = fs::remove_file(&p);
        let c = Connection::open(&p).expect("open sqlite");
        c.execute_batch(schema_and_data).expect("populate sqlite");
        p
    }

    // ------------------------------------------------------------ Chromium

    pub fn chromium_profile(&self, b: Chromium, profile: &str) -> ChromiumProfile {
        let e = &self.env;
        let rel = b.rel(e.os);
        match e.os {
            Os::Linux => ChromiumProfile {
                data: e.config_dir.join(rel).join(profile),
                cache: e.cache_dir.join(rel).join(profile),
            },
            Os::Windows => {
                let p = e.data_local_dir.join(rel).join(profile);
                ChromiumProfile {
                    data: p.clone(),
                    cache: p,
                }
            }
            Os::MacOs => ChromiumProfile {
                data: e.config_dir.join(rel).join(profile),
                cache: e.cache_dir.join(rel).join(profile),
            },
        }
    }

    /// A Chromium profile with cache, history, downloads, cookies, session, form data
    /// and saved logins. Cache content: `Cache` 60 KiB + `Code Cache` 25 KiB +
    /// `GPUCache` 15 KiB = exactly 100 KiB (`cache_bytes()`), plus a 4 KiB
    /// `Cache/Cache_Data/keepme.bin` that tests use for exclusions.
    pub fn populate_chromium(&self, b: Chromium, profile: &str) -> ChromiumProfile {
        let p = self.chromium_profile(b, profile);
        // cache
        self.file(p.cache.join("Cache/Cache_Data/data_0"), 40 * 1024);
        self.file(p.cache.join("Cache/Cache_Data/data_1"), 20 * 1024);
        self.file(p.cache.join("Cache/Cache_Data/keepme.bin"), 4096);
        self.file(p.cache.join("Code Cache/js/index"), 25 * 1024);
        self.file(p.cache.join("GPUCache/data_0"), 15 * 1024);
        // History with real table shapes
        self.sqlite(
            p.data.join("History"),
            "CREATE TABLE urls(id INTEGER PRIMARY KEY AUTOINCREMENT, url LONGVARCHAR, title LONGVARCHAR,
                visit_count INTEGER DEFAULT 0 NOT NULL, typed_count INTEGER DEFAULT 0 NOT NULL,
                last_visit_time INTEGER NOT NULL, hidden INTEGER DEFAULT 0 NOT NULL);
             CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER NOT NULL, visit_time INTEGER NOT NULL,
                from_visit INTEGER, transition INTEGER DEFAULT 0 NOT NULL, segment_id INTEGER,
                visit_duration INTEGER DEFAULT 0 NOT NULL);
             CREATE TABLE visit_source(id INTEGER PRIMARY KEY, source INTEGER NOT NULL);
             CREATE TABLE keyword_search_terms(keyword_id INTEGER NOT NULL, url_id INTEGER NOT NULL,
                term LONGVARCHAR NOT NULL, normalized_term LONGVARCHAR NOT NULL);
             CREATE TABLE segments(id INTEGER PRIMARY KEY, name VARCHAR, url_id INTEGER NON NULL);
             CREATE TABLE segment_usage(id INTEGER PRIMARY KEY, segment_id INTEGER NOT NULL,
                time_slot INTEGER NOT NULL, visit_count INTEGER DEFAULT 0 NOT NULL);
             CREATE TABLE downloads(id INTEGER PRIMARY KEY, guid VARCHAR NOT NULL, current_path LONGVARCHAR NOT NULL,
                target_path LONGVARCHAR NOT NULL, start_time INTEGER NOT NULL, received_bytes INTEGER NOT NULL,
                total_bytes INTEGER NOT NULL, state INTEGER NOT NULL, tab_url VARCHAR, mime_type VARCHAR);
             CREATE TABLE downloads_url_chains(id INTEGER NOT NULL, chain_index INTEGER NOT NULL,
                url LONGVARCHAR NOT NULL, PRIMARY KEY (id, chain_index));
             INSERT INTO urls(id,url,title,visit_count,last_visit_time) VALUES
                (1,'https://example.org/','Example',3,1),(2,'https://rust-lang.org/','Rust',2,2),
                (3,'https://news.example.com/','News',1,3);
             INSERT INTO visits(id,url,visit_time) VALUES (1,1,1),(2,1,2),(3,2,3),(4,3,4);
             INSERT INTO visit_source VALUES (1,0),(2,0);
             INSERT INTO keyword_search_terms VALUES (2,1,'Rust Book','rust book');
             INSERT INTO segments VALUES (1,'https://example.org/',1);
             INSERT INTO segment_usage VALUES (1,1,100,2);
             INSERT INTO downloads VALUES (1,'g-1','/dl/a.zip','/dl/a.zip',1,100,100,1,'https://example.org/','application/zip'),
                (2,'g-2','/dl/b.pdf','/dl/b.pdf',2,50,50,1,'https://example.org/','application/pdf');
             INSERT INTO downloads_url_chains VALUES (1,0,'https://example.org/a.zip'),(2,0,'https://example.org/b.pdf');",
        );
        // cookies: 3 for google (host, dotted, subdomain), a lookalike, a tracker
        self.sqlite(
            p.data.join("Network/Cookies"),
            "CREATE TABLE cookies(creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL, name TEXT NOT NULL,
                value TEXT NOT NULL, path TEXT NOT NULL, expires_utc INTEGER NOT NULL, is_secure INTEGER NOT NULL);
             INSERT INTO cookies VALUES (1,'.google.com','SID','a','/',0,1),(2,'accounts.google.com','LSID','b','/',0,1),
                (3,'google.com','NID','c','/',0,1),(4,'notgoogle.com','x','d','/',0,0),
                (5,'.tracker.example','uid','e','/',0,0),(6,'.tracker.example','uid2','f','/',0,0),
                (7,'.github.com','_gh_sess','g','/',0,1);",
        );
        // autofill + a table that must survive
        self.sqlite(
            p.data.join("Web Data"),
            "CREATE TABLE autofill(name VARCHAR NOT NULL, value VARCHAR NOT NULL, value_lower VARCHAR NOT NULL,
                date_created INTEGER DEFAULT 0, date_last_used INTEGER DEFAULT 0, count INTEGER DEFAULT 1);
             CREATE TABLE autofill_profiles(guid VARCHAR PRIMARY KEY, first_name VARCHAR, street_address VARCHAR);
             INSERT INTO autofill VALUES ('email','me@example.org','me@example.org',1,1,3),('city','Oslo','oslo',1,1,1);
             INSERT INTO autofill_profiles VALUES ('p-1','Ada','1 Main St');",
        );
        // saved passwords
        self.sqlite(
            p.data.join("Login Data"),
            "CREATE TABLE logins(origin_url VARCHAR NOT NULL, username_value VARCHAR, password_value BLOB,
                date_created INTEGER NOT NULL DEFAULT 0, id INTEGER PRIMARY KEY AUTOINCREMENT);
             CREATE TABLE stats(origin_domain VARCHAR NOT NULL, username_value VARCHAR, dismissal_count INTEGER);
             INSERT INTO logins(origin_url,username_value,password_value) VALUES
                ('https://example.org/','ada',x'0102'),('https://github.com/','ada',x'0304');
             INSERT INTO stats VALUES ('example.org','ada',1);",
        );
        // session
        self.file(p.data.join("Sessions/Session_13300000000"), 2048);
        self.file(p.data.join("Sessions/Tabs_13300000000"), 1024);
        self.file(p.data.join("Current Session"), 512);
        // something in the profile that must never be touched
        self.file(p.data.join("Bookmarks"), 300);
        self.file(p.data.join("Preferences"), 400);
        p
    }

    /// Total bytes of the cache files `populate_chromium` creates (excluding keepme.bin).
    pub fn chromium_cache_bytes() -> u64 {
        100 * 1024
    }

    // ------------------------------------------------------------ Firefox

    /// Profile directories for the current OS, named like a real default profile.
    pub fn firefox_profile(&self, id: &str) -> FirefoxProfile {
        let e = &self.env;
        match e.os {
            Os::Linux => FirefoxProfile {
                data: e.home.join(".mozilla/firefox").join(id),
                cache: e.cache_dir.join("mozilla/firefox").join(id),
            },
            Os::Windows => FirefoxProfile {
                data: e.config_dir.join("Mozilla/Firefox/Profiles").join(id),
                cache: e.data_local_dir.join("Mozilla/Firefox/Profiles").join(id),
            },
            Os::MacOs => FirefoxProfile {
                data: e.config_dir.join("Firefox/Profiles").join(id),
                cache: e.cache_dir.join("Firefox/Profiles").join(id),
            },
        }
    }

    /// `profiles.ini` location for the current OS.
    pub fn firefox_ini_path(&self) -> PathBuf {
        let e = &self.env;
        match e.os {
            Os::Linux => e.home.join(".mozilla/firefox/profiles.ini"),
            Os::Windows => e.config_dir.join("Mozilla/Firefox/profiles.ini"),
            Os::MacOs => e.config_dir.join("Firefox/profiles.ini"),
        }
    }

    /// A Firefox profile with cache (2 files, 30 KiB total), places (history +
    /// bookmarks + downloads), cookies, session, form history and logins.
    pub fn populate_firefox(&self, id: &str) -> FirefoxProfile {
        let p = self.firefox_profile(id);
        // profiles.ini so discovery through the ini works too
        let rel = match self.env.os {
            Os::Linux => id.to_string(),
            _ => format!("Profiles/{id}"),
        };
        let ini = self.firefox_ini_path();
        fs::create_dir_all(ini.parent().unwrap()).unwrap();
        fs::write(
            &ini,
            format!("[Profile0]\nName=default-release\nIsRelative=1\nPath={rel}\nDefault=1\n"),
        )
        .unwrap();

        self.file(p.cache.join("cache2/entries/AAAA1111"), 20 * 1024);
        self.file(p.cache.join("cache2/entries/BBBB2222"), 10 * 1024);
        self.file(p.cache.join("cache2/index"), 0);
        self.sqlite(
            p.data.join("places.sqlite"),
            "CREATE TABLE moz_origins(id INTEGER PRIMARY KEY, prefix TEXT NOT NULL, host TEXT NOT NULL, frecency INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE moz_places(id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR, rev_host LONGVARCHAR,
                visit_count INTEGER DEFAULT 0, hidden INTEGER DEFAULT 0 NOT NULL, typed INTEGER DEFAULT 0 NOT NULL,
                frecency INTEGER DEFAULT -1 NOT NULL, last_visit_date INTEGER, guid TEXT, foreign_count INTEGER DEFAULT 0 NOT NULL,
                origin_id INTEGER);
             CREATE TABLE moz_historyvisits(id INTEGER PRIMARY KEY, from_visit INTEGER, place_id INTEGER, visit_date INTEGER,
                visit_type INTEGER, session INTEGER);
             CREATE TABLE moz_bookmarks(id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER DEFAULT NULL, parent INTEGER,
                position INTEGER, title LONGVARCHAR, dateAdded INTEGER, lastModified INTEGER, guid TEXT);
             CREATE TABLE moz_keywords(id INTEGER PRIMARY KEY AUTOINCREMENT, keyword TEXT UNIQUE, place_id INTEGER, post_data TEXT);
             CREATE TABLE moz_inputhistory(place_id INTEGER NOT NULL, input LONGVARCHAR NOT NULL, use_count INTEGER, PRIMARY KEY (place_id, input));
             CREATE TABLE moz_anno_attributes(id INTEGER PRIMARY KEY, name VARCHAR(32) UNIQUE NOT NULL);
             CREATE TABLE moz_annos(id INTEGER PRIMARY KEY, place_id INTEGER NOT NULL, anno_attribute_id INTEGER,
                content LONGVARCHAR, flags INTEGER DEFAULT 0, expiration INTEGER DEFAULT 0, type INTEGER DEFAULT 0);
             CREATE TRIGGER moz_places_afterdelete_trigger AFTER DELETE ON moz_places
                BEGIN SELECT notify_frecency(OLD.frecency, OLD.url); END;
             INSERT INTO moz_origins VALUES (1,'https://','example.org',100),(2,'https://','news.example.com',50),(3,'https://','bookmarked.example',10);
             INSERT INTO moz_places(id,url,title,visit_count,last_visit_date,guid,foreign_count,origin_id) VALUES
                (1,'https://example.org/','Example',2,10,'g1',0,1),
                (2,'https://news.example.com/','News',1,20,'g2',0,2),
                (3,'https://bookmarked.example/','Bookmarked',1,30,'g3',1,3);
             INSERT INTO moz_historyvisits VALUES (1,0,1,10,1,0),(2,0,1,11,1,0),(3,0,2,20,1,0),(4,0,3,30,1,0);
             INSERT INTO moz_bookmarks VALUES (1,2,NULL,0,0,'root',1,1,'root________'),(2,1,3,1,0,'My bookmark',1,1,'bm1');
             INSERT INTO moz_inputhistory VALUES (1,'exa',2);
             INSERT INTO moz_anno_attributes VALUES (1,'downloads/destinationFileURI'),(2,'downloads/metaData'),(3,'bookmarkProperties/description');
             INSERT INTO moz_annos(id,place_id,anno_attribute_id,content) VALUES (1,1,1,'file:///dl/a.zip'),(2,1,2,'{}'),(3,3,3,'keep this note');",
        );
        self.sqlite(
            p.data.join("cookies.sqlite"),
            "CREATE TABLE moz_cookies(id INTEGER PRIMARY KEY, originAttributes TEXT NOT NULL DEFAULT '', name TEXT, value TEXT,
                host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER, creationTime INTEGER);
             INSERT INTO moz_cookies(name,value,host,path) VALUES ('a','1','.mozilla.org','/'),('b','2','accounts.mozilla.org','/'),
                ('c','3','.ads.example','/'),('d','4','.ads.example','/'),('e','5','notmozilla.org','/');",
        );
        self.sqlite(
            p.data.join("formhistory.sqlite"),
            "CREATE TABLE moz_formhistory(id INTEGER PRIMARY KEY AUTOINCREMENT, fieldname TEXT NOT NULL, value TEXT NOT NULL,
                timesUsed INTEGER, firstUsed INTEGER, lastUsed INTEGER, guid TEXT);
             INSERT INTO moz_formhistory(fieldname,value,timesUsed) VALUES ('email','me@example.org',2),('name','Ada',1);",
        );
        self.file(p.data.join("sessionstore.jsonlz4"), 3000);
        self.file(p.data.join("sessionstore-backups/recovery.jsonlz4"), 2000);
        self.file(p.data.join("sessionstore-backups/previous.jsonlz4"), 1000);
        self.file(p.data.join("logins.json"), 700);
        self.file(p.data.join("key4.db"), 800);
        self.file(p.data.join("prefs.js"), 500);
        p
    }

    // ------------------------------------------------------------ Linux system

    /// Files for the Linux system rules. Ages are relative to "now".
    pub fn populate_linux_system(&self) -> LinuxSystem {
        let e = &self.env;
        let tmp = e.temp_dir.clone();
        let s = LinuxSystem {
            old_tmp: self.aged_file(tmp.join("old-1.tmp"), 8000, 72),
            old_tmp_nested: self.aged_file(tmp.join("build/cache/deep.bin"), 3000, 72),
            new_tmp: self.aged_file(tmp.join("new.tmp"), 500, 1),
            lock_file: self.aged_file(tmp.join("app.lock"), 10, 72),
            x11_socket_dir_file: self.aged_file(tmp.join(".X11-unix/keep"), 10, 72),
            var_tmp_old: self.aged_file(e.sys_path("/var/tmp/old-var.tmp"), 1200, 100),
            trash_file: self.file(e.user_data_dir.join("Trash/files/deleted.txt"), 2500),
            trash_info: self.file(
                e.user_data_dir.join("Trash/info/deleted.txt.trashinfo"),
                100,
            ),
            trash_dir_file: self.file(e.user_data_dir.join("Trash/files/folder/inner.txt"), 400),
            thumbnail: self.file(e.cache_dir.join("thumbnails/normal/abc.png"), 6000),
            recent: self.file(e.user_data_dir.join("recently-used.xbel"), 900),
            apt_deb: self.file(
                e.sys_path("/var/cache/apt/archives/foo_1.0_amd64.deb"),
                50_000,
            ),
            apt_partial: self.file(e.sys_path("/var/cache/apt/archives/partial/bar.deb"), 700),
            apt_lock: self.file(e.sys_path("/var/cache/apt/archives/lock"), 0),
            log_active: self.file(e.sys_path("/var/log/syslog"), 4000),
            log_rot1: self.file(e.sys_path("/var/log/syslog.1"), 3000),
            log_gz: self.file(e.sys_path("/var/log/syslog.2.gz"), 1000),
            log_old: self.file(e.sys_path("/var/log/auth.log.old"), 800),
            crash: self.file(e.sys_path("/var/crash/_usr_bin_foo.1000.crash"), 5000),
        };
        // A directory's mtime changes when entries are added; age the ones that only
        // hold old files so "empty directory removal respects age" is realistic.
        set_age_hours(&tmp.join("build/cache"), 72);
        set_age_hours(&tmp.join("build"), 72);
        s
    }

    // ------------------------------------------------------------ everything

    /// A "typical machine" for the current OS used by the end-to-end sandbox.
    pub fn populate_typical(&self) {
        self.populate_chromium(Chromium::Chrome, "Default");
        self.populate_chromium(Chromium::Chrome, "Profile 1");
        self.populate_chromium(Chromium::Edge, "Default");
        self.populate_chromium(Chromium::Brave, "Default");
        self.populate_firefox("abcd1234.default-release");
        if self.env.os == Os::Linux {
            self.populate_linux_system();
        } else {
            self.aged_file(self.env.temp_dir.join("old.tmp"), 8000, 72);
        }
    }
}

/// Paths created by [`Fixture::populate_linux_system`].
#[derive(Debug, Clone)]
pub struct LinuxSystem {
    pub old_tmp: PathBuf,
    pub old_tmp_nested: PathBuf,
    pub new_tmp: PathBuf,
    pub lock_file: PathBuf,
    pub x11_socket_dir_file: PathBuf,
    pub var_tmp_old: PathBuf,
    pub trash_file: PathBuf,
    pub trash_info: PathBuf,
    pub trash_dir_file: PathBuf,
    pub thumbnail: PathBuf,
    pub recent: PathBuf,
    pub apt_deb: PathBuf,
    pub apt_partial: PathBuf,
    pub apt_lock: PathBuf,
    pub log_active: PathBuf,
    pub log_rot1: PathBuf,
    pub log_gz: PathBuf,
    pub log_old: PathBuf,
    pub crash: PathBuf,
}

/// Run a single-value integer query against a database file (test assertions).
pub fn query_i64(db: &Path, sql: &str) -> i64 {
    Connection::open(db)
        .expect("open db")
        .query_row(sql, [], |r| r.get(0))
        .expect("query")
}

/// Set a file's (or symlink target's) mtime/atime to `hours` ago.
pub fn set_age_hours(path: &Path, hours: u64) {
    let t = SystemTime::now() - Duration::from_secs(hours * 3600);
    let ft = FileTime::from_system_time(t);
    filetime::set_symlink_file_times(path, ft, ft).expect("set file times");
}

/// Set a file's modification and access times independently (`hours` ago each).
pub fn set_mtime_atime_hours(path: &Path, mtime_hours: u64, atime_hours: u64) {
    let ago =
        |h: u64| FileTime::from_system_time(SystemTime::now() - Duration::from_secs(h * 3600));
    filetime::set_symlink_file_times(path, ago(atime_hours), ago(mtime_hours))
        .expect("set file times");
}

/// Remove everything below `dir` (but keep `dir`).
pub fn clear_dir(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let _ = fs::remove_dir_all(&p);
            } else {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

// ---------------------------------------------------------------- snapshots

/// Read a file without touching its access time where the OS allows it: the cleaner treats a
/// recently read file as in use, so a test that snapshots the tree must not make every file
/// look freshly read.
fn read_no_atime(path: &Path) -> Vec<u8> {
    use std::io::Read;
    #[cfg(target_os = "linux")]
    let opened = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOATIME)
            .open(path)
            .or_else(|_| fs::File::open(path))
    };
    #[cfg(not(target_os = "linux"))]
    let opened = fs::File::open(path);
    let mut buf = Vec::new();
    if let Ok(mut f) = opened {
        let _ = f.read_to_end(&mut buf);
    }
    buf
}

/// One line per entry under `root`: type, size, content hash, mtime. Symlinks are not
/// followed. Two snapshots are equal iff the tree is byte-for-byte unchanged.
pub fn tree_snapshot(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for e in walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
    {
        let e = e.expect("walk");
        let rel = e
            .path()
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let md = fs::symlink_metadata(e.path()).expect("stat");
        let mtime = md
            .modified()
            .ok()
            .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let desc = if md.file_type().is_symlink() {
            format!("link -> {}", fs::read_link(e.path()).unwrap().display())
        } else if md.is_dir() {
            "dir".to_string()
        } else {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            read_no_atime(e.path()).hash(&mut h);
            format!("file {} {:x} {mtime}", md.len(), h.finish())
        };
        out.insert(rel, desc);
    }
    out
}
