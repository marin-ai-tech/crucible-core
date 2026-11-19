//! Tests reproducibility of the physics step.

mod common;

use common::{SCENARIOS, Trajectory, hash_all_scenarios, run_states};
use crucible_physics::PhysicsSnapshot;

/// Hash of every body's position, angle, velocity, and sleeping state on every tick in
/// all five reference scenarios. It was recorded on Linux x86_64 for cross-platform comparison.
const REFERENCE_HASH: u64 = 0xc493_eb05_b2ac_79d8;

/// Environment variable used to make the child process print the hash.
const PROBE_ENV: &str = "CRUCIBLE_PHYSICS_HASH_PROBE";

/// Scenario: repeating a physics run produces identical bits.
#[test]
fn step_replay_is_bitwise_identical() {
    // Two runs in one process.
    for name in SCENARIOS {
        let traj = Trajectory::load(name);
        let first = run_states(&traj);
        let second = run_states(&traj);
        assert_eq!(first.len(), second.len());
        for (k, (a, b)) in first.iter().zip(&second).enumerate() {
            for (s1, s2) in a.iter().zip(b) {
                let bits = |s: &common::State| {
                    [
                        s.p.x.to_bits(),
                        s.p.y.to_bits(),
                        s.a.to_bits(),
                        s.v.x.to_bits(),
                        s.v.y.to_bits(),
                        u32::from(s.awake),
                    ]
                };
                assert_eq!(
                    bits(s1),
                    bits(s2),
                    "{name}: tick {k} differs between two runs"
                );
            }
        }
    }

    // Snapshot rollback and replay must produce the same bits as the initial run.
    let traj = Trajectory::load("push");
    let mut scene = traj.build();
    let mut snapshot = PhysicsSnapshot::default();
    let mut straight = Vec::new();
    for k in 0..traj.tick_count() {
        if k == 40 {
            scene.world.save_state(&mut snapshot);
        }
        scene.step_reference_tick(&traj, k);
        straight.push(scene.state(0));
    }
    scene.restore(&snapshot, traj.buttons_at(39));
    let bits = |s: &common::State| {
        (
            s.p.x.to_bits(),
            s.p.y.to_bits(),
            s.v.x.to_bits(),
            s.v.y.to_bits(),
            s.awake,
        )
    };
    for (k, t) in straight.iter().enumerate().skip(40) {
        scene.step_reference_tick(&traj, k);
        assert_eq!(
            bits(&scene.state(0)),
            bits(t),
            "push: tick {k} differs after rollback"
        );
    }

    // Launch another process using the same test binary to compute the hash independently.
    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(exe)
        .args([
            "--exact",
            "child_process_hash_probe",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PROBE_ENV, "1")
        .output()
        .expect("spawn the child process");
    assert!(
        out.status.success(),
        "child process failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let child = stdout
        .lines()
        .find_map(|l| l.split("STATE_HASH=").nth(1).map(str::trim))
        .unwrap_or_else(|| panic!("no hash in the child output: {stdout}"));
    assert_eq!(
        child,
        format!("{:016x}", hash_all_scenarios()),
        "the hash differs between processes"
    );
}

/// Prints the bitwise replay hash when launched with the test environment variable.
#[test]
fn child_process_hash_probe() {
    if std::env::var_os(PROBE_ENV).is_some() {
        println!("STATE_HASH={:016x}", hash_all_scenarios());
    }
}

/// Scenario: results agree with the same reference on different operating systems.
///
/// The reference was recorded on Linux. The same test checks its exact bits when run
/// on Windows or another supported platform.
#[test]
fn same_result_on_different_os() {
    let hash = hash_all_scenarios();
    println!(
        "STATE_HASH {} {}: {hash:016x}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    assert_eq!(
        hash, REFERENCE_HASH,
        "state hash {hash:#018x} differs from the reference recorded on Linux x86_64"
    );
}
