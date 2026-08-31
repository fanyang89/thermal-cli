use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorKind {
    Cpu,
    Storage,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sensor {
    pub label: String,
    pub kind: SensorKind,
    pub temperature_c: f64,
    pub high_c: Option<f64>,
    pub critical_c: Option<f64>,
    pub alarm: bool,
    pub source: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuFrequency {
    pub current_min_mhz: f64,
    pub current_average_mhz: f64,
    pub current_max_mhz: f64,
    pub policy_max_mhz: f64,
    pub hardware_max_mhz: f64,
}

pub fn cpu_frequency(sysfs_root: &Path) -> Option<CpuFrequency> {
    let root = sysfs_root.join("devices/system/cpu/cpufreq");
    let mut current = Vec::new();
    let mut policy_max_khz = 0_u64;
    let mut hardware_max_khz = 0_u64;

    for policy in sorted_dirs(&root).ok()? {
        if !policy
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("policy"))
        {
            continue;
        }
        if let Some(value) = read_frequency_khz(&policy.join("scaling_cur_freq")) {
            current.push(value);
        }
        policy_max_khz = policy_max_khz
            .max(read_frequency_khz(&policy.join("scaling_max_freq")).unwrap_or_default());
        hardware_max_khz = hardware_max_khz
            .max(read_frequency_khz(&policy.join("cpuinfo_max_freq")).unwrap_or_default());
    }

    let current_min_khz = *current.iter().min()?;
    let current_max_khz = *current.iter().max()?;
    let current_average_khz = current.iter().sum::<u64>() as f64 / current.len() as f64;
    Some(CpuFrequency {
        current_min_mhz: current_min_khz as f64 / 1_000.0,
        current_average_mhz: current_average_khz / 1_000.0,
        current_max_mhz: current_max_khz as f64 / 1_000.0,
        policy_max_mhz: policy_max_khz as f64 / 1_000.0,
        hardware_max_mhz: hardware_max_khz as f64 / 1_000.0,
    })
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ThrottleCounters {
    pub core_events: u64,
    pub package_events: u64,
}

impl ThrottleCounters {
    pub fn increased_since(self, previous: Self) -> bool {
        self.core_events > previous.core_events || self.package_events > previous.package_events
    }
}

pub fn throttle_counters(sysfs_root: &Path) -> ThrottleCounters {
    let mut counters = ThrottleCounters::default();
    let cpu_root = sysfs_root.join("devices/system/cpu");
    let Ok(entries) = fs::read_dir(cpu_root) else {
        return counters;
    };
    for cpu in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        let is_cpu = cpu
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("cpu"))
            .is_some_and(|index| {
                !index.is_empty() && index.chars().all(|character| character.is_ascii_digit())
            });
        if !is_cpu {
            continue;
        }
        let throttle = cpu.join("thermal_throttle");
        counters.core_events = counters
            .core_events
            .saturating_add(read_counter(&throttle.join("core_throttle_count")));
        counters.package_events = counters
            .package_events
            .saturating_add(read_counter(&throttle.join("package_throttle_count")));
    }
    counters
}

pub fn discover(sysfs_root: &Path) -> io::Result<Vec<Sensor>> {
    let hwmon = discover_hwmon(&sysfs_root.join("class/hwmon"))?;
    if !hwmon.is_empty() {
        return Ok(hwmon);
    }

    discover_thermal_zones(&sysfs_root.join("class/thermal"))
}

