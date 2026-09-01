
multithreading?
jumping doesn't always work
fall through floor sometimes
amortize/SoA


guns for mario
bigger hit box/valid attack area

is it possible for some items to not be vertex shaded?  

vars in the console should spawn sliders like the panda3d did
get rid of everything that pulls assets from sm64.  Just keep the assets that have already been pulled and not the code to pull them.  

camera/aim/player pose adjustment
- like overwatch
- the players torso should be able to twist up to 20 degrees in the direction of aim before the legs start moving along too
- need sideways movement animation
- head should also look where aiming

smooth camera/aim without causing lag like overwatch

need vfx energy/aura/trails on weapons


cool centipied enemy that jumps in and out of the ground like LTTP sand worm


X pylons! should share logic with pathing.  traveling salemen problem, creeper world
X place pylons like stellarator, connect to network with beems along LOS.  

grid2quad system

make ow into a level


save file format, including settings

SoA amortization

ctrl+c/ctrl+v in ingame console

pop in shadows random delay



Planet gen topology needs to be walkable and flat enough to farm.  


clone technology?



m16/granades/rocket launcher/shield/turrets

bevy plugins
- bevy_hanabi VFX
- bevy_tweening 
- bevy_asset_loader 
- bevy_kira_audio 
- bevy_egui 
A crowd benchmark control panel.  
Flow-field visualization controls.  
Impostor bake previews.  
Live graphs of simulated/model/impostor enemy counts.  
Frame-spike visualization.  
A level and furniture debugging panel.  
Animation-clip and aim-rig inspection.  
- avian3d
- leafwing-input-manager , input should not latch at render rate.  


dash


X make luna AI playable too


X reorganize, clear clutter, get rid of unneccisary tests

X I do not want to preserve  SM64 collision quirks, including triangle orderingand unusual wall-resolution behavior

sometimes I have CPU load spikes per frame until I change the frame rate limit,but If I change it back to the same limit the spikes go away. this does not happen with vsync off.

health bar should be blue

enemies are getting inside walls a lot, I still fall through things often

need a way to spawn mario


VRR support?

orbs need to obey gravity and fall if an enemy dies on a wall for instance.  

marios have a hard time punching things that are farther away.  enemies should generally prefer to target other marios that are targeting them rather than chasing after units that have a different goal.  

vfx needs lightning/electricity

environmental clouds (prerendered asthetic), lightning, fog, better ground textures.  bumpmaps.  

tree shadows clip, but unit shadows never do.  

make the coper material on the stellarator better

asteroid generation


I want the closes 100 units to be fully simulated instead of impostors, even if they are far away.  For disance based impostors, I don;t want them to change right away. if they enter or leave the range I want there to be a certain probability that they change based on how close they are every cycle to prevent suden pop in along the line as I move around.  

get rid of all "make sure the positions do not change tests".  Thw whole point of the blender is to make the positions change.  

the autopilot should decellerate as well as accelerate for a smooth landing. after the autopilot lands it should disenguage.  when I try to fly off the planet it stays locked on and guides me back to the planet.  I should have a way to just orbit the planet too.  


I’m building a game in Bevy where the actual gameplay world is spherical, but I want the rendered world to appear locally flat around the player/camera.

Conceptually, I want an effect similar to Unity assets like Curved World, except in reverse:

* The real terrain/world geometry remains spherical.
* Physics, collisions, transforms, navigation, gameplay coordinates, and gravity should continue using the true spherical world.
* Only the rendered vertex positions should be modified.
* Visually, the area around the player/camera should look like it lies on a flat tangent plane instead of curving away around a sphere.
* As the player travels around the planet, the locally flat visual representation should follow them.

Please inspect this Bevy project first, including `Cargo.toml`, and determine:

1. Which Bevy version and rendering APIs are being used.
2. How the current terrain/world meshes and materials are implemented.
3. The cleanest way to add this effect without disrupting the existing gameplay systems.

I want to implement the flattening primarily in a WGSL vertex shader or custom Bevy material/render pipeline.

The underlying idea should be approximately:

1. Take each vertex’s true world-space position.
2. Express it relative to the planet center.
3. Determine the local surface frame based on the player/camera position on the sphere.
4. Treat the player's current surface normal as the visual "up" direction.
5. Transform the spherical surface around that point into something approximating a tangent plane.
6. Feed the transformed position into the normal Bevy camera projection.
7. Do not change the actual entity Transform or collider positions.

For example, if:

