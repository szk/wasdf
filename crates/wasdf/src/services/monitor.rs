//! Monitor: samples system metrics on its own thread into a shared snapshot
//! that render reads (doc/ARCHITECTURE.md, Render Contract). The battery and
//! WiFi (the top row's status readout) are always sampled; the
//! monitor panel's metrics only while the panel is on screen. A new sample
//! wakes the loop with `AppEvent::Tick`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::core::AppEvent;

/// How often the thread looks for due work; every cadence is a whole number
/// of seconds, so this bounds the lag of a sample, not its rate.
const POLL: Duration = Duration::from_millis(250);

/// One gauge metric (cpu / mem / net): its current ratio (0..1), value text,
/// and the ratio history over its graph window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetricSample {
    pub ratio: f64,
    pub value: String,
    pub history: Vec<f64>,
}

/// Disk read/write operation rates (ops/sec) and their histories.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskSample {
    pub r_rate: f64,
    pub w_rate: f64,
    pub r_hist: Vec<f64>,
    pub w_hist: Vec<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// The battery, when the machine has one the OS reports.
    pub battery: Option<Battery>,
    /// WiFi signal strength in percent, when a wireless link reports one.
    pub wifi: Option<u8>,
    pub cpu: Option<MetricSample>,
    pub mem: Option<MetricSample>,
    pub net: Option<MetricSample>,
    pub disk: DiskSample,
}

/// Battery charge, and whether it is charging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

/// How often battery and WiFi are read: they change slowly, and on macOS the
/// battery costs a process spawn.
const POWER_EVERY: Duration = Duration::from_secs(30);

/// One metric's sampling cadence (from its `monitor.<id>.*` options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    pub visible: bool,
    /// Seconds between samples.
    pub refresh: u64,
    /// Seconds of history kept for the graph.
    pub window: u64,
}

impl Default for Cadence {
    fn default() -> Self {
        Cadence { visible: true, refresh: 5, window: 120 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Config {
    pub cpu: Cadence,
    pub mem: Cadence,
    pub dsk: Cadence,
    pub net: Cadence,
}

struct Shared {
    snapshot: Mutex<Arc<Snapshot>>,
    config: Mutex<Config>,
    /// Whether the monitor panel is on screen (set after each draw).
    active: AtomicBool,
}

pub struct Monitor {
    shared: Arc<Shared>,
}

impl Monitor {
    /// Start sampling, waking the loop through `tx`. Without a `tx` (tests)
    /// no thread is started and the snapshot stays empty.
    pub fn spawn(tx: Option<Sender<AppEvent>>) -> Self {
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Arc::default()),
            config: Mutex::new(Config::default()),
            active: AtomicBool::new(false),
        });
        let Some(tx) = tx else { return Monitor { shared } };
        let worker = shared.clone();
        std::thread::spawn(move || {
            let mut state = Sampler::default();
            loop {
                let config = worker.config.lock().map(|c| *c).unwrap_or_default();
                let active = worker.active.load(Ordering::Relaxed);
                let current = worker.snapshot.lock().map(|s| s.clone()).unwrap_or_default();
                if let Some(next) = state.step(Instant::now(), &config, active, &current) {
                    if let Ok(mut s) = worker.snapshot.lock() {
                        *s = Arc::new(next);
                    }
                    if tx.send(AppEvent::Tick).is_err() {
                        return; // the loop is gone
                    }
                }
                std::thread::sleep(POLL);
            }
        });
        Monitor { shared }
    }

    /// The latest samples — a pointer copy, cheap enough for every frame.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.shared.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn set_active(&self, active: bool) {
        self.shared.active.store(active, Ordering::Relaxed);
    }

    pub fn set_config(&self, config: Config) {
        if let Ok(mut c) = self.shared.config.lock() {
            *c = config;
        }
    }
}

/// The sampling state the thread carries between steps.
#[derive(Default)]
struct Sampler {
    power_at: Option<Instant>,
    due: [Option<Instant>; 4], // cpu, mem, dsk, net
    cpu: VecDeque<f64>,
    mem: VecDeque<f64>,
    net_hist: VecDeque<f64>,
    net_bytes: VecDeque<(Instant, u64)>,
    disk_prev: Option<(Instant, u64, u64)>,
    r_hist: VecDeque<f64>,
    w_hist: VecDeque<f64>,
}

