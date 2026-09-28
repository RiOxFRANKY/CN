use netchat::csma::Algorithm;
use netchat::stats::{STATION_COUNTS, analyze, simulate};

#[test]
fn standard_analysis_covers_all_station_counts() {
    for algorithm in Algorithm::all() {
        let results = analyze(algorithm);
        let counts: Vec<usize> = results.iter().map(|result| result.stations).collect();
        assert_eq!(counts, STATION_COUNTS);
    }
}

#[test]
fn more_stations_increase_contention() {
    for algorithm in Algorithm::all() {
        let one = simulate(algorithm, 1, 40);
        let many = simulate(algorithm, 20, 40);
        assert!(many.collisions > one.collisions);
        assert!(many.average_delay_ms > one.average_delay_ms);
        assert!(many.throughput_percent < one.throughput_percent);
    }
}

#[test]
fn empty_input_returns_empty_statistics() {
    let result = simulate(Algorithm::OnePersistent, 0, 40);
    assert_eq!(result.frames, 0);
    assert_eq!(result.collisions, 0);
    assert_eq!(result.average_delay_ms, 0.0);
    assert_eq!(result.throughput_percent, 0.0);
}
