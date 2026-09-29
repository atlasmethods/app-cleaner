use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clearsweep"))
}

#[test]
fn call_sysinfo_prints_json() {
    let out = bin().args(["call", "sysinfo.get"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["cpu"]["cores"].as_u64().unwrap() >= 1);
}

#[test]
fn call_unknown_method_exits_1_with_error_json() {
    let out = bin().args(["call", "nope.nothing"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["code"], "NotFound");
}

#[test]
fn call_bad_params_json_exits_1() {
    let out = bin()
        .args(["call", "api.methods", "{oops"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["code"], "InvalidParams");
}

#[test]
fn stubs_exit_2() {
    for args in [&["clean", "--auto"][..], &["analyze"][..]] {
        let out = bin().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(v["code"], "NotImplemented");
    }
}