fn discover_hwmon(root: &Path) -> io::Result<Vec<Sensor>> {
    let mut sensors = Vec::new();
    for hwmon_dir in sorted_dirs(root)? {
        let raw_device_name =
            read_trimmed(&hwmon_dir.join("name")).unwrap_or_else(|| "hardware_sensor".to_owned());
        let kind = sensor_kind(&raw_device_name);
        let device_name = humanize(&raw_device_name);

        let mut inputs: Vec<PathBuf> = fs::read_dir(&hwmon_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_temperature_input)
            })
            .collect();
        inputs.sort();

        for input in inputs {
            let Some(stem) = input.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let prefix = stem.trim_end_matches("_input");
            let Some(temperature_c) = read_temperature(&input) else {
                continue;
            };
            if !(-100.0..=250.0).contains(&temperature_c) {
                continue;
            }

            let sensor_label = read_trimmed(&hwmon_dir.join(format!("{prefix}_label")))
                .map(|label| humanize(&label));
            let label = match sensor_label {
                Some(label) if !label.eq_ignore_ascii_case(&device_name) => {
                    format!("{device_name} · {label}")
                }
                Some(label) => label,
                None => device_name.clone(),
            };

            sensors.push(Sensor {
                label,
                kind,
                temperature_c,
                high_c: read_temperature(&hwmon_dir.join(format!("{prefix}_max"))),
                critical_c: read_temperature(&hwmon_dir.join(format!("{prefix}_crit"))),
                alarm: read_alarm(&hwmon_dir, prefix),
                source: input,
            });
        }
    }

    sensors.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(sensors)
}

fn discover_thermal_zones(root: &Path) -> io::Result<Vec<Sensor>> {
    let mut sensors = Vec::new();
    for zone in sorted_dirs(root)? {
        if !zone
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("thermal_zone"))
        {
            continue;
        }

        let input = zone.join("temp");
        let Some(temperature_c) = read_temperature(&input) else {
            continue;
        };
        if !(-100.0..=250.0).contains(&temperature_c) {
            continue;
        }

        let raw_type =
            read_trimmed(&zone.join("type")).unwrap_or_else(|| "thermal_zone".to_owned());
        let label = humanize(&raw_type);
        sensors.push(Sensor {
            label,
            kind: sensor_kind(&raw_type),
            temperature_c,
            high_c: None,
            critical_c: None,
            alarm: false,
            source: input,
        });
    }

    sensors.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(sensors)
}

