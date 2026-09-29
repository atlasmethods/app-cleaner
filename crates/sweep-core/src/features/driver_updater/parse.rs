//! Parsers and PowerShell scripts for the driver updater.

use serde_json::Value;

use crate::pkgutil::valid_pkg_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DSource {
    #[serde(rename = "fwupd")]
    Fwupd,
    #[serde(rename = "ubuntu-drivers")]
    UbuntuDrivers,
    #[serde(rename = "windows-update")]
    WindowsUpdate,
    #[serde(rename = "macos")]
    Macos,
}

impl DSource {
    pub fn prefix(self) -> &'static str {
        match self {
            DSource::Fwupd => "fwupd",
            DSource::UbuntuDrivers => "ubuntu-drivers",
            DSource::WindowsUpdate => "windows-update",
            DSource::Macos => "macos",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverEntry {
    pub id: String,
    pub device_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    pub source: DSource,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reboot_required: Option<bool>,
}

impl DriverEntry {
    pub fn new(source: DSource, key: &str, device_name: &str, description: &str) -> DriverEntry {
        DriverEntry {
            id: format!("{}:{}", source.prefix(), key),
            device_name: device_name.to_string(),
            current_version: None,
            new_version: None,
            vendor: None,
            source,
            description: description.to_string(),
            reboot_required: None,
        }
    }
    /// The part of the id after `<source>:`.
    pub fn key(&self) -> &str {
        self.id.split_once(':').map(|x| x.1).unwrap_or(&self.id)
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
}

/// Remove markup from fwupd's AppStream descriptions (`<p>..</p><ul><li>..</li></ul>`).
pub fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_flag(v: &Value, flag: &str) -> bool {
    v.get("Flags")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|f| f.as_str() == Some(flag)))
}

/// `fwupdmgr get-updates --json`. Exit status 2 ("nothing to do") is handled by the caller;
/// this parses the success case. Text that is not JSON yields `Err`.
pub fn parse_fwupd(out: &str) -> Result<Vec<DriverEntry>, String> {
    let t = out.trim();
    if t.is_empty() {
        return Ok(Vec::new());
    }
    let v: Value =
        serde_json::from_str(t).map_err(|e| format!("unexpected fwupdmgr output: {e}"))?;
    let mut list = Vec::new();
    for d in v
        .get("Devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = s(d, "DeviceId") else { continue };
        if !valid_pkg_name(&id) {
            continue;
        }
        let releases = d.get("Releases").and_then(Value::as_array);
        let Some(rel) = releases.and_then(|r| {
            r.iter()
                .find(|x| has_flag(x, "is-upgrade"))
                .or_else(|| r.first())
        }) else {
            continue;
        };
        let name = s(d, "Name").unwrap_or_else(|| id.clone());
        let desc = s(rel, "Summary")
            .or_else(|| s(rel, "Description").map(|h| strip_tags(&h)))
            .or_else(|| s(d, "Summary"))
            .unwrap_or_else(|| "Firmware update".to_string());
        let mut e = DriverEntry::new(DSource::Fwupd, &id, &name, &desc);
        e.current_version = s(d, "Version");
        e.new_version = s(rel, "Version");
        e.vendor = s(rel, "Vendor").or_else(|| s(d, "Vendor"));
        let reboot = has_flag(d, "needs-reboot")
            || has_flag(d, "needs-shutdown")
            || has_flag(rel, "needs-reboot")
            || has_flag(rel, "needs-shutdown");
        e.reboot_required = Some(reboot);
        list.push(e);
    }
    Ok(list)
}

/// A recommended proprietary driver from `ubuntu-drivers devices`.
#[derive(Debug, Clone, PartialEq)]
pub struct UbuntuDevice {
    pub vendor: String,
    pub model: String,
    pub recommended: String,
}

/// `ubuntu-drivers devices`: blocks headed `== /sys/... ==` with `vendor :`, `model :` and
/// `driver : <pkg> - <notes>` lines; the recommended driver is tagged `recommended`.
pub fn parse_ubuntu_devices(out: &str) -> Vec<UbuntuDevice> {
    let mut v = Vec::new();
    let mut cur: Option<UbuntuDevice> = None;
    let flush = |cur: &mut Option<UbuntuDevice>, v: &mut Vec<UbuntuDevice>| {
        if let Some(d) = cur.take() {
            if !d.recommended.is_empty() {
                v.push(d);
            }
        }
    };
    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("== ") && t.ends_with(" ==") {
            flush(&mut cur, &mut v);
            cur = Some(UbuntuDevice {
                vendor: String::new(),
                model: String::new(),
                recommended: String::new(),
            });
            continue;
        }
        let Some(d) = cur.as_mut() else { continue };
        let Some((k, val)) = t.split_once(':') else {
            continue;
        };
        let (k, val) = (k.trim(), val.trim());
        match k {
            "vendor" => d.vendor = val.to_string(),
            "model" => d.model = val.to_string(),
            "driver" => {
                // "nvidia-driver-535 - distro non-free recommended"
                let (pkg, notes) = val.split_once(" - ").unwrap_or((val, ""));
                let pkg = pkg.trim();
                if notes.split_whitespace().any(|w| w == "recommended") && valid_pkg_name(pkg) {
                    d.recommended = pkg.to_string();
                }
            }
            _ => {}
        }
    }
    flush(&mut cur, &mut v);
    v
}

