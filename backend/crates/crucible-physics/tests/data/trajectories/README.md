# Reference trajectories

These numeric traces were recorded with the integration test server at [Space Station 14 commit 52cf1558](https://github.com/space-wizards/space-station-14/commit/52cf1558) and [RobustToolbox v294.0.3](https://github.com/space-wizards/RobustToolbox/tree/v294.0.3). They retain the pinned source versions, scenario inputs and per-tick states in the recorded JSON. They contain numeric data and prototype identifiers.

| Fixture | Scenario |
| --- | --- |
| `walk.json` | Walk and stop, then walk diagonally and stop |
| `run.json` | Sprint through three directions and stop |
| `wall.json` | Run into a wall and slide along it |
| `push.json` | Interact with a sliding rigid body |
| `doorway.json` | Slide around a corner into a doorway |

The [regression test](../../reference_trajectories.rs) replays the inputs and checks the resulting trajectories. The format is `crucible-e7-trajectory/1`. The `source` object records how the traces were captured.