fn push(hist: &mut VecDeque<f64>, v: f64, c: &Cadence) -> Vec<f64> {
    hist.push_back(v);
    let cap = (c.window / c.refresh.max(1)).max(1) as usize;
    while hist.len() > cap {
        hist.pop_front();
    }
    hist.iter().copied().collect()
}

impl Sampler {
    /// Take whatever samples are due at `now`; `Some` with the new snapshot
    /// when anything changed.
    fn step(&mut self, now: Instant, config: &Config, active: bool, current: &Snapshot) -> Option<Snapshot> {
        let mut next = current.clone();
        let mut changed = false;
        if self.power_at.is_none_or(|t| now.duration_since(t) >= POWER_EVERY) {
            self.power_at = Some(now);
            let (battery, wifi) = (battery(), wifi_percent());
            changed |= battery != next.battery || wifi != next.wifi;
            next.battery = battery;
            next.wifi = wifi;
        }
        if !active {
            return changed.then_some(next);
        }
        let cadences = [config.cpu, config.mem, config.dsk, config.net];
        for (i, c) in cadences.iter().enumerate() {
            if !c.visible || self.due[i].is_some_and(|t| now < t) {
                continue;
            }
            self.due[i] = Some(now + Duration::from_secs(c.refresh.max(1)));
            changed = true;
            match i {
                0 => {
                    if let Some([la1, _, _]) = loadavg() {
                        let ratio = (la1 / 4.0).clamp(0.0, 1.0);
                        next.cpu = Some(MetricSample { ratio, value: format!("{la1:.2}"), history: push(&mut self.cpu, ratio, c) });
                    }
                }
                1 => {
                    if let Some((total, used)) = mem_info() {
                        // Show available memory; the gauge tracks used pressure.
                        let ratio = if total > 0 { used as f64 / total as f64 } else { 0.0 };
                        let value = wasdf_sdk::format_size(total.saturating_sub(used));
                        next.mem = Some(MetricSample { ratio, value, history: push(&mut self.mem, ratio, c) });
                    }
                }
                2 => {
                    let (reads, writes) = disk_ops_total();
                    if let Some((t, pr, pw)) = self.disk_prev {
                        let dt = now.duration_since(t).as_secs_f64();
                        if dt > 0.0 {
                            let r = reads.saturating_sub(pr) as f64 / dt;
                            let w = writes.saturating_sub(pw) as f64 / dt;
                            next.disk = DiskSample {
                                r_rate: r,
                                w_rate: w,
                                r_hist: push(&mut self.r_hist, r, c),
                                w_hist: push(&mut self.w_hist, w, c),
                            };
                        }
                    }
                    self.disk_prev = Some((now, reads, writes));
                }
                _ => {
                    let total = net_total_bytes();
                    self.net_bytes.push_back((now, total));
                    while self.net_bytes.front().is_some_and(|&(t, _)| now.duration_since(t).as_secs_f64() > 60.0) {
                        self.net_bytes.pop_front();
                    }
                    let avg5 = window_rate(&self.net_bytes, now, total, 5.0);
                    let avg60 = window_rate(&self.net_bytes, now, total, 60.0);
                    let ratio = if avg60 > 0 { (avg5 as f64 / avg60 as f64).clamp(0.0, 1.0) } else { 0.0 };
                    let value = format!("{}/s", wasdf_sdk::format_size(avg5));
                    next.net = Some(MetricSample { ratio, value, history: push(&mut self.net_hist, ratio, c) });
                }
            }
        }
        changed.then_some(next)
    }
}