// ---------------------------------------------------------------- Windows Update

pub const WIN_SEARCH_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
$session = New-Object -ComObject Microsoft.Update.Session
$result = $session.CreateUpdateSearcher().Search("IsInstalled=0 and Type='Driver'")
$out = @()
foreach ($u in $result.Updates) {
  $out += [pscustomobject]@{
    Title = [string]$u.Title
    DriverModel = [string]$u.DriverModel
    DriverVerDate = $u.DriverVerDate.ToString('yyyy-MM-dd')
    DriverProvider = [string]$u.DriverProvider
    DriverManufacturer = [string]$u.DriverManufacturer
    UpdateID = [string]$u.Identity.UpdateID
    RebootRequired = [bool]$u.RebootRequired
  }
}
ConvertTo-Json -InputObject @($out) -Compress
"#;

/// GUID (`UpdateID`) check; these are embedded in a PowerShell script.
pub fn valid_update_id(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

pub fn win_install_script(update_ids: &[String]) -> Result<String, String> {
    if update_ids.is_empty() || !update_ids.iter().all(|i| valid_update_id(i)) {
        return Err("invalid update id".into());
    }
    let list = update_ids
        .iter()
        .map(|i| format!("'{}'", i.to_lowercase()))
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        r#"$ErrorActionPreference = 'Stop'
$ids = @({list})
$session = New-Object -ComObject Microsoft.Update.Session
$result = $session.CreateUpdateSearcher().Search("IsInstalled=0 and Type='Driver'")
$coll = New-Object -ComObject Microsoft.Update.UpdateColl
foreach ($u in $result.Updates) {{
  if ($ids -contains ([string]$u.Identity.UpdateID).ToLower()) {{
    if (-not $u.EulaAccepted) {{ $u.AcceptEula() }}
    [void]$coll.Add($u)
  }}
}}
if ($coll.Count -eq 0) {{
  '{{"ResultCode":0,"RebootRequired":false,"Updates":[]}}'
  exit 0
}}
$dl = $session.CreateUpdateDownloader()
$dl.Updates = $coll
[void]$dl.Download()
$inst = $session.CreateUpdateInstaller()
$inst.Updates = $coll
$res = $inst.Install()
$per = @()
for ($k = 0; $k -lt $coll.Count; $k++) {{
  $ur = $res.GetUpdateResult($k)
  $per += [pscustomobject]@{{
    UpdateID = [string]$coll.Item($k).Identity.UpdateID
    ResultCode = [int]$ur.ResultCode
    HResult = [int]$ur.HResult
    RebootRequired = [bool]$ur.RebootRequired
  }}
}}
ConvertTo-Json -InputObject ([pscustomobject]@{{
  ResultCode = [int]$res.ResultCode
  RebootRequired = [bool]$res.RebootRequired
  Updates = @($per)
}}) -Compress -Depth 4
"#
    ))
}

