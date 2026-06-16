use std::fs;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResourceSnapshot {
    self_user_cpu: Duration,
    self_system_cpu: Duration,
    child_user_cpu: Duration,
    child_system_cpu: Duration,
    rss_kb: Option<u64>,
    high_water_rss_kb: Option<u64>,
    max_rss_kb: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResourceDelta {
    pub(crate) user_cpu: Duration,
    pub(crate) system_cpu: Duration,
    pub(crate) total_cpu: Duration,
    pub(crate) cpu_percent: Option<f64>,
    pub(crate) rss_start_kb: Option<u64>,
    pub(crate) rss_end_kb: Option<u64>,
    pub(crate) rss_delta_kb: Option<i64>,
    pub(crate) high_water_rss_kb: Option<u64>,
    pub(crate) max_rss_kb: Option<u64>,
}

impl ResourceSnapshot {
    pub(crate) fn capture() -> Self {
        let self_usage = usage(libc::RUSAGE_SELF);
        let child_usage = usage(libc::RUSAGE_CHILDREN);
        let status = read_proc_status();

        Self {
            self_user_cpu: self_usage.user_cpu,
            self_system_cpu: self_usage.system_cpu,
            child_user_cpu: child_usage.user_cpu,
            child_system_cpu: child_usage.system_cpu,
            rss_kb: status.rss_kb,
            high_water_rss_kb: status.high_water_rss_kb,
            max_rss_kb: self_usage.max_rss_kb.max(child_usage.max_rss_kb),
        }
    }

    pub(crate) fn delta(self, end: Self, wall_duration: Duration) -> ResourceDelta {
        let user_cpu = end
            .self_user_cpu
            .saturating_sub(self.self_user_cpu)
            .saturating_add(end.child_user_cpu.saturating_sub(self.child_user_cpu));
        let system_cpu = end
            .self_system_cpu
            .saturating_sub(self.self_system_cpu)
            .saturating_add(end.child_system_cpu.saturating_sub(self.child_system_cpu));
        let total_cpu = user_cpu.saturating_add(system_cpu);
        let cpu_percent = if wall_duration.is_zero() {
            None
        } else {
            Some((total_cpu.as_secs_f64() / wall_duration.as_secs_f64()) * 100.0)
        };

        ResourceDelta {
            user_cpu,
            system_cpu,
            total_cpu,
            cpu_percent,
            rss_start_kb: self.rss_kb,
            rss_end_kb: end.rss_kb,
            rss_delta_kb: self
                .rss_kb
                .zip(end.rss_kb)
                .map(|(start, end)| end as i64 - start as i64),
            high_water_rss_kb: end.high_water_rss_kb,
            max_rss_kb: end.max_rss_kb,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Usage {
    user_cpu: Duration,
    system_cpu: Duration,
    max_rss_kb: Option<u64>,
}

fn usage(who: libc::c_int) -> Usage {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let ok = unsafe { libc::getrusage(who, usage.as_mut_ptr()) == 0 };
    if !ok {
        return Usage {
            user_cpu: Duration::ZERO,
            system_cpu: Duration::ZERO,
            max_rss_kb: None,
        };
    }

    let usage = unsafe { usage.assume_init() };
    Usage {
        user_cpu: timeval_duration(usage.ru_utime),
        system_cpu: timeval_duration(usage.ru_stime),
        max_rss_kb: u64::try_from(usage.ru_maxrss).ok(),
    }
}

fn timeval_duration(value: libc::timeval) -> Duration {
    let secs = u64::try_from(value.tv_sec).unwrap_or_default();
    let micros = u32::try_from(value.tv_usec).unwrap_or_default();
    Duration::new(secs, micros.saturating_mul(1_000))
}

#[derive(Debug, Clone, Copy, Default)]
struct ProcStatus {
    rss_kb: Option<u64>,
    high_water_rss_kb: Option<u64>,
}

fn read_proc_status() -> ProcStatus {
    let Ok(content) = fs::read_to_string("/proc/self/status") else {
        return ProcStatus::default();
    };

    let mut status = ProcStatus::default();
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            status.rss_kb = parse_status_kb(value);
        } else if let Some(value) = line.strip_prefix("VmHWM:") {
            status.high_water_rss_kb = parse_status_kb(value);
        }
    }

    status
}

fn parse_status_kb(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}