/// Total and used physical memory in bytes.
fn mem_info() -> Option<(u64, u64)> {
    #[cfg(target_os = "macos")]
    {
        let mut total = 0u64;
        let mut sz = std::mem::size_of::<u64>();
        let name = c"hw.memsize";
        unsafe {
            libc::sysctlbyname(
                name.as_ptr(), &mut total as *mut u64 as *mut libc::c_void,
                &mut sz, std::ptr::null_mut(), 0,
            );
        }
        if total == 0 { return None; }

        let mut free_pages = 0u32;
        let mut fp_sz = std::mem::size_of::<u32>();
        let fp_name = c"vm.page_free_count";
        unsafe {
            libc::sysctlbyname(
                fp_name.as_ptr(), &mut free_pages as *mut u32 as *mut libc::c_void,
                &mut fp_sz, std::ptr::null_mut(), 0,
            );
        }

        let mut page_size = 4096u64;
        let mut ps_sz = std::mem::size_of::<u64>();
        let ps_name = c"hw.pagesize";
        unsafe {
            libc::sysctlbyname(
                ps_name.as_ptr(), &mut page_size as *mut u64 as *mut libc::c_void,
                &mut ps_sz, std::ptr::null_mut(), 0,
            );
        }

        let available = free_pages as u64 * page_size;
        Some((total, total.saturating_sub(available)))
    }
    #[cfg(target_os = "linux")]
    {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        let mut total = 0u64;
        let mut available = 0u64;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total = rest.trim().split_whitespace().next()?.parse::<u64>().ok()? * 1024;
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                available = rest.trim().split_whitespace().next()?.parse::<u64>().ok()? * 1024;
            }
        }
        if total == 0 { None } else { Some((total, total.saturating_sub(available))) }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    { None }
}

/// Cumulative (reads, writes) operation counts across all connected block
/// devices, via IOKit's `IOBlockStorageDriver` statistics.
#[cfg(target_os = "macos")]
fn disk_ops_total() -> (u64, u64) {
    use std::ffi::c_void;
    type CFRef = *const c_void;
    type IoObject = u32;
    const KERN_SUCCESS: i32 = 0;
    const UTF8: u32 = 0x0800_0100;
    const SINT64: i32 = 4;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const libc::c_char) -> CFRef;
        fn IOServiceGetMatchingServices(port: u32, matching: CFRef, iter: *mut IoObject) -> i32;
        fn IOIteratorNext(iter: IoObject) -> IoObject;
        fn IOObjectRelease(obj: IoObject) -> i32;
        fn IORegistryEntryCreateCFProperties(
            entry: IoObject, props: *mut CFRef, allocator: CFRef, options: u32,
        ) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(alloc: CFRef, s: *const libc::c_char, enc: u32) -> CFRef;
        fn CFDictionaryGetValue(dict: CFRef, key: CFRef) -> CFRef;
        fn CFNumberGetValue(num: CFRef, ty: i32, value: *mut c_void) -> u8;
        fn CFRelease(cf: CFRef);
    }

    let (mut reads, mut writes) = (0u64, 0u64);
    unsafe {
        let matching = IOServiceMatching(c"IOBlockStorageDriver".as_ptr());
        if matching.is_null() {
            return (0, 0);
        }
        let mut iter: IoObject = 0;
        if IOServiceGetMatchingServices(0, matching, &mut iter) != KERN_SUCCESS {
            return (0, 0);
        }
        let null: CFRef = std::ptr::null();
        let k_stats = CFStringCreateWithCString(null, c"Statistics".as_ptr(), UTF8);
        let k_read = CFStringCreateWithCString(null, c"Operations (Read)".as_ptr(), UTF8);
        let k_write = CFStringCreateWithCString(null, c"Operations (Write)".as_ptr(), UTF8);
        loop {
            let entry = IOIteratorNext(iter);
            if entry == 0 {
                break;
            }
            let mut props: CFRef = std::ptr::null();
            if IORegistryEntryCreateCFProperties(entry, &mut props, null, 0) == KERN_SUCCESS
                && !props.is_null()
            {
                let stats = CFDictionaryGetValue(props, k_stats);
                if !stats.is_null() {
                    let mut v: i64 = 0;
                    let r = CFDictionaryGetValue(stats, k_read);
                    if !r.is_null() && CFNumberGetValue(r, SINT64, &mut v as *mut i64 as *mut c_void) != 0 {
                        reads += v as u64;
                    }
                    v = 0;
                    let wv = CFDictionaryGetValue(stats, k_write);
                    if !wv.is_null() && CFNumberGetValue(wv, SINT64, &mut v as *mut i64 as *mut c_void) != 0 {
                        writes += v as u64;
                    }
                }
                CFRelease(props);
            }
            IOObjectRelease(entry);
        }
        CFRelease(k_stats);
        CFRelease(k_read);
        CFRelease(k_write);
        IOObjectRelease(iter);
    }
    (reads, writes)
}

