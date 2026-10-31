//! Detected tags: the OS, the CPU architecture, the RAM, the logical CPUs
//! and the GPU vendor of this machine. A value that the worker cannot find
//! cheaply gives no tag.

use consort_proto::model::is_valid_tag;

/// What the worker found about this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    /// `std::env::consts::OS`.
    pub os: String,
    /// `std::env::consts::ARCH`.
    pub arch: String,
    /// The physical memory in bytes.
    pub ram_bytes: Option<u64>,
    /// The logical CPUs.
    pub cpus: Option<usize>,
    /// GPU vendors: `nvidia`, `amd`, `intel` or `apple`.
    pub gpus: Vec<&'static str>,
}

/// The tags of a machine, in a fixed order: OS, architecture, RAM, CPUs,
/// GPUs. RAM is rounded to whole GiB.
pub fn format_tags(machine: &Machine) -> Vec<String> {
    let mut tags = Vec::new();
    match machine.os.as_str() {
        "macos" => tags.push("macos".to_owned()),
        "windows" => tags.push("windows".to_owned()),
        "linux" => tags.push("linux".to_owned()),
        _ => {}
    }
    match machine.arch.as_str() {
        "aarch64" => tags.push("arm64".to_owned()),
        "x86_64" => tags.push("x64".to_owned()),
        _ => {}
    }
    if let Some(bytes) = machine.ram_bytes {
        const GIB: u64 = 1 << 30;
        let gib = (bytes + GIB / 2) / GIB;
        if gib > 0 {
            tags.push(format!("ram-{gib}gb"));
        }
    }
    if let Some(cpus) = machine.cpus.filter(|c| *c > 0) {
        tags.push(format!("cpu-{cpus}"));
    }
    for gpu in &machine.gpus {
        let tag = format!("gpu-{gpu}");
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags.retain(|t| is_valid_tag(t));
    tags
}

/// Finds this machine's values. This runs small commands, so call it from a
/// blocking task.
pub fn detect() -> Machine {
    let mut gpus = Vec::new();
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        gpus.push("apple");
    }
    if consort_harness::process::resolve_program("nvidia-smi").is_some() {
        gpus.push("nvidia");
    }
    #[cfg(target_os = "linux")]
    for vendor in linux_gpu_vendors() {
        if !gpus.contains(&vendor) {
            gpus.push(vendor);
        }
    }
    Machine {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        ram_bytes: ram_bytes(),
        cpus: std::thread::available_parallelism().ok().map(|n| n.get()),
        gpus,
    }
}

/// The detected tags of this machine.
pub fn detected_tags() -> Vec<String> {
    format_tags(&detect())
}

/// The GPU vendor of a PCI vendor ID, such as `0x10de`.
pub fn gpu_vendor(pci_vendor: &str) -> Option<&'static str> {
    match pci_vendor.trim().to_ascii_lowercase().as_str() {
        "0x10de" => Some("nvidia"),
        "0x1002" => Some("amd"),
        "0x8086" => Some("intel"),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn linux_gpu_vendors() -> Vec<&'static str> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path().join("device").join("vendor")) else {
            continue;
        };
        if let Some(vendor) = gpu_vendor(&text)
            && !out.contains(&vendor)
        {
            out.push(vendor);
        }
    }
    out
}

/// The `MemTotal` of `/proc/meminfo`, in bytes.
pub fn meminfo_bytes(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

fn ram_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        meminfo_bytes(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_have_the_documented_names() {
        let machine = Machine {
            os: "macos".into(),
            arch: "aarch64".into(),
            ram_bytes: Some(34_359_738_368),
            cpus: Some(12),
            gpus: vec!["apple"],
        };
        assert_eq!(
            format_tags(&machine),
            ["macos", "arm64", "ram-32gb", "cpu-12", "gpu-apple"]
        );
        let linux = Machine {
            os: "linux".into(),
            arch: "x86_64".into(),
            // MemTotal is a little below the RAM size: 31.3 GiB.
            ram_bytes: Some(33_607_000_000),
            cpus: Some(32),
            gpus: vec!["nvidia", "intel", "nvidia"],
        };
        assert_eq!(
            format_tags(&linux),
            [
                "linux",
                "x64",
                "ram-31gb",
                "cpu-32",
                "gpu-nvidia",
                "gpu-intel"
            ]
        );
        let unknown = Machine {
            os: "freebsd".into(),
            arch: "riscv64".into(),
            ..Machine::default()
        };
        assert!(format_tags(&unknown).is_empty());
        for tag in format_tags(&machine)
            .iter()
            .chain(format_tags(&linux).iter())
        {
            assert!(is_valid_tag(tag), "{tag}");
        }
    }

    #[test]
    fn vendors_and_meminfo_parse() {
        assert_eq!(gpu_vendor("0x10de\n"), Some("nvidia"));
        assert_eq!(gpu_vendor("0x1002"), Some("amd"));
        assert_eq!(gpu_vendor("0x8086"), Some("intel"));
        assert_eq!(gpu_vendor("0x1af4"), None);
        assert_eq!(
            meminfo_bytes("MemTotal:       32768000 kB\nMemFree: 1 kB\n"),
            Some(32_768_000 * 1024)
        );
        assert_eq!(meminfo_bytes("nothing"), None);
    }

    #[test]
    fn this_machine_has_valid_tags() {
        for tag in detected_tags() {
            assert!(is_valid_tag(&tag), "{tag}");
        }
    }
}
