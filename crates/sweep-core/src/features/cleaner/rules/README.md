# ClearSweep cleaning rules

Rules are ClearSweep's own declarative TOML files, embedded in the binary with
`include_str!` (see `../rules.rs`). Nothing here is derived from any third-party rule
database. The unit tests parse and validate every file (`cargo test -p sweep-core rules`),
so a malformed rule fails CI instead of shipping.

Files: `browsers.toml`, `system_linux.toml`, `system_windows.toml`, `system_macos.toml`,
`apps.toml`. A file holds any number of `[[rule]]` tables; each rule has `[[rule.targets]]`.

## Rule

```toml
[[rule]]
id = "chrome.cache"            # unique, lowercase, "group.name" (used by settings + API)
name = "Internet Cache"        # label shown in the UI
group = "Google Chrome"        # rules with the same group are shown together
category = "browser"           # "browser" | "system" | "application"
os = ["linux", "windows", "macos"]   # every listed OS needs at least one target
defaultEnabled = true          # selection when the user has not chosen (settings.selectedRules == null)
description = "..."            # one plain sentence
warning = "..."                # optional; the UI asks for confirmation before enabling
processes = { linux = ["chrome"], windows = ["chrome.exe"], macos = ["Google Chrome"] }
```

`processes` are the process names that mean the app is running (case-insensitive; Linux
truncates names to 15 bytes, which is handled). A rule whose app is running is reported
with `appRunning` by analysis and, when cleaning, is skipped, or the app is asked to
quit (never force-killed), depending on the "close browsers" setting.

Rules that delete credentials or session state must have `defaultEnabled = false` and a
`warning`; a unit test enforces this for ids ending in `.passwords` and for `.session`.

## Targets

Every target may carry `os = [...]` (subset of the rule's `os`); without it the target
applies to all OSes of the rule.

| kind | fields | effect |
|------|--------|--------|
| `files` | `base`, `pattern="*"`, `recursive=false`, `minAgeHours`, `minAgeFromSettings=false`, `removeEmptyDirs=false`, `keepBase=true`, `ownedByUser=false`, `skipNames=[]` | delete files under `base` whose *name* matches `pattern` |
| `file` | `path` | delete a single file (globs allowed) |
| `sqlite` | `db`, `statements=[..]`, `countQuery` | run `DELETE`/`UPDATE` statements in one transaction, then `VACUUM` |
| `cookies` | `browserFamily = "chromium" \| "firefox"`, `db` | delete cookie rows except the keep-list domains |
| `command` | `program`, `args=[..]`, `label` | run only if the program exists (clipboard, DNS flush); reported as an action |
| `trash` | | the OS trash (XDG trash, `~/.Trash`, Windows Recycle Bin via PowerShell) |
| `registry` | `key`, `values`, `subkeys` | Windows only; the key must be under `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\` |

Details that matter for safety:

* `files`: symlinks found while walking are removed as links and never followed; other
  file systems are not entered; sockets, pipes and devices are never touched. `minAge*`
  compares the modification time, and also applies to `removeEmptyDirs`. `skipNames` are
  globs matched against entry names at any depth (a matching directory is skipped whole).
  `ownedByUser` limits deletion to entries owned by the current user (Unix).
* The `base` directory itself, and any path outside it, is never deleted. A `base` (or
  glob match) that is a symlink is skipped and reported.
* `sqlite`: statements must be `DELETE FROM ...` or `UPDATE ...`. A statement that fails
  with "no such table/column" is skipped (browser schema differences); any other error
  rolls the whole transaction back. A locked database is reported as `in use`. Analysis
  opens databases `immutable=1`, so it never touches them. `countQuery` must be a single
  value `SELECT` giving the rows the statements remove (reported as `rows`).
* Rules never take paths from clients: `cleaner.clean` re-scans on the server.

## Templates

`base`, `path` and `db` are templates: `{variable}/component/...` where the variable comes
first and components are separated by `/` (also on Windows). Components may contain glob
characters (`*`, `?`, `[...]`, `{a,b}`) to match profile directories, e.g.
`{config}/google-chrome/*/Cache`. Glob-matched directories must be real directories.

| variable | meaning |
|----------|---------|
| `{home}` | user home |
| `{config}` | `~/.config`, `~/Library/Application Support`, `%APPDATA%` (roaming) |
| `{cache}` | `~/.cache`, `~/Library/Caches`, `%LOCALAPPDATA%` |
| `{data}` | `~/.local/share`, `~/Library/Application Support`, `%APPDATA%` |
| `{temp}` | the user temp folder |
| `{sys}` | the system root (`/`, `C:\`); use as `{sys}/var/cache/apt/archives` |
| `{appdata}` / `{localappdata}` | Windows only |
| `{library}` | macOS only (`~/Library`) |
| `{firefox_profile}` | every Firefox profile directory (from `profiles.ini`, plus a fallback scan) |

Templates are expanded only from the process environment (`Env`), never from user input.
Validation rejects unknown variables, `..`, empty components and OS-only variables used in
targets that also run elsewhere.

## Adding a rule

1. Add the `[[rule]]` to the right file, with targets for every OS listed.
2. Prefer narrow globs over broad ones; never target a user's documents or a whole
   config directory.
3. `cargo test -p sweep-core` validates the file and its safety properties.