/// Cumulative (reads, writes) completed across all whole block devices, from
/// `/proc/diskstats` (field 4 = reads completed, field 8 = writes completed).
#[cfg(target_os = "linux")]
fn disk_ops_total() -> (u64, u64) {
    let (mut reads, mut writes) = (0u64, 0u64);
    if let Ok(content) = std::fs::read_to_string("/proc/diskstats") {
        for line in content.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 8 {
                continue;
            }
            let name = f[2];
            if name.starts_with("loop") || name.starts_with("ram") || is_partition(name) {
                continue;
            }
            reads += f[3].parse::<u64>().unwrap_or(0);
            writes += f[7].parse::<u64>().unwrap_or(0);
        }
    }
    (reads, writes)
}

/// Heuristic: is this `/proc/diskstats` device name a partition (skip, to avoid
/// double-counting its whole disk)? e.g. `sda1`, `nvme0n1p2`, `mmcblk0p1`.
#[cfg(target_os = "linux")]
fn is_partition(name: &str) -> bool {
    let ends_digit = name.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false);
    if !ends_digit {
        return false;
    }
    // nvme/mmcblk whole disks end in a digit (nvme0n1); their partitions carry a `p`.
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        return name.contains('p');
    }
    // sd*/hd*/vd*/xvd*: a trailing digit marks a partition (sda1).
    true
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn disk_ops_total() -> (u64, u64) {
    (0, 0)
}

/// Cumulative bytes (rx + tx) across all UP non-loopback interfaces.
#[cfg(target_os = "macos")]
fn net_total_bytes() -> u64 {
    let mut total: u64 = 0;
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return 0;
        }
        let mut ifa = ifap;
        while !ifa.is_null() {
            let flags = (*ifa).ifa_flags as u32;
            let up = flags & (libc::IFF_UP as u32) != 0;
            let loopback = flags & (libc::IFF_LOOPBACK as u32) != 0;
            let addr = (*ifa).ifa_addr;
            // Link-level rows (AF_LINK) carry the per-interface byte counters in
            // their ifa_data as a `struct if_data`.
            if up && !loopback && !addr.is_null()
                && (*addr).sa_family as i32 == libc::AF_LINK
                && !(*ifa).ifa_data.is_null()
            {
                let data = (*ifa).ifa_data as *const libc::if_data;
                total += (*data).ifi_ibytes as u64 + (*data).ifi_obytes as u64;
            }
            ifa = (*ifa).ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    total
}

