use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    OnePersistent,
    NonPersistent,
    PPersistent,
    CsmaCd,
}

impl Algorithm {
    pub fn all() -> [Algorithm; 4] {
        [
            Algorithm::OnePersistent,
            Algorithm::NonPersistent,
            Algorithm::PPersistent,
            Algorithm::CsmaCd,
        ]
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "1" | "1-persistent" | "1persistent" | "one-persistent" => {
                Some(Algorithm::OnePersistent)
            }
            "2" | "non-persistent" | "nonpersistent" => Some(Algorithm::NonPersistent),
            "3" | "p-persistent" | "ppersistent" => Some(Algorithm::PPersistent),
            "4" | "csma/cd" | "csma-cd" | "csmacd" => Some(Algorithm::CsmaCd),
            _ => None,
        }
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Algorithm::OnePersistent => "1-persistent",
            Algorithm::NonPersistent => "non-persistent",
            Algorithm::PPersistent => "p-persistent",
            Algorithm::CsmaCd => "csma/cd",
        };
        write!(f, "{}", name)
    }
}

#[derive(Default)]
pub struct AccessReport {
    pub attempts: u32,
    pub collisions: u32,
    pub waited_ms: u64,
}

pub struct Medium {
    busy: AtomicBool,
    random: AtomicU64,
}

pub struct Access<'a> {
    medium: &'a Medium,
}

impl Medium {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self {
            busy: AtomicBool::new(false),
            random: AtomicU64::new(seed | 1),
        }
    }

    pub fn acquire(&self, algorithm: Algorithm) -> (Access<'_>, AccessReport) {
        let mut report = AccessReport::default();
        match algorithm {
            Algorithm::OnePersistent => loop {
                report.attempts += 1;
                if self.take() {
                    break;
                }
                self.pause(5, &mut report);
            },
            Algorithm::NonPersistent => loop {
                report.attempts += 1;
                if self.take() {
                    break;
                }
                let delay = 25 + self.number(126);
                self.pause(delay, &mut report);
            },
            Algorithm::PPersistent => loop {
                report.attempts += 1;
                if !self.busy.load(Ordering::Acquire) && self.number(100) < 50 && self.take() {
                    break;
                }
                self.pause(50, &mut report);
            },
            Algorithm::CsmaCd => {
                let mut round = 0u32;
                loop {
                    report.attempts += 1;
                    if self.take() {
                        break;
                    }
                    report.collisions += 1;
                    round = (round + 1).min(10);
                    let slots = self.number(1u64 << round);
                    self.pause(10 * slots.max(1), &mut report);
                }
            }
        }
        (Access { medium: self }, report)
    }

    fn take(&self) -> bool {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn pause(&self, milliseconds: u64, report: &mut AccessReport) {
        report.waited_ms += milliseconds;
        thread::sleep(Duration::from_millis(milliseconds));
    }

    fn number(&self, limit: u64) -> u64 {
        let mut old = self.random.load(Ordering::Relaxed);
        loop {
            let mut next = old;
            next ^= next << 13;
            next ^= next >> 7;
            next ^= next << 17;
            match self
                .random
                .compare_exchange_weak(old, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return next % limit.max(1),
                Err(value) => old = value,
            }
        }
    }
}

impl Default for Medium {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Access<'_> {
    fn drop(&mut self) {
        self.medium.busy.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::Algorithm;

    #[test]
    fn accepts_names_and_numbers() {
        for (number, algorithm) in Algorithm::all().into_iter().enumerate() {
            assert_eq!(Algorithm::parse(&(number + 1).to_string()), Some(algorithm));
            assert_eq!(Algorithm::parse(&algorithm.to_string()), Some(algorithm));
        }
    }
}
