# Third-party notices

The following notices apply to adapted source files.

## RobustToolbox

- **Repository:** https://github.com/space-wizards/RobustToolbox
- **Version:** v294.0.3
- **License:** MIT, with additional Zlib notices for Farseer Physics Engine and Box2D and an MIT notice for Box2D by Erin Catto. Modified files carry `modified: yes` and “ALTERED SOURCE VERSION” in their headers.

### Notice

    Copyright (c) 2019 Space Station 14 Contributors

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

### Notice (Zlib: Farseer Physics Engine, Box2D)

From the headers of `SharedPhysicsSystem.Solver.cs`, `SharedPhysicsSystem.Components.cs`, `CollisionManager.cs`, `Manifold.cs`, `ClipVertex.cs`, `ContactVelocityConstraint.cs`, `ContactPositionConstraint.cs`, `Vector4Helpers.cs`, `Transform.cs`. The Rust ports are modified versions.

    Farseer Physics Engine:
    Copyright (c) 2012 Ian Qvist

    Original source Box2D:
    Copyright (c) 2006-2011 Erin Catto http://www.box2d.org

    This software is provided 'as-is', without any express or implied
    warranty.  In no event will the authors be held liable for any damages
    arising from the use of this software.
    Permission is granted to anyone to use this software for any purpose,
    including commercial applications, and to alter it and redistribute it
    freely, subject to the following restrictions:
    1. The origin of this software must not be misrepresented; you must not
    claim that you wrote the original software. If you use this software
    in a product, an acknowledgment in the product documentation would be
    appreciated but is not required.
    2. Altered source versions must be plainly marked as such, and must not be
    misrepresented as being the original software.
    3. This notice may not be removed or altered from any source distribution.

### Notice (MIT: Box2D, Erin Catto)

From `SharedPhysicsSystem.Island.cs` and `CollisionManager.Polygons.cs`.

    MIT License

    Copyright (c) 2019 Erin Catto

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

### Adapted files

| Backend file | Upstream path | Adaptation |
|---|---|---|
| `backend/crates/crucible-physics/src/collision.rs` | `Robust.Shared/Physics/Collision/CollisionManager.PolygonCircle.cs`, `CollisionManager.Polygons.cs` (MIT Erin Catto), `CollisionManager.Circles.cs`, `CollisionManager.cs` (Zlib), `Manifold.cs` (Zlib), `ClipVertex.cs` (Zlib) | Rust port of polygon-circle, polygon-polygon and circle-circle manifolds and segment clipping |
| `backend/crates/crucible-physics/src/solver.rs` | `Robust.Shared/Physics/Systems/SharedPhysicsSystem.Solver.cs`, `Dynamics/Contacts/ContactVelocityConstraint.cs`, `Dynamics/Contacts/ContactPositionConstraint.cs`, `Vector4Helpers.cs`, `Transform.cs` (all Zlib) | Rust port of the Box2D v2 contact solver with a two-point block solver, excluding parallel execution and pooling |
| `backend/crates/crucible-physics/src/island.rs` | `Robust.Shared/Physics/Systems/SharedPhysicsSystem.Island.cs` (MIT Erin Catto) | Rust port of islands, integration, damping and sleep. Joints, gravity, parents and events are outside its scope. |
| `backend/crates/crucible-physics/src/body.rs` | `Robust.Shared/Physics/Systems/SharedPhysicsSystem.Components.cs` (Zlib) | Rust port of the behavior of `SetAwake`, `ResetDynamics`, `SetLinearVelocity`, and `SetAngularVelocity` |
| `backend/crates/crucible-physics/src/math.rs` | `Robust.Shared/Physics/Transform.cs` (Zlib) | Rust port of the operation order in `Transform.Mul` and `MulT` and the `Quaternion2D` rotation using `libm` |
| `backend/crates/crucible-physics/src/shape.rs` | `Robust.Shared/Physics/PhysicsConstants.cs` | Constants `LinearSlop`, `PolygonRadius`, `MaxPolygonVertices` |

## Space Station 14

- **Repository:** https://github.com/space-wizards/space-station-14
- **Version:** 52cf1558aa0f1e8125231a410c12fd090bbc235c
- **License:** MIT (`LICENSE.TXT`)

### Notice

    MIT License

    Copyright (c) 2017-2026 Space Wizards Federation

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

### Adapted files

| Backend file | Upstream path | Adaptation |
|---|---|---|
| `backend/crates/crucible-physics/src/mover.rs` | `Content.Shared/Movement/Systems/SharedMoverController.cs`, `Content.Shared/Movement/Systems/SharedMoverController.Input.cs`, `Content.Shared/Movement/Components/InputMoverComponent.cs`, `Content.Shared/Movement/Components/MovementSpeedModifierComponent.cs` | Rust port of movement on tiles with gravity in `HandleMobMovement`, `Friction`, and `Accelerate`, plus subtick input, movement component fields and their defaults |