/// Cumulative bytes (rx + tx) across all non-loopback interfaces, from
/// `/proc/net/dev`.
#[cfg(target_os = "linux")]
fn net_total_bytes() -> u64 {
    let mut total: u64 = 0;
    if let Ok(content) = std::fs::read_to_string("/proc/net/dev") {
        for line in content.lines() {
            let Some((name, rest)) = line.split_once(':') else { continue };
            if name.trim() == "lo" { continue; }
            let cols: Vec<&str> = rest.split_whitespace().collect();
            // rx bytes = column 0, tx bytes = column 8.
            let rx = cols.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let tx = cols.get(8).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            total += rx + tx;
        }
    }
    total
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn net_total_bytes() -> u64 {
    0
}

/// Average byte/second rate over roughly the last `min_age` seconds: diff the
/// current cumulative byte count against the newest sample at least `min_age`
/// old (or the oldest available if the history is shorter than the window).
fn window_rate(
    hist: &std::collections::VecDeque<(std::time::Instant, u64)>,
    now: std::time::Instant,
    total: u64,
    min_age: f64,
) -> u64 {
    let mut base: Option<&(std::time::Instant, u64)> = None;
    for s in hist.iter() {
        if now.duration_since(s.0).as_secs_f64() >= min_age {
            base = Some(s);
        } else {
            break;
        }
    }
    match base.or_else(|| hist.front()) {
        Some(&(t0, b0)) => {
            let dt = now.duration_since(t0).as_secs_f64();
            if dt > 0.0 { (total.saturating_sub(b0) as f64 / dt) as u64 } else { 0 }
        }
        None => 0,
    }
}

/// The first battery the OS reports. Linux: `/sys/class/power_supply/*` of
/// type `Battery` (`capacity`, `status`). macOS: `pmset -g batt`.
#[cfg(target_os = "linux")]
fn battery() -> Option<Battery> {
    let dirs = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for d in dirs.flatten() {
        let p = d.path();
        let read = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
        if read("type").as_deref() != Some("Battery") {
            continue;
        }
        let Some(percent) = read("capacity").and_then(|c| c.parse::<u8>().ok()) else { continue };
        let charging = matches!(read("status").as_deref(), Some("Charging" | "Full"));
        return Some(Battery { percent: percent.min(100), charging });
    }
    None
}

#[cfg(target_os = "macos")]
fn battery() -> Option<Battery> {
    let out = std::process::Command::new("pmset").args(["-g", "batt"]).output().ok()?;
    parse_pmset(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn battery() -> Option<Battery> {
    None
}

/// `pmset -g batt`: `… -InternalBattery-0 (id=…)\t87%; charging; …`. A desktop
/// lists no battery line.
fn parse_pmset(text: &str) -> Option<Battery> {
    let line = text.lines().find(|l| l.contains("InternalBattery"))?;
    let pct = line.split(['\t', ' ']).find_map(|w| w.strip_suffix("%;").and_then(|n| n.parse::<u8>().ok()))?;
    let state = line.split(';').nth(1).map(str::trim).unwrap_or("");
    Some(Battery { percent: pct.min(100), charging: matches!(state, "charging" | "charged" | "finishing charge") })
}

/// WiFi signal in percent. Linux: `/proc/net/wireless` signal level (dBm),
/// mapped the usual way (−100 dBm = 0 %, −50 dBm = 100 %). Elsewhere there is
/// no source without a privileged tool, so none.
fn wifi_percent() -> Option<u8> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/net/wireless").ok()?;
        parse_wireless(&text)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// `/proc/net/wireless`: two header lines, then `iface: status link level noise …`
/// (level in dBm, with a trailing `.`). The first interface wins.
fn parse_wireless(text: &str) -> Option<u8> {
    let line = text.lines().skip(2).find(|l| l.contains(':'))?;
    let level: f64 = line.split(':').nth(1)?.split_whitespace().nth(2)?.trim_end_matches('.').parse().ok()?;
    // Drivers that report a 0..100-style level instead of dBm give a positive value.
    let pct = if level > 0.0 { level } else { 2.0 * (level + 100.0) };
    Some(pct.clamp(0.0, 100.0) as u8)
}

/// 1/5/15-minute load averages, if available.
fn loadavg() -> Option<[f64; 3]> {
    let mut a = [0f64; 3];
    if unsafe { libc::getloadavg(a.as_mut_ptr(), 3) } == 3 { Some(a) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_follow_their_cadence_and_keep_a_bounded_history() {
        let mut s = Sampler::default();
        let c = Cadence { visible: true, refresh: 1, window: 3 };
        let config = Config { cpu: c, mem: c, dsk: c, net: c };
        let t0 = Instant::now();
        let mut snap = s.step(t0, &config, true, &Snapshot::default()).expect("first samples");
        assert!(snap.net.is_some(), "net always yields a value");
        assert_eq!(s.step(t0 + Duration::from_millis(500), &config, true, &snap), None, "not due");
        for k in 1..6 {
            snap = s.step(t0 + Duration::from_secs(k), &config, true, &snap).expect("due");
        }
        assert_eq!(snap.net.unwrap().history.len(), 3, "window / refresh samples");
    }
}