/// Windows PowerShell's `ConvertTo-Json` emits an array, a lone object, or nothing.
fn json_items(t: &str) -> Result<Vec<Value>, String> {
    let t = t.trim().trim_start_matches('\u{feff}');
    if t.is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<Value>(t)
        .map_err(|e| format!("unexpected PowerShell output: {e}"))?
    {
        Value::Array(a) => Ok(a),
        Value::Null => Ok(Vec::new()),
        o @ Value::Object(_) => Ok(vec![o]),
        _ => Err("unexpected PowerShell output".into()),
    }
}

/// `/Date(1700000000000)/` (PowerShell 5.1 serialisation) or an ISO string -> `YYYY-MM-DD`.
fn normalise_date(s: &str) -> Option<String> {
    if let Some(ms) = s
        .strip_prefix("/Date(")
        .and_then(|r| r.split([')', '+', '-']).next())
        .and_then(|n| n.parse::<i64>().ok())
    {
        return crate::pkgutil::date_from_unix(ms / 1000);
    }
    let d = s.get(..10)?;
    let ok = d.len() == 10
        && d.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    ok.then(|| d.to_string())
}

/// Version-looking suffix of a driver title: `Intel - Net - 22.1.0.4` -> `22.1.0.4`.
fn version_from_title(title: &str) -> Option<String> {
    title
        .split_whitespace()
        .rev()
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.'))
        .find(|w| {
            w.contains('.')
                && w.chars().all(|c| c.is_ascii_digit() || c == '.')
                && w.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
        .map(str::to_string)
}

/// Output of [`WIN_SEARCH_SCRIPT`].
pub fn parse_windows_drivers(out: &str) -> Result<Vec<DriverEntry>, String> {
    let mut v = Vec::new();
    for it in json_items(out)? {
        let Some(id) = s(&it, "UpdateID") else {
            continue;
        };
        if !valid_update_id(&id) {
            continue;
        }
        let title = s(&it, "Title").unwrap_or_default();
        let device = s(&it, "DriverModel").unwrap_or_else(|| title.clone());
        let date = s(&it, "DriverVerDate").and_then(|d| normalise_date(&d));
        let mut e = DriverEntry::new(DSource::WindowsUpdate, &id.to_lowercase(), &device, &title);
        e.vendor = s(&it, "DriverProvider").or_else(|| s(&it, "DriverManufacturer"));
        e.new_version = version_from_title(&title).or(date);
        e.reboot_required = it.get("RebootRequired").and_then(Value::as_bool);
        v.push(e);
    }
    Ok(v)
}

#[derive(Debug, Clone, PartialEq)]
pub struct WinInstall {
    pub reboot_required: bool,
    /// (update id lower-cased, result code, hresult, reboot)
    pub updates: Vec<(String, i64, i64, bool)>,
}

/// Output of [`win_install_script`].
pub fn parse_windows_install(out: &str) -> Result<WinInstall, String> {
    let items = json_items(out)?;
    let Some(v) = items.into_iter().next() else {
        return Err("PowerShell returned no result".into());
    };
    let per = v
        .get("Updates")
        .map(|u| json_items(&u.to_string()))
        .transpose()?
        .unwrap_or_default();
    Ok(WinInstall {
        reboot_required: v
            .get("RebootRequired")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        updates: per
            .iter()
            .filter_map(|u| {
                Some((
                    s(u, "UpdateID")?.to_lowercase(),
                    u.get("ResultCode").and_then(Value::as_i64).unwrap_or(-1),
                    u.get("HResult").and_then(Value::as_i64).unwrap_or(0),
                    u.get("RebootRequired")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ))
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured shape of `fwupdmgr get-updates --json` (fwupd 1.9, trimmed).
    const FWUPD: &str = r#"{
  "Devices" : [
    {
      "Name" : "System Firmware",
      "DeviceId" : "3fc0a2fa8d0ff8ec5b4d0d4dbbc91ba6a3d1d1d1",
      "InstanceIds" : ["4b8fc1b1-1a47-5d0d-a0aa-58a0f5a8b3f4"],
      "Guid" : ["4b8fc1b1-1a47-5d0d-a0aa-58a0f5a8b3f4"],
      "Summary" : "UEFI System Resource Table device",
      "Plugin" : "uefi_capsule",
      "Flags" : ["internal","updatable","require-ac","supported","registered","needs-reboot"],
      "Vendor" : "Dell Inc.",
      "VendorId" : "DMI:Dell Inc.",
      "Version" : "1.20.0",
      "VersionFormat" : "triplet",
      "Created" : 1712345678,
      "Releases" : [
        {
          "AppstreamId" : "com.dell.uefi3fc0a2fa.firmware",
          "RemoteId" : "lvfs",
          "Summary" : "Firmware for the Dell XPS 13 9310",
          "Description" : "<p>Fixes:</p><ul><li>Improved   stability</li><li>Security fix</li></ul>",
          "Version" : "1.22.0",
          "Vendor" : "Dell",
          "License" : "proprietary",
          "Size" : 12345678,
          "Created" : 1714000000,
          "Urgency" : "high",
          "Locations" : ["https://fwupd.org/downloads/x.cab"],
          "Flags" : ["is-upgrade"]
        },
        {
          "Version" : "1.21.0",
          "Flags" : ["is-upgrade"]
        }
      ]
    },
    {
      "Name" : "UEFI dbx",
      "DeviceId" : "362301da643102b9f38477387e2193e57abaa590",
      "Flags" : ["internal","updatable","supported","registered"],
      "Vendor" : "UEFI:Microsoft",
      "Version" : "77",
      "Releases" : [
        {
          "Description" : "<p>Update the list of forbidden signatures.</p>",
          "Version" : "371"
        }
      ]
    },
    {
      "Name" : "No release device",
      "DeviceId" : "aaaa",
      "Releases" : []
    }
  ]
}
"#;

    #[test]
    fn fwupd_json() {
        let v = parse_fwupd(FWUPD).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "fwupd:3fc0a2fa8d0ff8ec5b4d0d4dbbc91ba6a3d1d1d1");
        assert_eq!(v[0].device_name, "System Firmware");
        assert_eq!(v[0].current_version.as_deref(), Some("1.20.0"));
        assert_eq!(v[0].new_version.as_deref(), Some("1.22.0"));
        assert_eq!(v[0].vendor.as_deref(), Some("Dell"));
        assert_eq!(v[0].description, "Firmware for the Dell XPS 13 9310");
        assert_eq!(v[0].reboot_required, Some(true));
        assert_eq!(v[0].key(), "3fc0a2fa8d0ff8ec5b4d0d4dbbc91ba6a3d1d1d1");
        assert_eq!(v[1].reboot_required, Some(false));
        assert_eq!(v[1].description, "Update the list of forbidden signatures.");
        assert_eq!(v[1].vendor.as_deref(), Some("UEFI:Microsoft"));
    }

    #[test]
    fn fwupd_edge_cases() {
        assert!(parse_fwupd("").unwrap().is_empty());
        assert!(parse_fwupd("{}").unwrap().is_empty());
        assert!(parse_fwupd(r#"{"Devices":[]}"#).unwrap().is_empty());
        assert!(parse_fwupd("No updatable devices").is_err());
        // hostile device id
        assert!(
            parse_fwupd(r#"{"Devices":[{"DeviceId":"-x","Releases":[{"Version":"1"}]}]}"#)
                .unwrap()
                .is_empty()
        );
        assert_eq!(strip_tags("<p>a</p><ul><li>b   c</li></ul>"), "a b c");
    }

    /// `ubuntu-drivers devices` on Ubuntu 22.04 with an NVIDIA GPU.
    const UBUNTU: &str = "\
== /sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0 ==
modalias : pci:v000010DEd00002504sv00001462sd0000397Dbc03sc00i00
vendor   : NVIDIA Corporation
model    : GA106 [GeForce RTX 3060 Lite Hash Rate]
driver   : nvidia-driver-535 - distro non-free recommended
driver   : nvidia-driver-470 - distro non-free
driver   : nvidia-driver-535-open - distro non-free
driver   : xserver-xorg-video-nouveau - distro free builtin

== /sys/devices/pci0000:00/0000:00:1c.2/0000:03:00.0 ==
modalias : pci:v000014E4d000043A0sv00000000sd00000000bc02sc80i00
vendor   : Broadcom Inc. and subsidiaries
model    : BCM4360 802.11ac Dual Band Wireless Network Adapter
driver   : bcmwl-kernel-source - distro non-free
";

    #[test]
    fn ubuntu_drivers_devices() {
        let v = parse_ubuntu_devices(UBUNTU);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].recommended, "nvidia-driver-535");
        assert_eq!(v[0].vendor, "NVIDIA Corporation");
        assert_eq!(v[0].model, "GA106 [GeForce RTX 3060 Lite Hash Rate]");
        assert!(parse_ubuntu_devices("").is_empty());
        assert!(parse_ubuntu_devices("No drivers found for installed devices.\n").is_empty());
    }

    #[test]
    fn windows_search_output_shapes() {
        let arr = r#"[{"Title":"Intel - Net - 22.1.0.4","DriverModel":"Intel(R) Wi-Fi 6 AX201 160MHz","DriverVerDate":"2023-05-01","DriverProvider":"Intel","DriverManufacturer":"Intel Corporation","UpdateID":"A1B2C3D4-0000-1111-2222-333344445555","RebootRequired":false},{"Title":"Realtek - Audio","DriverModel":"Realtek Audio","DriverVerDate":"/Date(1690000000000)/","DriverProvider":"Realtek","UpdateID":"a1b2c3d4-0000-1111-2222-333344445556","RebootRequired":true}]"#;
        let v = parse_windows_drivers(arr).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[0].id,
            "windows-update:a1b2c3d4-0000-1111-2222-333344445555"
        );
        assert_eq!(v[0].device_name, "Intel(R) Wi-Fi 6 AX201 160MHz");
        assert_eq!(v[0].new_version.as_deref(), Some("22.1.0.4"));
        assert_eq!(v[0].vendor.as_deref(), Some("Intel"));
        assert_eq!(v[0].reboot_required, Some(false));
        assert_eq!(v[1].new_version.as_deref(), Some("2023-07-22"));
        assert_eq!(v[1].reboot_required, Some(true));
        // single object (PowerShell unwraps one-element arrays)
        let one =
            r#"{"Title":"X","DriverModel":"","UpdateID":"a1b2c3d4-0000-1111-2222-333344445555"}"#;
        let v = parse_windows_drivers(one).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].device_name, "X");
        // nothing / empty array / BOM
        assert!(parse_windows_drivers("").unwrap().is_empty());
        assert!(parse_windows_drivers("[]").unwrap().is_empty());
        assert!(parse_windows_drivers("\u{feff}[]\r\n").unwrap().is_empty());
        // invalid ids are dropped, garbage is an error
        assert!(
            parse_windows_drivers(r#"[{"Title":"x","UpdateID":"'; calc; '"}]"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse_windows_drivers("Access denied").is_err());
    }

    #[test]
    fn windows_install_script_and_result() {
        assert!(win_install_script(&[]).is_err());
        assert!(win_install_script(&["'; calc; '".to_string()]).is_err());
        let ids = vec!["A1B2C3D4-0000-1111-2222-333344445555".to_string()];
        let s = win_install_script(&ids).unwrap();
        assert!(
            s.contains("$ids = @('a1b2c3d4-0000-1111-2222-333344445555')"),
            "{s}"
        );
        assert!(s.contains("CreateUpdateInstaller"));
        let r = parse_windows_install(r#"{"ResultCode":2,"RebootRequired":true,"Updates":[{"UpdateID":"A1B2C3D4-0000-1111-2222-333344445555","ResultCode":2,"HResult":0,"RebootRequired":true}]}"#).unwrap();
        assert!(r.reboot_required);
        assert_eq!(
            r.updates,
            vec![(
                "a1b2c3d4-0000-1111-2222-333344445555".to_string(),
                2,
                0,
                true
            )]
        );
        // one-element Updates unwrapped by PowerShell
        let r = parse_windows_install(r#"{"ResultCode":4,"RebootRequired":false,"Updates":{"UpdateID":"a1b2c3d4-0000-1111-2222-333344445555","ResultCode":4,"HResult":-2145116147}}"#).unwrap();
        assert_eq!(r.updates[0].1, 4);
        assert_eq!(r.updates[0].2, -2145116147);
        assert!(parse_windows_install("").is_err());
        assert!(parse_windows_install("boom").is_err());
    }

    #[test]
    fn update_ids() {
        assert!(valid_update_id("a1b2c3d4-0000-1111-2222-333344445555"));
        assert!(!valid_update_id("a1b2c3d4-0000-1111-2222-33334444555"));
        assert!(!valid_update_id("g1b2c3d4-0000-1111-2222-333344445555"));
    }
}