fn sorted_dirs(root: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn is_temperature_input(name: &str) -> bool {
    let Some(index) = name
        .strip_prefix("temp")
        .and_then(|value| value.strip_suffix("_input"))
    else {
        return false;
    };
    !index.is_empty() && index.chars().all(|character| character.is_ascii_digit())
}

fn sensor_kind(name: &str) -> SensorKind {
    let name = name.to_ascii_lowercase();
    if [
        "coretemp",
        "k10temp",
        "zenpower",
        "cpu_thermal",
        "cpu-thermal",
        "x86_pkg_temp",
        "soc_thermal",
    ]
    .iter()
    .any(|candidate| name.contains(candidate))
    {
        SensorKind::Cpu
    } else if name.contains("nvme") || name.contains("drivetemp") {
        SensorKind::Storage
    } else {
        SensorKind::Other
    }
}

fn read_alarm(directory: &Path, prefix: &str) -> bool {
    ["crit_alarm", "max_alarm"]
        .iter()
        .any(|suffix| read_counter(&directory.join(format!("{prefix}_{suffix}"))) > 0)
}

fn read_frequency_khz(path: &Path) -> Option<u64> {
    read_trimmed(path)?
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
}

fn read_counter(path: &Path) -> u64 {
    read_trimmed(path)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn read_temperature(path: &Path) -> Option<f64> {
    let millidegrees = read_trimmed(path)?.parse::<f64>().ok()?;
    Some(millidegrees / 1_000.0)
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn humanize(value: &str) -> String {
    let words: Vec<String> = value
        .split(['_', '-'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), characters.as_str()),
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        value.to_owned()
    } else {
        words.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("thermal-cli-test-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_labeled_hwmon_sensors_and_thresholds() {
        let root = TestDir::new();
        let hwmon = root.0.join("class/hwmon/hwmon0");
        fs::create_dir_all(&hwmon).unwrap();
        fs::write(hwmon.join("name"), "coretemp\n").unwrap();
        fs::write(hwmon.join("temp1_label"), "Package id 0\n").unwrap();
        fs::write(hwmon.join("temp1_input"), "47500\n").unwrap();
        fs::write(hwmon.join("temp1_max"), "80000\n").unwrap();
        fs::write(hwmon.join("temp1_crit"), "100000\n").unwrap();
        fs::write(hwmon.join("temp1_crit_alarm"), "1\n").unwrap();

        let sensors = discover(&root.0).unwrap();

        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].label, "Coretemp · Package id 0");
        assert_eq!(sensors[0].kind, SensorKind::Cpu);
        assert_eq!(sensors[0].temperature_c, 47.5);
        assert_eq!(sensors[0].high_c, Some(80.0));
        assert_eq!(sensors[0].critical_c, Some(100.0));
        assert!(sensors[0].alarm);
    }

    #[test]
    fn falls_back_to_thermal_zones() {
        let root = TestDir::new();
        let zone = root.0.join("class/thermal/thermal_zone0");
        fs::create_dir_all(&zone).unwrap();
        fs::write(zone.join("type"), "x86_pkg_temp\n").unwrap();
        fs::write(zone.join("temp"), "52000\n").unwrap();

        let sensors = discover(&root.0).unwrap();

        assert_eq!(sensors.len(), 1);
        assert_eq!(sensors[0].label, "X86 Pkg Temp");
        assert_eq!(sensors[0].kind, SensorKind::Cpu);
        assert_eq!(sensors[0].temperature_c, 52.0);
    }

    #[test]
    fn reads_aggregate_cpu_frequency() {
        let root = TestDir::new();
        let policy0 = root.0.join("devices/system/cpu/cpufreq/policy0");
        let policy1 = root.0.join("devices/system/cpu/cpufreq/policy1");
        fs::create_dir_all(&policy0).unwrap();
        fs::create_dir_all(&policy1).unwrap();
        for (policy, current) in [(&policy0, "400000\n"), (&policy1, "1800000\n")] {
            fs::write(policy.join("scaling_cur_freq"), current).unwrap();
            fs::write(policy.join("scaling_max_freq"), "4500000\n").unwrap();
            fs::write(policy.join("cpuinfo_max_freq"), "5400000\n").unwrap();
        }

        let frequency = cpu_frequency(&root.0).unwrap();

        assert_eq!(frequency.current_min_mhz, 400.0);
        assert_eq!(frequency.current_average_mhz, 1_100.0);
        assert_eq!(frequency.current_max_mhz, 1_800.0);
        assert_eq!(frequency.policy_max_mhz, 4_500.0);
        assert_eq!(frequency.hardware_max_mhz, 5_400.0);
    }

    #[test]
    fn detects_increasing_cpu_throttle_counters() {
        let root = TestDir::new();
        let throttle0 = root.0.join("devices/system/cpu/cpu0/thermal_throttle");
        let throttle1 = root.0.join("devices/system/cpu/cpu1/thermal_throttle");
        fs::create_dir_all(&throttle0).unwrap();
        fs::create_dir_all(&throttle1).unwrap();
        fs::write(throttle0.join("core_throttle_count"), "2\n").unwrap();
        fs::write(throttle0.join("package_throttle_count"), "5\n").unwrap();
        fs::write(throttle1.join("core_throttle_count"), "3\n").unwrap();
        fs::write(throttle1.join("package_throttle_count"), "7\n").unwrap();

        let previous = throttle_counters(&root.0);
        fs::write(throttle1.join("core_throttle_count"), "4\n").unwrap();
        let current = throttle_counters(&root.0);

        assert_eq!(previous.core_events, 5);
        assert_eq!(previous.package_events, 12);
        assert!(current.increased_since(previous));
    }

    #[test]
    fn ignores_invalid_temperature_files() {
        let root = TestDir::new();
        let hwmon = root.0.join("class/hwmon/hwmon0");
        fs::create_dir_all(&hwmon).unwrap();
        fs::write(hwmon.join("name"), "broken\n").unwrap();
        fs::write(hwmon.join("temp1_input"), "not-a-number\n").unwrap();
        fs::write(hwmon.join("temp2_input"), "999000\n").unwrap();

        assert!(discover(&root.0).unwrap().is_empty());
    }
}
