//! Regression tests replay the recorded reference trajectories. On each tick, error must stay
//! within 0.001 m and 0.001 m/s, with an exact match for the sleeping state.

mod common;

use common::{TOLERANCE_POSITION, TOLERANCE_VELOCITY, Trajectory, angle_diff};

/// Maximum deviation for a body across a scenario.
#[derive(Debug, Default)]
struct Deviation {
    max_dp: f32,
    max_dv: f32,
    max_da: f32,
    worst_tick_p: usize,
}

/// Runs the scenario, checks every tick against the reference, and returns each body's deviation.
fn check_reference(name: &str) -> Vec<(String, Deviation)> {
    let traj = Trajectory::load(name);
    let mut scene = traj.build();
    let mut devs: Vec<Deviation> = traj
        .body_names
        .iter()
        .map(|_| Deviation::default())
        .collect();
    let mut failures = Vec::new();
    for k in 0..=traj.tick_count() {
        if k > 0 {
            scene.step_reference_tick(&traj, k - 1);
        }
        for (i, body) in traj.body_names.iter().enumerate() {
            let r = traj.reference(k, body);
            let s = scene.state(i);
            let dp = (s.p - r.p).length();
            let dv = (s.v - r.v).length();
            let da = angle_diff(s.a, r.a);
            let d = &mut devs[i];
            if dp > d.max_dp {
                d.max_dp = dp;
                d.worst_tick_p = k;
            }
            d.max_dv = d.max_dv.max(dv);
            d.max_da = d.max_da.max(da);
            if dp > TOLERANCE_POSITION || dv > TOLERANCE_VELOCITY || s.awake != r.awake {
                failures.push(format!(
                    "{name}/{body} tick {k}: dp {dp:.6} dv {dv:.6} awake {} vs {} (ours {s:?}, reference {r:?})",
                    s.awake, r.awake
                ));
            }
        }
    }
    let out: Vec<(String, Deviation)> = traj.body_names.iter().cloned().zip(devs).collect();
    for (body, d) in &out {
        println!(
            "DEVIATION {name} {body}: ticks {} max_dp {:.6} (tick {}) max_dv {:.6} max_da {:.6}",
            traj.tick_count(),
            d.max_dp,
            d.worst_tick_p,
            d.max_dv,
            d.max_da
        );
    }
    assert!(
        failures.is_empty(),
        "{} ticks out of tolerance:\n{}",
        failures.len(),
        failures.join("\n")
    );
    out
}

/// Reference scenario: walking along a wall.
#[test]
fn reference_walking_along_a_wall() {
    check_reference("wall");
}

/// Walk right, release, walk diagonally, then release.
#[test]
fn reference_walk() {
    check_reference("walk");
}

/// Sprint right, up, and down-left, then release.
#[test]
fn reference_run() {
    check_reference("run");
}

/// A crate pushes the human, who then pushes against it and moves around it.
#[test]
fn reference_push() {
    check_reference("push");
}

/// Hit a corner, slide into the doorway, then move through it diagonally.
#[test]
fn reference_doorway() {
    check_reference("doorway");
}
