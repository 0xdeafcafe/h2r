//! platform/xbox/deploy.sh and its Device Portal helpers (portal.sh),
//! against a stand-in curl and keychain: addresses, the CSRF cookie and
//! header, the netrc file, the install, poll and launch requests, and the
//! AUMID lookup. Needs sh, jq, awk and base64; no network.

use std::path::Path;
use std::process::Command;

#[test]
fn the_deploy_script_builds_the_portal_requests() {
    if Command::new("jq").arg("--version").output().is_err() {
        eprintln!("skipped: no jq");
        return;
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../platform/xbox/tests/portal_test.sh");
    let o = Command::new("sh").arg(&script).output().unwrap();
    assert!(
        o.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("portal_test: ok"));
}
