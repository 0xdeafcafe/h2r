//! Native stamps identify executable modules. The portable contract identifies
//! the guest instructions and execution options of a verified Clang module.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GuestImage {
    pub version: u32,
    pub target: String,
    /// Absent for unsupported tiers and builds using an external host module.
    pub contract_sha256: Option<String>,
}

impl GuestImage {
    pub fn capture(stamp: &str, target: &str, xbe_sha256: &str) -> Self {
        Self {
            version: 1,
            target: target.to_owned(),
            contract_sha256: clang_contract(stamp, target, xbe_sha256),
        }
    }
}

fn clang_contract(stamp: &str, target: &str, xbe_sha256: &str) -> Option<String> {
    let suffix = match target {
        "arm64-apple-macos11" | "x86_64-apple-macos11" => "",
        "x86_64-unknown-linux-gnu" => " target x86_64-unknown-linux-gnu",
        _ => return None,
    };
    if xbe_sha256.len() != 64 || !xbe_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut lines = stamp.strip_suffix('\n')?.split('\n');
    let line = lines.next()?.strip_suffix(suffix)?;
    let (guest, flags) = line.split_once(" flags ")?;
    if !guest.starts_with("xbe ")
        || !guest.contains(" generator ")
        || !guest.contains(" passes ")
        || !guest.contains(" codegen ")
        || !guest.contains(" abi ")
    {
        return None;
    }
    if let Some(line) = lines.next() {
        let id = line.strip_prefix("host coverage ")?;
        if lines.next().is_some() || id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
    }
    let mut flags: Vec<String> = serde_json::from_str(flags).ok()?;
    // ELF linkage/visibility flags are the only native flags this contract
    // erases. Preserve optimisation, tiers, PGO, x87/SSE, FP contraction,
    // fast memory, charging, compiler defines and any future unknown flag.
    flags.retain(|flag| !matches!(flag.as_str(), "-fPIC" | "-fvisibility=hidden"));
    if !flags.iter().any(|f| f == "-fno-fast-math")
        || !flags.iter().any(|f| f == "-ffp-contract=off")
    {
        return None;
    }
    let wire =
        serde_json::to_vec(&("h2film guest image v1 clang", xbe_sha256, guest, flags)).ok()?;
    Some(crate::xbe_digest(&wire))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_guest_on_another_target_has_the_same_contract() {
        let xbe = "ab".repeat(32);
        let mac = "xbe A generator B passes P codegen C abi 1 flags [\"-fno-fast-math\", \"-ffp-contract=off\", \"-O2\"]\n";
        let linux = mac
            .replace("\"-O2\"", "\"-fPIC\", \"-fvisibility=hidden\", \"-O2\"")
            .replace(
                "]\n",
                "] target x86_64-unknown-linux-gnu\nhost coverage 0123456789abcdef\n",
            );
        let arm = "arm64-apple-macos11";
        let elf = "x86_64-unknown-linux-gnu";
        let contract = |stamp: &str, target: &str, xbe: &str| {
            GuestImage::capture(stamp, target, xbe).contract_sha256
        };
        let recorded = contract(&linux, elf, &xbe);
        assert!(recorded.is_some());
        assert_eq!(contract(mac, arm, &xbe), recorded);
        assert_ne!(contract(&mac.replace("-O2", "-O3"), arm, &xbe), recorded);
        assert_ne!(contract(&mac.replace("passes P", "passes X"), arm, &xbe), recorded);
        assert_ne!(contract(mac, arm, &"fe".repeat(32)), recorded);
        assert_eq!(contract(&mac.replace("-fno-fast-math", "-ffast-math"), arm, &xbe), None);
        assert_eq!(contract(mac, "aarch64-unknown-linux-gnu", &xbe), None);
        assert_eq!(contract(&linux.replace("0123456789abcdef", "22"), elf, &xbe), None);
    }
}
