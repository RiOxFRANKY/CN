use netchat::csma::Algorithm;
use netchat::stats::simulate;

#[test]
fn one_station_has_no_collisions() {
    for algorithm in Algorithm::all() {
        let result = simulate(algorithm, 1, 25);
        assert_eq!(result.frames, 25);
        assert_eq!(result.collisions, 0);
        assert!(result.average_delay_ms >= 50.0);
        assert!(result.throughput_percent > 0.0);
        assert!(result.throughput_percent <= 100.0);
    }
}

#[test]
fn metrics_are_valid_for_every_algorithm() {
    for algorithm in Algorithm::all() {
        let result = simulate(algorithm, 12, 30);
        assert_eq!(result.frames, 360);
        assert!(result.collisions > 0);
        assert!(result.average_delay_ms.is_finite());
        assert!(result.average_delay_ms > 0.0);
        assert!(result.throughput_percent > 0.0);
        assert!(result.throughput_percent <= 100.0);
        assert!(result.elapsed_slots > 0);
    }
}

#[test]
fn simulation_is_repeatable() {
    for algorithm in Algorithm::all() {
        assert_eq!(
            simulate(algorithm, 8, 20),
            simulate(algorithm, 8, 20)
        );
    }
}
