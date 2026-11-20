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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageMatch {
    Native,
    /// Re-execute from the start manifest. Native machine snapshots cannot
    /// be assumed to have the same runtime layout or code coverage.
    FromBoot,
}

impl GuestImage {
    pub fn capture(stamp: &str, target: &str, xbe_sha256: &str) -> Self {
        Self {
            version: 1,
            target: target.to_owned(),
            contract_sha256: clang_contract(stamp, target, xbe_sha256).map(|(hash, _)| hash),
        }
    }

    pub fn compare(
        &self,
        original: &str,
        candidate: &str,
        target: &str,
        xbe_sha256: &str,
    ) -> Result<ImageMatch, &'static str> {
        if original == candidate && !candidate.is_empty() && self.target == target {
            return Ok(ImageMatch::Native);
        }
        if self.version != 1 {
            return Err("unsupported portable guest image stamp version");
        }
        let (recorded, subset) = clang_contract(original, &self.target, xbe_sha256)
            .ok_or("recording requires its original native image and target")?;
        if Some(&recorded) != self.contract_sha256.as_ref() {
            return Err("portable guest image stamp disagrees with the recorded native stamp");
        }
        let (local, local_subset) = clang_contract(candidate, target, xbe_sha256)
            .ok_or("portable replay requires a supported Clang image on macOS or Linux")?;
        if recorded != local {
            return Err("guest image, generator, ABI, IR, FP or compiler options differ");
        }
        if local_subset.is_some() && local_subset != subset {
            return Err("the local slim image has different guest code coverage; use a full image");
        }
        Ok(ImageMatch::FromBoot)
    }
}

fn clang_contract<'a>(
    stamp: &'a str,
    target: &str,
    xbe_sha256: &str,
) -> Option<(String, Option<&'a str>)> {
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
    let coverage = match lines.next() {
        None => None,
        Some(line) => {
            let id = line.strip_prefix("host coverage ")?;
            if lines.next().is_some()
                || id.len() != 16
                || !id.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return None;
            }
            Some(id)
        }
    };
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
    Some((crate::xbe_digest(&wire), coverage))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_guest_full_image_can_play_slim_but_machine_snapshots_remain_native() {
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
        let guest = GuestImage::capture(&linux, elf, &xbe);
        assert_eq!(
            guest.compare(&linux, &linux, elf, &xbe),
            Ok(ImageMatch::Native)
        );
        assert_eq!(
            guest.compare(&linux, mac, arm, &xbe),
            Ok(ImageMatch::FromBoot)
        );
        assert!(
            guest
                .compare(&linux, &mac.replace("-O2", "-O3"), arm, &xbe)
                .is_err()
        );
        assert!(
            guest
                .compare(&linux, &mac.replace("passes P", "passes X"), arm, &xbe)
                .is_err()
        );
        assert!(
            guest
                .compare(
                    &linux,
                    &mac.replace("-fno-fast-math", "-ffast-math"),
                    arm,
                    &xbe
                )
                .is_err()
        );
        assert!(guest.compare(&linux, mac, arm, &"fe".repeat(32)).is_err());
        assert!(
            GuestImage::capture(mac, arm, &xbe)
                .compare(mac, &linux, elf, &xbe)
                .is_err()
        );
        assert!(
            guest
                .compare(
                    &linux,
                    &linux.replace("0123456789abcdef", "2222222222222222"),
                    elf,
                    &xbe
                )
                .is_err()
        );
    }
}