```text
planet_center = C
player_position = P
vertex_world_position = V
```

then:

```text
player_normal = normalize(P - C)
vertex_direction = normalize(V - C)
planet_radius = length(P - C)
```

I want the shader to visually "unbend" the sphere around `P`.

A simple tangent-plane projection may be a good starting point, but please consider the visual consequences and propose a practical game-friendly mapping.

The result should behave roughly like this:

```text
TRUE WORLD:

                player
                  ↓
             ___/ \___
          __/         \__
        /                 \
       |      planet       |
        \_________________/


RENDERED VIEW:

        _______________________
                  ↑
                player
```

Important requirements:

* The true spherical geometry must remain unchanged.
* Colliders must remain spherical.
* Gravity can continue pointing toward the planet center.
* Gameplay code should still operate in true world coordinates.
* The shader transformation should only affect rendering.
* Separate entities placed on the planet, such as trees, rocks, buildings, characters, etc., should ideally participate in the same visual transformation so they remain attached to the visually flattened ground.
* Their vertex geometry should be warped consistently with the terrain.
* Avoid moving their entity transforms every frame solely for this visual effect.
* The effect should be centered on either the player or camera; recommend whichever gives more stable visuals.
* It should work while moving continuously around the sphere.
* Avoid discontinuities when crossing world axes or poles.
* Keep floating-point precision in mind if the planet is large.
* Normals may need to be transformed/reconstructed so lighting still looks correct.
* Shadows and depth should line up with the warped geometry if reasonably possible.
* Explain any Bevy rendering limitations that affect shadow passes, depth prepasses, motion vectors, picking, etc.
* If custom material shaders need to implement multiple render passes in Bevy, account for that.
* Prefer a clean reusable plugin/material architecture rather than one-off hacks.

I do NOT need a mathematically perfect global map projection. I only need the region around the player to look convincingly flat. Distortion far from the player is acceptable and can be hidden using:

* render distance,
* terrain chunking,
* fog,
* LOD,
* or a configurable flattening radius/falloff.

Please investigate whether a transformation like the following is appropriate:

* Build a local tangent basis at the player:

  * `up = normalize(P - C)`
  * compute stable `right` and `forward` vectors perpendicular to `up`
* For every vertex, determine its angular/geodesic displacement from the player on the sphere.
* Convert that displacement into tangent-plane X/Z coordinates.
* Convert the radial difference from the planet radius into local Y/elevation.
* Reconstruct a flattened render-space position.

Something conceptually like:

```text
sphere/world coordinates
        ↓
player-centered spherical coordinates
        ↓
local tangent coordinates
        ↓
flattened render position
        ↓
camera view/projection
```

Please compare at least these possible approaches:

1. Tangent-plane / exponential-map style projection.
2. Simple curvature cancellation using vertex displacement.
3. Rotating the world into a player-local frame and flattening based on radial height.
4. Any alternative you think would work better in a real-time Bevy game.

Pick the approach you recommend and explain why.

Then implement a minimal working version in this project.

Implementation goals:

* Create the necessary Rust-side Bevy material/plugin/components/resources.
* Pass at least:

  * planet center,
  * planet radius if needed,
  * player/camera surface position,
  * flattening strength/radius,
  * any tangent-frame data needed
    to the shader.
* Implement the vertex deformation in WGSL.
* Make the parameters easy to tune from Rust.
* Preserve existing material textures/colors if possible.
* Structure it so I can eventually use the same deformation on terrain, props, vegetation, and other meshes.

Please comment the important math heavily.

Also add a debug mode if practical, such as:

* flattening strength = 0 showing the real sphere,
* flattening strength = 1 showing the flattened version,
* a runtime toggle between them.

Before making broad architectural changes, inspect the existing code and adapt the solution to it.

When finished, explain:

1. What files you changed.
2. How the spherical-to-flat mapping works mathematically.
3. What coordinate space the shader performs the deformation in.
4. How the shader gets the player/camera and planet data.
5. How to apply the effect to additional materials/entities.
6. Any problems with normals, shadows, culling, depth, frustum calculations, or picking.
7. What you would improve next for production quality.

One particularly important issue: CPU-side frustum culling may think an object is outside the camera view based on its true spherical position even though the shader bends it into view. Please check how Bevy handles this in the version used by the project and propose or implement an appropriate workaround if necessary.

Please make the smallest useful implementation first, verify it compiles, and then iterate from there rather than rewriting the entire renderer.
