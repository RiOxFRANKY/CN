use crate::csma::Algorithm;

pub const STATION_COUNTS: [usize; 5] = [1, 2, 5, 10, 20];
pub const FRAMES_PER_STATION: usize = 40;

const FRAME_SLOTS: u64 = 5;
const SLOT_MS: f64 = 10.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Statistics {
    pub algorithm: Algorithm,
    pub stations: usize,
    pub frames: usize,
    pub collisions: u64,
    pub average_delay_ms: f64,
    pub throughput_percent: f64,
    pub elapsed_slots: u64,
}

struct Station {
    remaining: usize,
    ready_at: u64,
    created_at: u64,
    collision_round: u32,
}

struct Random {
    state: u64,
}

impl Random {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next(&mut self, limit: u64) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state % limit.max(1)
    }
}

pub fn analyze(algorithm: Algorithm) -> Vec<Statistics> {
    STATION_COUNTS
        .into_iter()
        .map(|stations| simulate(algorithm, stations, FRAMES_PER_STATION))
        .collect()
}

pub fn simulate(
    algorithm: Algorithm,
    station_count: usize,
    frames_per_station: usize,
) -> Statistics {
    if station_count == 0 || frames_per_station == 0 {
        return Statistics {
            algorithm,
            stations: station_count,
            frames: 0,
            collisions: 0,
            average_delay_ms: 0.0,
            throughput_percent: 0.0,
            elapsed_slots: 0,
        };
    }

    let seed = algorithm_seed(algorithm)
        ^ (station_count as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ frames_per_station as u64;
    let mut random = Random::new(seed);
    let mut stations: Vec<Station> = (0..station_count)
        .map(|_| Station {
            remaining: frames_per_station,
            ready_at: 0,
            created_at: 0,
            collision_round: 0,
        })
        .collect();
    let frame_total = station_count * frames_per_station;
    let mut completed = 0usize;
    let mut collisions = 0u64;
    let mut total_delay_slots = 0u64;
    let mut slot = 0u64;

    while completed < frame_total {
        let ready: Vec<usize> = stations
            .iter()
            .enumerate()
            .filter(|(_, station)| station.remaining > 0 && station.ready_at <= slot)
            .map(|(index, _)| index)
            .collect();

        if ready.is_empty() {
            slot = stations
                .iter()
                .filter(|station| station.remaining > 0)
                .map(|station| station.ready_at)
                .min()
                .unwrap_or(slot + 1);
            continue;
        }

        let attempts: Vec<usize> = match algorithm {
            Algorithm::PPersistent => ready
                .into_iter()
                .filter(|_| random.next(100) < 20)
                .collect(),
            _ => ready,
        };

        if attempts.is_empty() {
            slot += 1;
            continue;
        }

        if attempts.len() == 1 {
            let index = attempts[0];
            let completed_at = slot + FRAME_SLOTS;
            total_delay_slots += completed_at - stations[index].created_at;
            completed += 1;
            stations[index].remaining -= 1;
            stations[index].collision_round = 0;
            if stations[index].remaining > 0 {
                stations[index].created_at = completed_at;
                stations[index].ready_at = completed_at;
            } else {
                stations[index].ready_at = u64::MAX;
            }
            if algorithm == Algorithm::NonPersistent {
                for (other, station) in stations.iter_mut().enumerate() {
                    if other != index
                        && station.remaining > 0
                        && station.ready_at < completed_at
                    {
                        station.ready_at = completed_at + 1 + random.next(8);
                    }
                }
            }
            slot = completed_at;
            continue;
        }

        collisions += 1;
        let collision_slots = if algorithm == Algorithm::CsmaCd {
            1
        } else {
            FRAME_SLOTS
        };
        slot += collision_slots;
        for index in attempts {
            let station = &mut stations[index];
            station.collision_round = (station.collision_round + 1).min(10);
            let backoff = match algorithm {
                Algorithm::OnePersistent => {
                    1 + random.next(1u64 << station.collision_round.min(8))
                }
                Algorithm::NonPersistent => 2 + random.next(24),
                Algorithm::PPersistent => {
                    1 + random.next(1u64 << station.collision_round.min(6))
                }
                Algorithm::CsmaCd => 1 + random.next(1u64 << station.collision_round),
            };
            station.ready_at = slot + backoff;
        }
    }

    Statistics {
        algorithm,
        stations: station_count,
        frames: completed,
        collisions,
        average_delay_ms: total_delay_slots as f64 * SLOT_MS / completed as f64,
        throughput_percent: completed as f64 * FRAME_SLOTS as f64 * 100.0 / slot as f64,
        elapsed_slots: slot,
    }
}

fn algorithm_seed(algorithm: Algorithm) -> u64 {
    match algorithm {
        Algorithm::OnePersistent => 0x1234_5678_9abc_def1,
        Algorithm::NonPersistent => 0x2345_6789_abcd_ef12,
        Algorithm::PPersistent => 0x3456_789a_bcde_f123,
        Algorithm::CsmaCd => 0x4567_89ab_cdef_1234,
    }
}
